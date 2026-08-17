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

/// Accumulator for one streamed tool call. OpenAI streams tool calls as a
/// sequence of fragments keyed by `index`: `id` and `name` arrive on the
/// first fragment, `arguments` concatenate across fragments, and fragments
/// for different calls may interleave when the model calls tools in parallel.
#[derive(Default)]
pub(crate) struct ToolCallAcc {
    pub id: String,
    pub name: String,
    pub args: String,
}

/// Apply one `delta.tool_calls[i]` fragment to the accumulated calls.
/// Keyed by the fragment's `index` field; nonconforming servers that omit
/// `index` start a new call when a fresh `id` appears and otherwise extend
/// the most recent call.
pub(crate) fn apply_tool_call_delta(calls: &mut Vec<ToolCallAcc>, tc: &Value) {
    let frag_id = tc
        .get("id")
        .and_then(|i| i.as_str())
        .filter(|s| !s.is_empty());
    let idx = match tc.get("index").and_then(|i| i.as_u64()) {
        Some(i) => i as usize,
        None => match (frag_id, calls.last()) {
            (Some(id), Some(last)) if last.id == id => calls.len() - 1,
            (Some(_), _) => calls.len(),
            (None, Some(_)) => calls.len() - 1,
            (None, None) => 0,
        },
    };
    while calls.len() <= idx {
        calls.push(ToolCallAcc::default());
    }
    let acc = &mut calls[idx];
    if let Some(id) = frag_id {
        acc.id = id.to_string();
    }
    if let Some(f) = tc.get("function") {
        if let Some(n) = f.get("name").and_then(|n| n.as_str())
            && !n.is_empty()
        {
            acc.name.push_str(n);
        }
        if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) {
            acc.args.push_str(a);
        }
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
            "stream": true,
            "stream_options": { "include_usage": true },
            "messages": full_messages,
        });
        // api.openai.com rejects the legacy `max_tokens` on reasoning models
        // and expects `max_completion_tokens`; local OpenAI-compatible servers
        // (Ollama, LM Studio, vLLM) predate the new name, so pick by host.
        let max_tokens_key = if self.base_url.starts_with("https://api.openai.com") {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[max_tokens_key] = json!(4096);
        if use_tools {
            body["tools"] = json!(get_openai_tool_definition(&loaded_tools));
        }
        // With no `tools` in the body the model cannot call any; sending
        // `tool_choice` without `tools` is a 400 on api.openai.com, so the
        // use_tools=false case simply omits both.

        let response = send_with_retry(|| {
            http()
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
        })
        .await?;

        let mut stream = response.bytes_stream();
        let mut calls: Vec<ToolCallAcc> = Vec::new();
        let mut leftover = String::new();
        let mut usage = TokenBreakdown::default();

        /// Maximum size of the SSE leftover buffer (1 MiB). A misbehaving
        /// proxy that sends data without newlines would otherwise grow it
        /// without bound.
        const MAX_LEFTOVER_BYTES: usize = 1 << 20;

        'outer: while let Some(chunk) = stream.next().await {
            let bytes = crate::ai::stream_chunk(chunk)?;
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

                // SSE permits `data:` with no space after the colon; some
                // OpenAI-compatible servers emit that form.
                if let Some(rest) = line.strip_prefix("data:") {
                    let data = rest.trim_start();
                    if data == "[DONE]" {
                        break 'outer;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        // Providers can report failures as an in-stream error
                        // payload after the 200; dropping it would surface as
                        // a silent empty response.
                        if let Some(err) = v.get("error") {
                            let msg = err
                                .get("message")
                                .and_then(|m| m.as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| err.to_string());
                            anyhow::bail!("AI stream returned an error: {msg}");
                        }
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
                            {
                                for tc in tool_calls {
                                    apply_tool_call_delta(&mut calls, tc);
                                }
                            }
                        }
                        if let Some(reason) = v["choices"]
                            .get(0)
                            .and_then(|c| c["finish_reason"].as_str())
                        {
                            match reason {
                                "length" => log::warn!(
                                    "OpenAI response truncated: finish_reason=length \
                                     (max_tokens reached) for model {}",
                                    self.model
                                ),
                                "content_filter" => log::warn!(
                                    "OpenAI response stopped by content filter for model {}",
                                    self.model
                                ),
                                _ => {}
                            }
                        }
                        if let Some(u) = v.get("usage").and_then(|u| u.as_object()) {
                            usage = parse_openai_usage(u);
                        }
                    }
                }
            }
        }

        // Dispatch all accumulated tool calls in emission order. Deltas only
        // ever append to a call's fragments, so nothing is complete until the
        // stream ends and flushing here loses no mid-stream information.
        for acc in calls {
            if acc.name.is_empty() {
                continue;
            }
            let id = if acc.id.is_empty() {
                // Compat servers may omit ids; results still need one to correlate.
                crate::ai::next_tool_id()
            } else {
                acc.id
            };
            // A tool that takes no arguments may arrive with an empty string
            // rather than "{}" from some compat servers.
            let args_str = if acc.args.trim().is_empty() {
                "{}"
            } else {
                acc.args.as_str()
            };
            match serde_json::from_str::<Value>(args_str) {
                Ok(args) => {
                    if let Some(ev) = dispatch_tool_event(&id, &acc.name, &args, None) {
                        let _ = tx.send(ev);
                    } else {
                        log::warn!("model called unknown tool '{}' — call dropped", acc.name);
                    }
                }
                Err(e) => {
                    log::warn!(
                        "model emitted malformed JSON arguments for tool '{}': {e}",
                        acc.name
                    );
                    let _ = tx.send(AiEvent::Error(format!(
                        "model emitted malformed arguments for tool '{}': {e}",
                        acc.name
                    )));
                }
            }
        }

        let _ = tx.send(AiEvent::Done(usage));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolCallAcc, apply_tool_call_delta, parse_openai_usage};
    use serde_json::json;

    fn apply_all(fragments: &[serde_json::Value]) -> Vec<ToolCallAcc> {
        let mut calls = Vec::new();
        for f in fragments {
            apply_tool_call_delta(&mut calls, f);
        }
        calls
    }

    #[test]
    fn accumulates_fragmented_arguments_for_one_call() {
        let calls = apply_all(&[
            json!({"index": 0, "id": "call_1",
                   "function": {"name": "read_file", "arguments": ""}}),
            json!({"index": 0, "function": {"arguments": "{\"path\":"}}),
            json!({"index": 0, "function": {"arguments": "\"/etc/hosts\"}"}}),
        ]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].args, "{\"path\":\"/etc/hosts\"}");
    }

    #[test]
    fn parallel_tool_calls_accumulate_independently_by_index() {
        let calls = apply_all(&[
            json!({"index": 0, "id": "call_a",
                   "function": {"name": "list_panes", "arguments": "{}"}}),
            json!({"index": 1, "id": "call_b",
                   "function": {"name": "read_pane", "arguments": "{\"pane"}}),
            json!({"index": 1, "function": {"arguments": "_id\":\"%3\"}"}}),
        ]);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].name, "list_panes");
        assert_eq!(calls[1].id, "call_b");
        assert_eq!(calls[1].name, "read_pane");
        assert_eq!(calls[1].args, "{\"pane_id\":\"%3\"}");
    }

    #[test]
    fn two_entries_in_one_delta_array_both_land() {
        let mut calls = Vec::new();
        for tc in [
            json!({"index": 0, "id": "call_a",
                   "function": {"name": "list_panes", "arguments": "{}"}}),
            json!({"index": 1, "id": "call_b",
                   "function": {"name": "list_scripts", "arguments": "{}"}}),
        ] {
            apply_tool_call_delta(&mut calls, &tc);
        }
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].name, "list_scripts");
    }

    #[test]
    fn missing_index_falls_back_to_id_change_detection() {
        let calls = apply_all(&[
            json!({"id": "call_a", "function": {"name": "t1", "arguments": "{\"a\":"}}),
            json!({"function": {"arguments": "1}"}}),
            json!({"id": "call_b", "function": {"name": "t2", "arguments": "{}"}}),
        ]);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].args, "{\"a\":1}");
        assert_eq!(calls[1].name, "t2");
    }

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
