pub mod backends;
pub mod filter;
pub mod tools;
pub mod types;

use crate::util::UnpoisonExt;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::AtomicU64;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

pub use filter::{mask_json_value, mask_sensitive};
pub use types::{AiEvent, Message, PendingCall, TokenBreakdown, ToolResult};

pub use backends::anthropic::AnthropicClient;
pub use backends::gemini::GeminiClient;
pub use backends::openai::OpenAiClient;

pub(crate) mod circuit;

use circuit::CB_COOLDOWN;
#[cfg(test)]
use circuit::CB_FAILURE_THRESHOLD;

static TOOL_CALL_ID: AtomicU64 = AtomicU64::new(1);
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

#[async_trait]
pub trait AiClient: Send + Sync {
    /// Stream a chat completion.
    ///
    /// Set `use_tools = false` for pure-text analysis calls (e.g. watchdog
    /// analysis) where the model must not emit tool/function calls.  Backends
    /// use provider-specific mechanisms to enforce this:
    /// - Gemini: `toolConfig.functionCallingConfig.mode = "NONE"`
    /// - OpenAI: `tools` and `tool_choice` both omitted from body
    /// - Anthropic: tool list omitted from body
    async fn chat(
        &self,
        system_prompt: &str,
        messages: Vec<Message>,
        tx: UnboundedSender<AiEvent>,
        use_tools: bool,
        loaded_tools: Vec<String>,
    ) -> Result<()>;
}

/// Maximum size of the SSE reassembly buffer (1 MiB). A misbehaving proxy
/// that sends data without newlines would otherwise grow it without bound.
const MAX_SSE_BUFFER_BYTES: usize = 1 << 20;

// ── Stream timeouts (two-phase budgets) ─────────────────────────────────────

/// Stream-timeout budgets, read at stream start from `[ai]` config and
/// replaced on every `init_stream_timeouts` call (a reload re-installs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamTimeouts {
    pub first_token: Duration,
    pub stream_idle: Duration,
    pub connect: Duration,
}

impl Default for StreamTimeouts {
    fn default() -> Self {
        StreamTimeouts {
            first_token: Duration::from_secs(600),
            stream_idle: Duration::from_secs(240),
            connect: Duration::from_secs(30),
        }
    }
}

static STREAM_TIMEOUTS: RwLock<Option<StreamTimeouts>> = RwLock::new(None);

/// Install the configured budgets. Safe to call on every reload: a later
/// call replaces the whole set, and a stream already in flight keeps the
/// budgets it read when it started
pub fn init_stream_timeouts(cfg: &crate::config::AiConfig) {
    *STREAM_TIMEOUTS.write().unwrap_or_log() = Some(StreamTimeouts {
        first_token: Duration::from_secs(cfg.first_token_timeout_secs),
        stream_idle: Duration::from_secs(cfg.stream_idle_timeout_secs),
        connect: Duration::from_secs(cfg.connect_timeout_secs),
    });
}

pub fn stream_timeouts() -> StreamTimeouts {
    let guard = STREAM_TIMEOUTS.read().unwrap_or_log();
    guard.as_ref().copied().unwrap_or_default()
}

/// Select which timeout bounds the next stream read: the (long) first-token
/// budget before any real token has arrived, the (shorter) idle budget after.
pub(crate) fn select_timeout(first_token_seen: bool, t: StreamTimeouts) -> std::time::Duration {
    if first_token_seen {
        t.stream_idle
    } else {
        t.first_token
    }
}

/// Read the next stream item under a timeout, converting a stall into an
/// explicit, phase-accurate error instead of an unbounded await.
pub(crate) async fn stream_next_with_timeout<B>(
    stream: &mut (impl futures_util::Stream<Item = reqwest::Result<B>> + Unpin),
    timeout: Duration,
    first_token_seen: bool,
) -> Option<anyhow::Result<B>> {
    use futures_util::StreamExt;
    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(Ok(bytes))) => Some(Ok(bytes)),
        Ok(Some(Err(e))) => Some(Err(scrub_transport_error(e))),
        Ok(None) => None,
        Err(_elapsed) => Some(Err(if first_token_seen {
            anyhow::anyhow!(
                "AI stream went idle mid-response: no data for {}s after output began \
                 (provider or network dropped the stream)",
                timeout.as_secs()
            )
        } else {
            anyhow::anyhow!(
                "AI produced no output for {}s before the first token \
                 (provider accepted the request then went silent)",
                timeout.as_secs()
            )
        })),
    }
}

