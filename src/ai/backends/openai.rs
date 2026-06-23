use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::tools::{dispatch_tool_event, get_openai_tool_definition};
use crate::ai::types::{AiEvent, Message, TokenBreakdown};
use crate::ai::{AiClient, http, send_with_retry};

/// Parse the `usage` object from an OpenAI SSE event into a `TokenBreakdown`.
/// Reads `prompt_tokens`, `completion_tokens`, and `prompt_tokens_details.cached_tokens`;
/// subtracts cached_tokens from the total prompt to yield uncached `input_tokens`.
pub(crate) fn parse_openai_usage(u: &serde_json::Map<String, Value>) -> TokenBreakdown {
    let total_prompt = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let output_tokens = u
        .get("completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_read = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    TokenBreakdown {
        input_tokens: total_prompt.saturating_sub(cache_read),
        output_tokens,
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
    }
}

/// OpenAI-compatible API backend (GPT family, or any OpenAI-compatible endpoint).
/// Supports Ollama, LM Studio, vLLM, and any other OpenAI-API-compatible server
/// by passing the appropriate `base_url` (e.g. `http://localhost:11434/v1`).
pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiClient {
    /// Create a new OpenAI-compatible client.
    /// `base_url` should be the full base URL including `/v1`, e.g.
    /// `https://api.openai.com/v1` or `http://localhost:11434/v1`.
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        let resolved_url = if base_url.is_empty() {
            "https://api.openai.com/v1".to_string()
        } else {
            base_url
        };
        OpenAiClient {
            api_key,
            model,
            base_url: resolved_url,
        }
    }

    fn convert_messages(&self, messages: Vec<Message>) -> Vec<Value> {
        let mut result = Vec::new();
        for m in messages {
            if let Some(trs) = m.tool_results {
                // OpenAI expects one role: "tool" message per result.
                for tr in trs {
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_call_id,
                        "content": tr.content
                    }));
                }
            } else if let Some(tcs) = m.tool_calls {
                let mut tool_calls = Vec::new();
                for tc in tcs {
                    tool_calls.push(json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments
                        }
                    }));
                }
                result.push(json!({
                    "role": "assistant",
                    "content": m.content,
                    "tool_calls": tool_calls
                }));
            } else {
                result.push(json!({
                    "role": m.role,
                    "content": m.content
                }));
            }
        }
        result
    }
}

#[async_trait]
impl AiClient for OpenAiClient {
    async fn chat(
        &self,
        system: &str,
        messages: Vec<Message>,
        tx: UnboundedSender<AiEvent>,
        use_tools: bool,
        loaded_tools: Vec<String>,
    ) -> Result<()> {
        let converted = self.convert_messages(messages);
        let mut full_messages = vec![json!({"role": "system", "content": system})];
        full_messages.extend(converted);

        let mut body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "stream": true,
            "stream_options": { "include_usage": true },
            "messages": full_messages,
        });
        if use_tools {
            body["tools"] = json!(get_openai_tool_definition(&loaded_tools));
        } else {
            body["tool_choice"] = json!("none");
        }

        let response = send_with_retry(|| {
            http()
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
        })
        .await?;

        let mut stream = response.bytes_stream();
        let mut tool_id = String::new();
        let mut tool_name = String::new();
        let mut tool_args = String::new();
        let mut leftover = String::new();
        let mut usage = TokenBreakdown::default();

        /// Maximum size of the SSE leftover buffer (1 MiB). A misbehaving
        /// proxy that sends data without newlines would otherwise grow it
        /// without bound.
        const MAX_LEFTOVER_BYTES: usize = 1 << 20;

        'outer: while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            leftover.push_str(&String::from_utf8_lossy(&bytes));
            if leftover.len() > MAX_LEFTOVER_BYTES {
                return Err(anyhow::anyhow!(
                    "SSE stream leftover buffer exceeded {} bytes without a newline; \
                     aborting to prevent memory exhaustion",
                    MAX_LEFTOVER_BYTES
                ));
            }

            while let Some(pos) = leftover.find('\n') {
                let line = leftover[..pos].trim().to_string();
                leftover = leftover[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break 'outer;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(delta) =
                            v["choices"].get(0).and_then(|c| c["delta"].as_object())
                        {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                                && !content.is_empty()
                            {
                                let _ = tx.send(AiEvent::Token(content.to_string()));
                            }
                            if let Some(tool_calls) =
                                delta.get("tool_calls").and_then(|t| t.as_array())
                                && let Some(tc) = tool_calls.first()
                            {
                                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                    if !tool_id.is_empty() && tool_id != id {
                                        // Flush previous tool call
                                        if let Ok(args) = serde_json::from_str::<Value>(&tool_args)
                                            && let Some(ev) = dispatch_tool_event(
                                                &tool_id, &tool_name, &args, None,
                                            )
                                        {
                                            let _ = tx.send(ev);
                                        }
                                    }
                                    tool_id = id.to_string();
                                    tool_args.clear();
                                }
                                if let Some(f) = tc.get("function") {
                                    if let Some(n) = f.get("name").and_then(|n| n.as_str())
                                        && !n.is_empty()
                                    {
                                        tool_name = n.to_string();
                                    }
                                    if let Some(args) = f.get("arguments").and_then(|a| a.as_str())
                                    {
                                        tool_args.push_str(args);
                                    }
                                }
                            }
                        }
                        if let Some(u) = v.get("usage").and_then(|u| u.as_object()) {
                            usage = parse_openai_usage(u);
                        }
                    }
                }
            }
        }

        // Final flush of any buffered tool call
        if !tool_id.is_empty()
            && let Ok(args) = serde_json::from_str::<Value>(&tool_args)
            && let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args, None)
        {
            let _ = tx.send(ev);
        }

        let _ = tx.send(AiEvent::Done(usage));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_openai_usage;

    #[test]
    fn openai_parses_cached_tokens_from_details() {
        let usage_obj = serde_json::json!({
            "prompt_tokens": 2000,
            "completion_tokens": 500,
            "prompt_tokens_details": {
                "cached_tokens": 800
            }
        })
        .as_object()
        .cloned()
        .unwrap();

        let usage = parse_openai_usage(&usage_obj);

        assert_eq!(usage.input_tokens, 1200); // 2000 - 800
        assert_eq!(usage.cache_read_tokens, 800);
        assert_eq!(usage.cache_write_tokens, 0);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.total(), 2500);
    }

    #[test]
    fn openai_parses_zero_cache_when_details_absent() {
        let usage_obj = serde_json::json!({
            "prompt_tokens": 1000,
            "completion_tokens": 300
        })
        .as_object()
        .cloned()
        .unwrap();

        let usage = parse_openai_usage(&usage_obj);

        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_write_tokens, 0);
    }
}
