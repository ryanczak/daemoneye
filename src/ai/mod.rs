pub mod backends;
pub mod filter;
pub mod tools;
pub mod types;

use crate::util::UnpoisonExt;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

pub use filter::{mask_json_value, mask_sensitive};
pub use types::{AiEvent, Message, PendingCall, TokenBreakdown, ToolResult};

pub use backends::anthropic::AnthropicClient;
pub use backends::gemini::GeminiClient;
pub use backends::openai::OpenAiClient;

static TOOL_CALL_ID: AtomicU64 = AtomicU64::new(1);
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

// ── Circuit breaker (F5) ─────────────────────────────────────────────────────

/// Consecutive failures before the circuit trips open.
const CB_FAILURE_THRESHOLD: u32 = 5;
/// How long the circuit stays open before allowing a probe request.
const CB_COOLDOWN: Duration = Duration::from_secs(60);

struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    open_until: std::sync::Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            open_until: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn state_str(&self) -> &'static str {
        let open_until = *self.open_until.lock().unwrap_or_log();
        match open_until {
            None => "closed",
            Some(t) if t > Instant::now() => "open",
            Some(_) => "half-open",
        }
    }

    fn allow(&self) -> bool {
        let open_until = *self.open_until.lock().unwrap_or_log();
        match open_until {
            None => true,
            Some(t) => Instant::now() >= t, // half-open: allow one probe
        }
    }

    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        let prev = self.open_until.lock().unwrap_or_log().take();
        if prev.is_some() {
            log::info!("AI circuit breaker CLOSED — backend responded successfully.");
        }
    }

    fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= CB_FAILURE_THRESHOLD {
            let cooldown_until = Instant::now() + CB_COOLDOWN;
            *self.open_until.lock().unwrap_or_log() = Some(cooldown_until);
            log::warn!(
                "AI circuit breaker OPEN after {} consecutive failures — \
                 cooling down for {}s before allowing a probe.",
                failures,
                CB_COOLDOWN.as_secs()
            );
        } else {
            log::warn!(
                "AI backend request failed ({}/{} failures before circuit opens).",
                failures,
                CB_FAILURE_THRESHOLD
            );
        }
    }
}

static CIRCUIT_BREAKER: OnceLock<CircuitBreaker> = OnceLock::new();

fn circuit() -> &'static CircuitBreaker {
    CIRCUIT_BREAKER.get_or_init(CircuitBreaker::new)
}

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

/// Idle ceiling for a single read from an in-flight AI response stream.
///
/// `Client::timeout` bounds the *whole* request; it cannot tell a slow-but-alive
/// provider from one that accepted the connection and went silent. Without a
/// per-read bound, a quiet provider freezes the turn for the full total timeout
/// with no diagnostic — `docs/design/daemon-stalls.md` § 1.5 (mechanism C).
pub const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Normalise one chunk of an AI response stream, converting an idle-read
/// timeout into a diagnosable error and logging it.
///
/// Generic over the chunk payload so the `bytes` crate need not be a direct
/// dependency.
pub fn stream_chunk<T>(chunk: reqwest::Result<T>) -> Result<T> {
    chunk.map_err(|e| {
        if e.is_timeout() {
            log::error!(
                "AI stream stalled: no data for {}s — provider accepted the \
                 connection then went silent (mechanism C)",
                STREAM_IDLE_TIMEOUT.as_secs()
            );
            anyhow::anyhow!(
                "AI stream stalled: the provider sent no data for {}s",
                STREAM_IDLE_TIMEOUT.as_secs()
            )
        } else {
            log::error!("AI stream read failed: {e}");
            anyhow::anyhow!("AI stream read failed: {e}")
        }
    })
}

/// Maximum size of the SSE reassembly buffer (1 MiB). A misbehaving proxy
/// that sends data without newlines would otherwise grow it without bound.
const MAX_SSE_BUFFER_BYTES: usize = 1 << 20;

// ── Stream timeouts (two-phase budgets) ─────────────────────────────────────

