pub mod backends;
pub mod filter;
pub mod tools;
pub mod types;

use anyhow::Result;
use async_trait::async_trait;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

pub use filter::mask_sensitive;
pub use types::{AiEvent, Message, PendingCall, ToolResult};

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

    fn state_str(&self) -> &'static str {
        let open_until = *self.open_until.lock().unwrap_or_else(|e| e.into_inner());
        match open_until {
            None => "closed",
            Some(t) if t > Instant::now() => "open",
            Some(_) => "half-open",
        }
    }

    fn allow(&self) -> bool {
        let open_until = *self.open_until.lock().unwrap_or_else(|e| e.into_inner());
        match open_until {
            None => true,
            Some(t) => Instant::now() >= t, // half-open: allow one probe
        }
    }

    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        *self.open_until.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= CB_FAILURE_THRESHOLD {
            let cooldown_until = Instant::now() + CB_COOLDOWN;
            *self.open_until.lock().unwrap_or_else(|e| e.into_inner()) = Some(cooldown_until);
            log::warn!(
                "AI circuit breaker OPEN after {} consecutive failures — \
                 cooling down for {}s before allowing a probe.",
                failures,
                CB_COOLDOWN.as_secs()
            );
        }
    }
}

static CIRCUIT_BREAKER: OnceLock<CircuitBreaker> = OnceLock::new();

fn circuit() -> &'static CircuitBreaker {
    CIRCUIT_BREAKER.get_or_init(CircuitBreaker::new)
}

/// Returns the current AI backend circuit breaker state: `"closed"`, `"open"`, or `"half-open"`.
pub fn circuit_state_str() -> &'static str {
    circuit().state_str()
}

/// Returns the current consecutive failure count for the circuit breaker.
pub fn circuit_failure_count() -> u32 {
    circuit().consecutive_failures.load(Ordering::Relaxed)
}

#[async_trait]
pub trait AiClient: Send + Sync {
    async fn chat(
        &self,
        system_prompt: &str,
        messages: Vec<Message>,
        tx: UnboundedSender<AiEvent>,
    ) -> Result<()>;
}

pub fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap()
    })
}

pub fn next_tool_id() -> String {
    format!(
        "tc_{}",
        TOOL_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
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
                if status.is_success() || status == reqwest::StatusCode::BAD_REQUEST {
                    return Ok(resp);
                }
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    if retries >= 2 {
                        let bytes = resp.bytes().await.unwrap_or_default();
                        let text = String::from_utf8_lossy(&bytes);
                        anyhow::bail!("API error {}: {}", status, text);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2 << retries)).await;
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

/// Construct an [`AiClient`] for the given provider name.
/// `base_url` is used by the OpenAI-compatible backend; pass an empty string
/// for providers that ignore it (Anthropic, Gemini).
/// Defaults to Anthropic for any unrecognised provider string.
pub fn make_client(
    provider: &str,
    api_key: String,
    model: String,
    base_url: String,
) -> Box<dyn AiClient> {
    match provider {
        "openai" | "ollama" | "lmstudio" => Box::new(OpenAiClient::new(api_key, model, base_url)),
        "gemini" => Box::new(GeminiClient::new(api_key, model)),
        _ => Box::new(AnthropicClient::new(api_key, model)),
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
}