/// A stream error worth retrying: a transport/body failure (connection dropped
/// mid-stream), as opposed to a stall timeout or a runaway-buffer abort, which
/// are synthetic `anyhow` errors that don't downcast to `reqwest::Error`.
pub(crate) fn is_retriable_transport(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>().is_some()
}

/// Bounded exponential backoff for mid-stream transport retries:
/// 250 ms, 500 ms, 1 s, capped at 2 s.
pub(crate) fn stream_retry_backoff(attempt: u32) -> Duration {
    let ms = (250 * 2u64.pow(attempt.saturating_sub(1))).min(2000);
    Duration::from_millis(ms)
}

/// Incremental SSE line reassembler shared by all streaming backends.
///
/// Network chunks split SSE frames at arbitrary byte positions; this buffers
/// the tail until a full line arrives.  [`SseBuffer::next_data`] yields only
/// `data:` payloads (with or without a space after the colon, both spec-legal)
/// and skips `event:` lines, comments, and blanks.
pub(crate) struct SseBuffer {
    buf: Vec<u8>,
}

impl SseBuffer {
    pub fn new() -> Self {
        SseBuffer { buf: Vec::new() }
    }

    /// Append one network chunk.  Errors if the buffer exceeds
    /// [`MAX_SSE_BUFFER_BYTES`] without a newline.
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > MAX_SSE_BUFFER_BYTES {
            anyhow::bail!(
                "SSE stream buffer exceeded {} bytes without a newline; \
                 aborting to prevent memory exhaustion",
                MAX_SSE_BUFFER_BYTES
            );
        }
        Ok(())
    }

    /// Next complete `data:` payload, or `None` until more bytes arrive.
    ///
    /// Decoding happens here, on a whole line, so a multibyte character
    /// split across two network chunks never becomes two U+FFFDs (B5).
    pub fn next_data(&mut self) -> Option<String> {
        loop {
            let pos = self.buf.iter().position(|b| *b == b'\n')?;
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("data:") {
                return Some(rest.trim_start().to_string());
            }
        }
    }
}

pub fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // No total `.timeout` and no client read timeout: a streamed
            // generation may legitimately run for many minutes, and every
            // backend now bounds its own reads via stream_next_with_timeout
            // (first-token / idle budgets from `[ai]` config). A client-level
            // total timeout would kill long streams mid-response and
            // misreport them as transport errors.
            .connect_timeout(crate::ai::stream_timeouts().connect)
            .build()
            // INVARIANT: default reqwest client config is always valid
            .unwrap()
    })
}