/// Stream-timeout budgets, set once at daemon start from `[ai]` config.
#[derive(Debug, Clone, Copy)]
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

static STREAM_TIMEOUTS: OnceLock<StreamTimeouts> = OnceLock::new();

/// Install the configured budgets. Later calls are ignored (OnceLock).
pub fn init_stream_timeouts(cfg: &crate::config::AiConfig) {
    let _ = STREAM_TIMEOUTS.set(StreamTimeouts {
        first_token: Duration::from_secs(cfg.first_token_timeout_secs),
        stream_idle: Duration::from_secs(cfg.stream_idle_timeout_secs),
        connect: Duration::from_secs(cfg.connect_timeout_secs),
    });
}

pub fn stream_timeouts() -> StreamTimeouts {
    STREAM_TIMEOUTS.get().copied().unwrap_or_default()
}

/// Select which timeout bounds the next stream read: the (long) first-token
/// budget before any real token has arrived, the (shorter) idle budget after.
#[cfg_attr(not(test), expect(dead_code))] // phase-02/03 start consuming these helpers
pub(crate) fn select_timeout(first_token_seen: bool, t: StreamTimeouts) -> std::time::Duration {
    if first_token_seen {
        t.stream_idle
    } else {
        t.first_token
    }
}

/// Read the next stream item under a timeout, converting a stall into an
/// explicit, phase-accurate error instead of an unbounded await.
#[cfg_attr(not(test), expect(dead_code))] // phase-02/03 start consuming these helpers
pub(crate) async fn stream_next_with_timeout<B>(
    stream: &mut (impl futures_util::Stream<Item = reqwest::Result<B>> + Unpin),
    timeout: Duration,
    first_token_seen: bool,
) -> Option<anyhow::Result<B>> {
    use futures_util::StreamExt;
    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(Ok(bytes))) => Some(Ok(bytes)),
        Ok(Some(Err(e))) => Some(Err(e.into())),
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
#[cfg_attr(not(test), expect(dead_code))] // phase-02/03 start consuming these helpers
pub(crate) fn is_retriable_transport(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>().is_some()
}

/// Bounded exponential backoff for mid-stream transport retries:
/// 250 ms, 500 ms, 1 s, capped at 2 s.
#[cfg_attr(not(test), expect(dead_code))] // phase-02/03 start consuming these helpers
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
    leftover: String,
}

impl SseBuffer {
    pub fn new() -> Self {
        SseBuffer {
            leftover: String::new(),
        }
    }

    /// Append one network chunk.  Errors if the buffer exceeds
    /// [`MAX_SSE_BUFFER_BYTES`] without a newline.
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.leftover.push_str(&String::from_utf8_lossy(bytes));
        if self.leftover.len() > MAX_SSE_BUFFER_BYTES {
            anyhow::bail!(
                "SSE stream buffer exceeded {} bytes without a newline; \
                 aborting to prevent memory exhaustion",
                MAX_SSE_BUFFER_BYTES
            );
        }
        Ok(())
    }

    /// Next complete `data:` payload, or `None` until more bytes arrive.
    pub fn next_data(&mut self) -> Option<String> {
        while let Some(pos) = self.leftover.find('\n') {
            let line: String = self.leftover.drain(..=pos).collect();
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("data:") {
                return Some(rest.trim_start().to_string());
            }
        }
        None
    }
}

pub fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .read_timeout(STREAM_IDLE_TIMEOUT)
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

async fn send_with_retry_inner(
    make_req: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    let mut retries = 0;
    loop {
        let req = make_req();
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp);
                }
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    if retries >= 2 {
                        let bytes = resp.bytes().await.unwrap_or_default();
                        let text = String::from_utf8_lossy(&bytes);
                        anyhow::bail!("API error {}: {}", status, text);
                    }
                    // Honour the provider's Retry-After when it sends one,
                    // capped so a turn cannot stall for minutes; fall back to
                    // exponential backoff (2 s, 4 s) otherwise.
                    let delay = retry_after_secs(resp.headers())
                        .unwrap_or(2 << retries)
                        .clamp(1, MAX_RETRY_DELAY_SECS);
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    retries += 1;
                    continue;
                }
                let bytes = resp.bytes().await.unwrap_or_default();
                let text = String::from_utf8_lossy(&bytes);
                anyhow::bail!("API error {}: {}", status, text);
            }
            Err(e) => {
                if retries >= 2 {
                    anyhow::bail!("Request failed: {}", e);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2 << retries)).await;
                retries += 1;
            }
        }
    }
}

