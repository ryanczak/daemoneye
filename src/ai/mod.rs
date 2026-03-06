pub mod backends;
pub mod filter;
pub mod tools;
pub mod types;

use anyhow::Result;
use async_trait::async_trait;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use tokio::sync::mpsc::UnboundedSender;

pub use types::{AiEvent, Message, ToolResult, PendingCall};
pub use filter::mask_sensitive;

pub use backends::anthropic::AnthropicClient;
pub use backends::openai::OpenAiClient;
pub use backends::gemini::GeminiClient;

static TOOL_CALL_ID: AtomicU64 = AtomicU64::new(1);
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

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
    format!("tc_{}", TOOL_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

pub async fn send_with_retry(
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

/// Construct an [`AiClient`] for the given provider name.
/// Defaults to Anthropic for any unrecognised provider string.
pub fn make_client(provider: &str, api_key: String, model: String) -> Box<dyn AiClient> {
    match provider {
        "openai" => Box::new(OpenAiClient::new(api_key, model)),
        "gemini" => Box::new(GeminiClient::new(api_key, model)),
        _ => Box::new(AnthropicClient::new(api_key, model)),
    }
}