pub fn next_tool_id() -> String {
    format!(
        "tc_{}",
        TOOL_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Ceiling on how long a single retry may sleep, even when the provider's
/// `Retry-After` header asks for more — a turn should fail fast rather than
/// stall for minutes.
const MAX_RETRY_DELAY_SECS: u64 = 60;

/// Delay requested by a `Retry-After` response header, if present and
/// parseable.  Accepts both forms: delta-seconds and HTTP-date.
fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    let when = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let delta = when.signed_duration_since(chrono::Utc::now()).num_seconds();
    // A past date means "retry now"; the caller's backoff floor still applies.
    u64::try_from(delta).ok()
}

async fn send_once<F: Fn() -> reqwest::RequestBuilder>(
    make_req: &F,
) -> std::result::Result<reqwest::Response, reqwest::Error> {
    make_req().send().await
}

fn is_retriable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Rebuild a `reqwest::Error` without its request URL. reqwest's `Error`
/// Display appends the full URL, and the Gemini URL once held the API key, so
/// any transport error that reached a log carried the key with it (M36
/// phase-10, S4). The message still names what went wrong.
fn scrub_transport_error(e: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(e.without_url())
}

/// Send an HTTP request with automatic retry and circuit-breaker protection.
///
/// Returns the response together with the [`circuit::CircuitPermit`] for the
/// request's origin. The caller resolves the outcome once the whole logical
/// call is over — a stream that fails after its headers must resolve the
/// permit, not the header exchange. A neutral client error (any 4xx that is
/// neither 408 nor 429) is resolved with `record_neutral`, which leaves the
/// failure count unchanged.
///
/// On refusal the error names the origin only, never the full URL (the Gemini
/// URL once carried the API key — M36 phase-10, S4).
pub(crate) async fn send_with_retry(
    make_req: impl Fn() -> reqwest::RequestBuilder,
) -> Result<(reqwest::Response, circuit::CircuitPermit)> {
    let req = make_req()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build request: {}", e))?;
    let origin = circuit::request_origin(&req);

    let mut permit = match circuit::acquire(&origin) {
        Some(p) => p,
        None => anyhow::bail!(
            "AI backend circuit breaker is open for {origin} — too many recent failures. \
             Retry in ~{}s.",
            CB_COOLDOWN.as_secs()
        ),
    };

    match send_with_retry_inner(&make_req, &mut permit).await {
        Ok(resp) => Ok((resp, permit)),
        Err(e) => {
            drop(permit);
            Err(e)
        }
    }
}

/// The retry loop proper. It never touches the breaker: the caller already
/// holds the one permit for this logical call, and a retry sent while a
/// half-open probe is in flight would otherwise be refused by its own
/// breaker (bug-03-1 §4).
pub(crate) async fn send_under_permit(
    make_req: &impl Fn() -> reqwest::RequestBuilder,
    permit: &mut circuit::CircuitPermit,
) -> Result<reqwest::Response> {
    send_with_retry_inner(make_req, permit).await
}

async fn send_with_retry_inner(
    make_req: &impl Fn() -> reqwest::RequestBuilder,
    permit: &mut circuit::CircuitPermit,
) -> Result<reqwest::Response> {
    let mut retries: u32 = 0;
    loop {
        let outcome = send_once(make_req).await;
        match outcome {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) if retries < 2 && is_retriable_status(resp.status()) => {
                // Honour the provider's Retry-After when it sends one, capped
                // so a turn cannot stall for minutes; fall back to
                // exponential backoff (2 s, 4 s) otherwise.
                let delay = retry_after_secs(resp.headers())
                    .unwrap_or(2 << retries)
                    .clamp(1, MAX_RETRY_DELAY_SECS);
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                retries += 1;
                continue;
            }
            Ok(resp) => {
                if resp.status().as_u16() == 408 || resp.status().as_u16() == 429 {
                    permit.record_failure();
                } else {
                    permit.record_neutral();
                }
                let status = resp.status();
                let bytes = resp.bytes().await.unwrap_or_default();
                let text = String::from_utf8_lossy(&bytes);
                anyhow::bail!("API error {status}: {text}");
            }
            Err(e) => {
                if retries >= 2 {
                    permit.record_failure();
                    anyhow::bail!("Request failed: {}", scrub_transport_error(e));
                }
                tokio::time::sleep(std::time::Duration::from_secs(2 << retries)).await;
                retries += 1;
            }
        }
    }
}