/// Send an HTTP request with automatic retry and circuit-breaker protection.
///
/// The circuit opens after [`CB_FAILURE_THRESHOLD`] consecutive failures and
/// stays open for [`CB_COOLDOWN`] before allowing a single probe request.
/// A successful probe closes the circuit; a failed probe re-opens it.
pub async fn send_with_retry(
    make_req: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    if !circuit().allow() {
        anyhow::bail!(
            "AI backend circuit breaker is open — too many recent failures. \
             Retry in ~{}s.",
            CB_COOLDOWN.as_secs()
        );
    }
    match send_with_retry_inner(make_req).await {
        Ok(resp) => {
            circuit().record_success();
            Ok(resp)
        }
        Err(e) => {
            circuit().record_failure();
            Err(e)
        }
    }
}

/// Record a mid-stream failure against the circuit breaker. `send_with_retry`
/// only accounts for the header exchange; backends call this when the stream
/// itself fails after a 200.
#[expect(dead_code)] // phase-02/03 start consuming these helpers
pub(crate) fn record_stream_failure() {
    circuit().record_failure();
}

/// Record a stream that reached its natural end.
#[expect(dead_code)] // phase-02/03 start consuming these helpers
pub(crate) fn record_stream_success() {
    circuit().record_success();
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

    #[test]
    fn circuit_breaker_closed_initially() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state_str(), "closed");
        assert!(cb.allow());
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new();
        for _ in 0..CB_FAILURE_THRESHOLD {
            assert!(cb.allow(), "should still be allowed before threshold");
            cb.record_failure();
        }
        // After threshold failures the circuit should be open.
        assert_eq!(cb.state_str(), "open");
        assert!(!cb.allow());
    }

    #[test]
    fn circuit_breaker_closes_on_success() {
        let cb = CircuitBreaker::new();
        // Trip the circuit.
        for _ in 0..CB_FAILURE_THRESHOLD {
            cb.record_failure();
        }
        assert_eq!(cb.state_str(), "open");
        // Force-close by simulating the cooldown expiring: set open_until to the past.
        {
            let mut guard = cb.open_until.lock().unwrap();
            *guard = Some(Instant::now() - Duration::from_secs(1));
        }
        assert_eq!(cb.state_str(), "half-open");
        assert!(cb.allow());
        cb.record_success();
        assert_eq!(cb.state_str(), "closed");
        assert!(cb.allow());
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
}

#[cfg(test)]
mod stream_idle_tests {
    use futures_util::StreamExt;

    /// Serve HTTP 200 + one SSE chunk, then go silent without closing the
    /// socket — the exact mechanism-C shape. Returns the bound address.
    async fn silent_after_first_chunk() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
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
    async fn idle_stream_times_out_and_reports_a_stall() {
        let url = silent_after_first_chunk().await;
        let client = reqwest::Client::builder()
            .read_timeout(std::time::Duration::from_millis(300))
            .build()
            .unwrap();
        let resp = client.get(&url).send().await.unwrap();
        let mut stream = resp.bytes_stream();

        let first = stream.next().await.expect("first chunk");
        assert!(
            super::stream_chunk(first).is_ok(),
            "first chunk must arrive"
        );

        let second = stream.next().await.expect("a second stream item");
        assert!(second.is_err(), "second read must time out, not succeed");
        let err = super::stream_chunk(second).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("stalled"),
            "idle timeout must be reported as a stall, got: {msg}"
        );
    }
}