/// Construct an [`AiClient`] for the given provider name.
/// `base_url` is used by the OpenAI-compatible backend; pass an empty string
/// for providers that ignore it (Anthropic, Gemini).
/// `max_tokens` caps the tokens generated per response
/// (`ModelEntry::effective_max_tokens()` at daemon call sites).
/// Defaults to Anthropic for any unrecognised provider string.
pub fn make_client(
    provider: &str,
    api_key: String,
    model: String,
    base_url: String,
    max_tokens: u32,
) -> Box<dyn AiClient> {
    match provider {
        "openai" | "ollama" | "lmstudio" => {
            Box::new(OpenAiClient::new(api_key, model, base_url, max_tokens))
        }
        "gemini" => Box::new(GeminiClient::new(api_key, model, max_tokens)),
        _ => Box::new(AnthropicClient::new(api_key, model, max_tokens)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn circuit_breaker_closed_initially() {
        let origin = "http://cb-initial.test:1";
        let state = circuit::breaker_for_test(origin);
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.open_until, None);
        assert!(circuit::acquire(origin).is_some(), "a fresh origin admits");
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let origin = "http://cb-open.test:1";
        for i in 1..CB_FAILURE_THRESHOLD {
            let mut permit = circuit::acquire(origin).expect("admitted before threshold");
            permit.record_failure();
            let state = circuit::breaker_for_test(origin);
            assert!(
                state.open_until.is_none(),
                "should still be closed before threshold (failure {i})"
            );
        }
        let mut permit = circuit::acquire(origin).expect("admitted at the threshold");
        permit.record_failure();
        assert!(
            circuit::acquire(origin).is_none(),
            "the threshold-th failure opens the circuit"
        );
        let state = circuit::breaker_for_test(origin);
        assert!(
            state.open_until.is_some(),
            "after threshold failures the circuit should be open"
        );
        assert!(
            circuit::acquire(origin).is_none(),
            "an open circuit refuses"
        );
    }

    #[test]
    fn circuit_breaker_closes_on_success() {
        let origin = "http://cb-close.test:1";
        for _ in 0..CB_FAILURE_THRESHOLD {
            circuit::acquire(origin).expect("admitted").record_failure();
        }
        assert!(
            circuit::acquire(origin).is_none(),
            "an open circuit refuses"
        );
        // Simulate the cooldown expiring.
        let mut state = circuit::breaker_for_test(origin);
        state.open_until = Some(Instant::now() - Duration::from_secs(1));
        circuit::set_breaker_for_test(origin, state);

        // The probe succeeds and closes the circuit.
        let mut permit = circuit::acquire(origin).expect("half-open admits the probe");
        permit.record_success();
        let state = circuit::breaker_for_test(origin);
        assert_eq!(state.open_until, None, "probe success closes the circuit");
        assert_eq!(state.consecutive_failures, 0, "count resets to 0");
        assert!(circuit::acquire(origin).is_some(), "closed admits again");
    }

    #[tokio::test]
    async fn half_open_admits_exactly_one_probe() {
        let origin = "http://cb-half-open.test:1";
        for _ in 0..CB_FAILURE_THRESHOLD {
            circuit::acquire(origin).expect("admitted").record_failure();
        }
        // Expire the cooldown, as `circuit_breaker_closes_on_success` does.
        let mut state = circuit::breaker_for_test(origin);
        state.open_until = Some(Instant::now() - Duration::from_secs(1));
        circuit::set_breaker_for_test(origin, state);

        // The first caller takes the probe.
        let mut permit =
            circuit::acquire(origin).expect("half-open admits the first caller as the probe");
        // A second caller is refused while the probe is in flight.
        assert!(
            circuit::acquire(origin).is_none(),
            "half-open must admit one probe only"
        );
        // Probe success closes the breaker.
        permit.record_success();
        let state = circuit::breaker_for_test(origin);
        assert_eq!(state.open_until, None);
        assert!(circuit::acquire(origin).is_some(), "closed admits again");

        // Probe failure re-opens it for a full cooldown.
        let origin = "http://cb-half-open-fail.test:1";
        for _ in 0..CB_FAILURE_THRESHOLD {
            circuit::acquire(origin).expect("admitted").record_failure();
        }
        let mut state = circuit::breaker_for_test(origin);
        state.open_until = Some(Instant::now() - Duration::from_secs(1));
        circuit::set_breaker_for_test(origin, state);
        let mut permit =
            circuit::acquire(origin).expect("half-open admits the first caller as the probe");
        permit.record_failure();
        let state = circuit::breaker_for_test(origin);
        assert!(state.open_until.is_some(), "probe failure re-opens");
        assert!(
            circuit::acquire(origin).is_none(),
            "a re-opened breaker refuses callers"
        );
    }

    #[test]
    fn make_client_unknown_defaults_to_anthropic() {
        let _c = make_client(
            "unknown_provider",
            "key".to_string(),
            "model".to_string(),
            String::new(),
            4096,
        );
    }

    #[test]
    fn make_client_openai() {
        let _c = make_client(
            "openai",
            "key".to_string(),
            "gpt-4o".to_string(),
            String::new(),
            4096,
        );
    }

    #[test]
    fn make_client_gemini() {
        let _c = make_client(
            "gemini",
            "key".to_string(),
            "gemini-2.0-flash".to_string(),
            String::new(),
            4096,
        );
    }

    #[test]
    fn make_client_ollama() {
        let _c = make_client(
            "ollama",
            "local".to_string(),
            "llama3.2".to_string(),
            "http://localhost:11434/v1".to_string(),
            4096,
        );
    }

    #[test]
    fn make_client_lmstudio() {
        let _c = make_client(
            "lmstudio",
            "local".to_string(),
            "some-model".to_string(),
            "http://localhost:1234/v1".to_string(),
            4096,
        );
    }

    #[test]
    fn select_timeout_uses_first_token_budget_before_first_token() {
        let t = StreamTimeouts {
            first_token: Duration::from_secs(600),
            stream_idle: Duration::from_secs(240),
            ..StreamTimeouts::default()
        };
        assert_eq!(select_timeout(false, t), Duration::from_secs(600));
        assert_eq!(select_timeout(true, t), Duration::from_secs(240));
    }

    #[tokio::test(start_paused = true)]
    async fn stream_next_timeout_reports_first_token_stall() {
        let mut stream = futures_util::stream::pending::<reqwest::Result<Vec<u8>>>();
        let res = super::stream_next_with_timeout(&mut stream, Duration::from_millis(100), false)
            .await
            .expect("stall must produce a result, not None");
        let err = res.expect_err("stall must produce an Err");
        let msg = err.to_string();
        assert!(msg.contains("before the first token"), "got: {msg}");
    }

    #[tokio::test(start_paused = true)]
    async fn stream_next_timeout_reports_mid_stream_idle() {
        let mut stream = futures_util::stream::pending::<reqwest::Result<Vec<u8>>>();
        let res = super::stream_next_with_timeout(&mut stream, Duration::from_millis(100), true)
            .await
            .expect("stall must produce a result, not None");
        let err = res.expect_err("stall must produce an Err");
        let msg = err.to_string();
        assert!(msg.contains("idle mid-response"), "got: {msg}");
    }

    #[test]
    fn stream_retry_backoff_is_bounded() {
        assert_eq!(stream_retry_backoff(1), Duration::from_millis(250));
        assert_eq!(stream_retry_backoff(2), Duration::from_millis(500));
        assert_eq!(stream_retry_backoff(3), Duration::from_millis(1000));
        assert_eq!(stream_retry_backoff(10), Duration::from_millis(2000));
    }

    #[test]
    fn synthetic_stall_error_is_not_retriable_transport() {
        assert!(!is_retriable_transport(&anyhow::anyhow!("stall")));
    }

    #[test]
    fn stream_timeouts_defaults_without_init() {
        let t = stream_timeouts();
        assert_eq!(t.first_token, Duration::from_secs(600));
        assert_eq!(t.stream_idle, Duration::from_secs(240));
        assert_eq!(t.connect, Duration::from_secs(30));
    }

    // ── M36 phase-10 (S4) — a reqwest error never carries its URL ────────────

    #[test]
    fn secrets_transport_error_drops_url() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(async {
            client
                .get("http://127.0.0.1:1/?key=SECRETKEY123")
                .send()
                .await
                .unwrap_err()
        });
        assert!(
            err.url().is_some(),
            "the raw reqwest error must carry the URL before scrubbing: {:?}",
            err.url()
        );
        let scrubbed = scrub_transport_error(err);
        let text = format!("{scrubbed:#}");
        assert!(
            !text.contains("SECRETKEY123"),
            "the scrubbed error must not carry the key: {text}"
        );
        assert!(
            !text.contains("127.0.0.1:1/"),
            "the scrubbed error must not carry the URL path: {text}"
        );
        assert!(
            !text.is_empty(),
            "the scrubbed error must still name what went wrong: {text:?}"
        );
    }

    #[test]
    fn secrets_transport_error_keeps_cause() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(async {
            client
                .get("http://127.0.0.1:1/?key=SECRETKEY123")
                .send()
                .await
                .unwrap_err()
        });
        let scrubbed = scrub_transport_error(err);
        let text = format!("{scrubbed:#}");
        assert!(
            text.contains("tcp connect error"),
            "the scrubbed error must keep its cause (the connection failure): {text}"
        );
        assert!(
            !text.contains("SECRETKEY123"),
            "the scrubbed error must not carry the key: {text}"
        );
    }
}

#[cfg(test)]
mod retry_after_tests {
    use super::retry_after_secs;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    fn headers_with(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn parses_delta_seconds() {
        assert_eq!(retry_after_secs(&headers_with("17")), Some(17));
    }

    #[test]
    fn parses_http_date_in_the_future() {
        let when = chrono::Utc::now() + chrono::Duration::seconds(30);
        let secs = retry_after_secs(&headers_with(&when.to_rfc2822())).expect("parseable date");
        assert!((28..=30).contains(&secs), "got {secs}");
    }

    #[test]
    fn ignores_garbage_and_missing_header() {
        assert_eq!(retry_after_secs(&headers_with("soon-ish")), None);
        assert_eq!(retry_after_secs(&HeaderMap::new()), None);
    }
}

#[cfg(test)]
mod sse_buffer_tests {
    use super::SseBuffer;

    #[test]
    fn reassembles_lines_split_across_chunks() {
        let mut sse = SseBuffer::new();
        sse.push(b"data: {\"a\":").unwrap();
        assert!(sse.next_data().is_none(), "incomplete line must buffer");
        sse.push(b"1}\ndata: [DONE]\n").unwrap();
        assert_eq!(sse.next_data().as_deref(), Some("{\"a\":1}"));
        assert_eq!(sse.next_data().as_deref(), Some("[DONE]"));
        assert!(sse.next_data().is_none());
    }

    #[test]
    fn accepts_data_prefix_without_space_and_crlf() {
        let mut sse = SseBuffer::new();
        sse.push(b"data:{\"x\":2}\r\n").unwrap();
        assert_eq!(sse.next_data().as_deref(), Some("{\"x\":2}"));
    }

    #[test]
    fn skips_event_lines_comments_and_blanks() {
        let mut sse = SseBuffer::new();
        sse.push(b"event: message_start\n: keep-alive\n\ndata: {}\n")
            .unwrap();
        assert_eq!(sse.next_data().as_deref(), Some("{}"));
        assert!(sse.next_data().is_none());
    }

    #[test]
    fn errors_when_buffer_grows_without_newline() {
        let mut sse = SseBuffer::new();
        let chunk = vec![b'x'; 600 * 1024];
        sse.push(&chunk).unwrap();
        let err = sse.push(&chunk).unwrap_err().to_string();
        assert!(err.contains("exceeded"), "got: {err}");
    }

    // ── M36 phase-11 (B5) — a multibyte character split across chunks ─────

    #[test]
    fn sse_utf8_split_two_byte() {
        let mut sse = SseBuffer::new();
        sse.push(b"data: caf\xC3").unwrap();
        assert!(sse.next_data().is_none(), "incomplete line must buffer");
        sse.push(b"\xA9\n").unwrap();
        assert_eq!(sse.next_data().as_deref(), Some("café"));
    }

    #[test]
    fn sse_utf8_split_four_byte_three_chunks() {
        let mut sse = SseBuffer::new();
        sse.push(b"data: \xF0").unwrap();
        sse.push(b"\x9F\x99").unwrap();
        sse.push(b"\x82\n").unwrap();
        assert_eq!(sse.next_data().as_deref(), Some("\u{1F642}"));
    }

    #[test]
    fn sse_utf8_invalid_bytes_lossy() {
        let mut sse = SseBuffer::new();
        sse.push(b"data: bad \xFF byte\n").unwrap();
        let line = sse.next_data().expect("a complete line");
        assert!(
            line.contains('\u{FFFD}'),
            "an invalid byte decodes to U+FFFD: {line:?}"
        );
    }
}

#[cfg(test)]
mod stream_idle_tests {
    use std::time::Duration;

    /// Serve HTTP 200 + one SSE chunk, then go silent without closing the
    /// socket — the exact mechanism-C shape. Returns the bound address.
    async fn silent_after_first_chunk() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                // Read the whole request before answering. A response that
                // arrives before the request is written is an
                // `UnexpectedMessage` to hyper, and under tokio 1.54's
                // scheduling that race fired on every run.
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\n\
                          Content-Type: text/event-stream\r\n\
                          Transfer-Encoding: chunked\r\n\r\n\
                          5\r\nhello\r\n",
                    )
                    .await;
                let _ = sock.flush().await;
                // Hold the connection open, sending nothing further. `pending()`
                // never resolves, so the socket stays open with no clock involved.
                std::future::pending::<()>().await;
            }
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn idle_stream_stall_is_reported_by_stream_next_with_timeout() {
        let url = silent_after_first_chunk().await;
        let client = reqwest::Client::new();
        let resp = client.get(&url).send().await.unwrap();
        let mut stream = resp.bytes_stream();

        let first =
            crate::ai::stream_next_with_timeout(&mut stream, Duration::from_millis(500), false)
                .await
                .expect("a stream item")
                .expect("first chunk");
        assert!(!first.is_empty(), "first chunk must be hello");

        let second =
            crate::ai::stream_next_with_timeout(&mut stream, Duration::from_millis(300), true)
                .await
                .expect("a second stream item")
                .expect_err("second read must time out, not succeed");
        let msg = second.to_string();
        assert!(
            msg.contains("idle mid-response"),
            "idle timeout must be reported as a mid-response stall, got: {msg}"
        );
    }
}

#[cfg(test)]
mod stream_timeout_reinit_tests {
    use super::{StreamTimeouts, init_stream_timeouts, stream_timeouts};
    use std::time::Duration;

    /// `init_stream_timeouts` replaces the budget set on every call, so a
    /// reload follows `[ai]`; a stream that already read the old set keeps it.
    #[test]
    fn stream_timeouts_reinit_applies_new_budgets() {
        assert_eq!(
            stream_timeouts(),
            StreamTimeouts::default(),
            "defaults before any init"
        );

        let cfg = crate::config::AiConfig {
            first_token_timeout_secs: 11,
            stream_idle_timeout_secs: 22,
            connect_timeout_secs: 33,
            ..Default::default()
        };
        init_stream_timeouts(&cfg);
        let first = stream_timeouts();
        assert_eq!(
            first.first_token,
            Duration::from_secs(11),
            "first-token budget applied"
        );
        assert_eq!(
            first.stream_idle,
            Duration::from_secs(22),
            "idle budget applied"
        );
        assert_eq!(
            first.connect,
            Duration::from_secs(33),
            "connect budget applied"
        );

        // A caller that borrowed the set before the re-init keeps its copy.
        let in_flight = first;
        init_stream_timeouts(&crate::config::AiConfig::default());
        let after = stream_timeouts();
        assert_eq!(
            after.first_token,
            Duration::from_secs(600),
            "re-init back to defaults replaces the set"
        );
        assert_eq!(
            in_flight.first_token,
            Duration::from_secs(11),
            "a stream that started under the old budgets keeps them"
        );
    }
}

#[cfg(test)]
mod circuit_request_path_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    /// A local server that reads the whole request before answering, as
    /// `stream_idle_tests` does. `status` picks the answer; `hits` counts.
    async fn local_server(status: &'static str) -> (String, std::sync::Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let hit_counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let counter = hit_counter.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&chunk[..n]),
                        }
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    let body = if status.starts_with("HTTP/1.1 200") {
                        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n"
                    } else {
                        "{\"error\":{\"message\":\"no\"}}\n"
                    };
                    let response = format!(
                        "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (format!("http://{addr}/"), hits)
    }

    fn stream_body() -> serde_json::Value {
        serde_json::json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]})
    }

    fn make_build_request(
        base: String,
        body: serde_json::Value,
    ) -> impl Fn() -> reqwest::RequestBuilder {
        move || {
            http()
                .post(format!("{base}chat/completions"))
                .bearer_auth("k")
                .json(&body)
        }
    }

    #[tokio::test]
    async fn headers_alone_do_not_record_success() {
        let (base, hits) = local_server("HTTP/1.1 200 OK").await;
        let origin = {
            let req = make_build_request(base.clone(), stream_body())()
                .build()
                .unwrap();
            circuit::request_origin(&req)
        };
        for _ in 0..CB_FAILURE_THRESHOLD - 1 {
            circuit::acquire(&origin)
                .expect("admitted")
                .record_failure();
        }
        assert_eq!(
            circuit::breaker_for_test(&origin).consecutive_failures,
            CB_FAILURE_THRESHOLD - 1
        );
        // One real header exchange through the request path: the 200 headers
        // alone must not record success, so the count stays at threshold - 1.
        let result = send_with_retry(make_build_request(base.clone(), stream_body())).await;
        assert!(result.is_ok(), "a 200 must succeed");
        assert_eq!(
            circuit::breaker_for_test(&origin).consecutive_failures,
            CB_FAILURE_THRESHOLD - 1,
            "a header exchange alone records nothing"
        );
        assert!(
            hits.load(Ordering::SeqCst) >= 1,
            "the server must have seen the request"
        );
        // So one more recorded failure is the threshold and opens the breaker.
        circuit::acquire(&origin)
            .expect("still closed below the threshold")
            .record_failure();
        assert!(
            circuit::acquire(&origin).is_none(),
            "one more failure must open the breaker"
        );
    }

    #[tokio::test]
    async fn a_client_error_does_not_trip_the_breaker() {
        let (base, _hits) = local_server("HTTP/1.1 400 Bad Request").await;
        let origin = {
            let req = make_build_request(base.clone(), stream_body())()
                .build()
                .unwrap();
            circuit::request_origin(&req)
        };
        for _ in 0..CB_FAILURE_THRESHOLD + 1 {
            let result = send_with_retry(make_build_request(base.clone(), stream_body())).await;
            assert!(result.is_err(), "a 400 must surface as an error");
        }
        assert_eq!(
            circuit::breaker_for_test(&origin).consecutive_failures,
            0,
            "client errors are neutral: the count never moves"
        );
        assert!(
            circuit::acquire(&origin).is_some(),
            "the breaker for that origin stays closed"
        );
    }

    #[tokio::test]
    async fn a_client_error_leaves_the_failure_count_unchanged() {
        let (base, _hits) = local_server("HTTP/1.1 400 Bad Request").await;
        let origin = {
            let req = make_build_request(base.clone(), stream_body())()
                .build()
                .unwrap();
            circuit::request_origin(&req)
        };
        for _ in 0..CB_FAILURE_THRESHOLD - 1 {
            circuit::acquire(&origin)
                .expect("admitted")
                .record_failure();
        }
        assert_eq!(
            circuit::breaker_for_test(&origin).consecutive_failures,
            CB_FAILURE_THRESHOLD - 1
        );
        // One neutral 4xx through the real request path must not move the count.
        let result = send_with_retry(make_build_request(base.clone(), stream_body())).await;
        assert!(result.is_err(), "a 400 must surface as an error");
        assert_eq!(
            circuit::breaker_for_test(&origin).consecutive_failures,
            CB_FAILURE_THRESHOLD - 1,
            "a neutral 4xx leaves the failure count unchanged"
        );
        // So one more recorded failure is the threshold and opens the breaker.
        circuit::acquire(&origin)
            .expect("still closed below the threshold")
            .record_failure();
        assert!(
            circuit::acquire(&origin).is_none(),
            "one more failure after the neutral must open the breaker"
        );
    }

    #[tokio::test]
    async fn a_probe_that_fails_before_its_first_token_reopens_the_breaker() {
        // The reviewer's fixture: the server reads the whole request, writes a
        // 200 header plus one SSE chunk, then drops the socket. The stream
        // fails mid-response, so the call retries under its own permit.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let hit_counter = hits.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let counter = hit_counter.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&chunk[..n]),
                        }
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = sock
                        .write_all(
                            b"HTTP/1.1 200 OK\r\n\
                              Content-Type: text/event-stream\r\n\
                              Transfer-Encoding: chunked\r\n\r\n\
                              5\r\nhello\r\n",
                        )
                        .await;
                    let _ = sock.flush().await;
                    // Drop the socket: the stream fails before its first token.
                });
            }
        });
        let base = format!("http://{addr}/v1");
        let origin = {
            let req = http()
                .post(format!("{base}/chat/completions"))
                .bearer_auth("k")
                .json(&serde_json::json!({
                    "model": "m",
                    "stream": true,
                    "messages": [{"role": "system", "content": "sys"}]
                }))
                .build()
                .unwrap();
            circuit::request_origin(&req)
        };
        for _ in 0..CB_FAILURE_THRESHOLD {
            circuit::acquire(&origin)
                .expect("admitted")
                .record_failure();
        }
        let mut state = circuit::breaker_for_test(&origin);
        state.open_until = Some(Instant::now() - Duration::from_secs(1));
        circuit::set_breaker_for_test(&origin, state);

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let client = crate::ai::backends::openai::OpenAiClient::new(
            "k".into(),
            "m".into(),
            base.clone(),
            16,
        );
        let result = client.chat("sys", vec![], tx, false, vec![]).await;
        server.abort();

        let err = result.expect_err("the stream failed before its first token");
        let msg = err.to_string();
        assert!(
            !msg.contains("circuit breaker is open"),
            "the probe's retries run under its own permit, got: {msg}"
        );
        assert!(
            hits.load(Ordering::SeqCst) > 1,
            "the server saw more than one request, so the retries ran"
        );
        let state = circuit::breaker_for_test(&origin);
        assert!(
            state.open_until.is_some_and(|until| until > Instant::now()),
            "the failed probe re-opens the breaker"
        );
        assert!(
            circuit::acquire(&origin).is_none(),
            "a re-opened breaker refuses"
        );
    }

    #[tokio::test]
    async fn breakers_are_per_endpoint() {
        let (a_base, _) = local_server("HTTP/1.1 200 OK").await;
        let (b_base, b_hits) = local_server("HTTP/1.1 200 OK").await;
        let origin_a = {
            let req = make_build_request(a_base.clone(), stream_body())()
                .build()
                .unwrap();
            circuit::request_origin(&req)
        };
        for _ in 0..CB_FAILURE_THRESHOLD {
            circuit::acquire(&origin_a)
                .expect("admitted")
                .record_failure();
        }
        assert!(
            circuit::acquire(&origin_a).is_none(),
            "endpoint A's breaker is open"
        );
        let result = send_with_retry(make_build_request(b_base.clone(), stream_body())).await;
        assert!(result.is_ok(), "endpoint B must still be served");
        assert!(
            b_hits.load(Ordering::SeqCst) >= 1,
            "the request actually reached endpoint B"
        );
    }

    #[tokio::test]
    async fn a_cancelled_probe_frees_the_slot() {
        let origin = "http://cb-cancelled-probe.test:1";
        for _ in 0..CB_FAILURE_THRESHOLD {
            circuit::acquire(origin).expect("admitted").record_failure();
        }
        let mut state = circuit::breaker_for_test(origin);
        state.open_until = Some(Instant::now() - Duration::from_secs(1));
        circuit::set_breaker_for_test(origin, state);

        {
            let probe = circuit::acquire(origin).expect("the probe is admitted");
            drop(probe);
        }
        // Nothing was recorded, and the slot is free: the next caller probes.
        let mut next =
            circuit::acquire(origin).expect("a cancelled probe records nothing and frees the slot");
        next.record_success();
        let state = circuit::breaker_for_test(origin);
        assert_eq!(state.open_until, None, "the probe succeeded: closed");
        assert_eq!(state.consecutive_failures, 0);
    }
}
