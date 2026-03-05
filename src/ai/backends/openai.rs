use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::{AiClient, send_with_retry, http};
use crate::ai::types::{AiEvent, Message};
use crate::ai::tools::{dispatch_tool_event, get_openai_tool_definition};

/// OpenAI-compatible API backend (GPT family, or any OpenAI-compatible endpoint).
/// The `base_url` defaults to the official OpenAI API but can be overridden for
/// local inference servers (e.g. Ollama, vLLM) via the `OPENAI_BASE_URL` env var.
pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiClient {
    /// Create a new OpenAI-compatible client, reading `OPENAI_BASE_URL` for the endpoint.
    pub fn new(api_key: String, model: String) -> Self {
        OpenAiClient {
            api_key,
            model,
            base_url: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
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
    async fn chat(&self, system: &str, messages: Vec<Message>, tx: UnboundedSender<AiEvent>) -> Result<()> {
        let converted = self.convert_messages(messages);
        let mut full_messages = vec![json!({"role": "system", "content": system})];
        full_messages.extend(converted);

        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "stream": true,
            "messages": full_messages,
            "tools": get_openai_tool_definition()
        });

        let response = send_with_retry(|| {
            http()
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
        }).await?;

        let mut stream = response.bytes_stream();
        let mut tool_id = String::new();
        let mut tool_name = String::new();
        let mut tool_args = String::new();
        let mut leftover = String::new();

        'outer: while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            leftover.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = leftover.find('\n') {
                let line = leftover[..pos].trim().to_string();
                leftover = leftover[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break 'outer;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(delta) = v["choices"][0]["delta"].as_object() {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                if !content.is_empty() {
                                    let _ = tx.send(AiEvent::Token(content.to_string()));
                                }
                            }
                            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                                if let Some(tc) = tool_calls.get(0) {
                                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                        if !tool_id.is_empty() && tool_id != id {
                                            // Flush previous tool call
                                            if let Ok(args) = serde_json::from_str::<Value>(&tool_args) {
                                                if let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args, None) {
                                                    let _ = tx.send(ev);
                                                }
                                            }
                                        }
                                        tool_id = id.to_string();
                                        tool_args.clear();
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                                            if !n.is_empty() { tool_name = n.to_string(); }
                                        }
                                        if let Some(args) = f.get("arguments").and_then(|a| a.as_str()) {
                                            tool_args.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Final flush of any buffered tool call
        if !tool_id.is_empty() {
            if let Ok(args) = serde_json::from_str::<Value>(&tool_args) {
                if let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args, None) {
                    let _ = tx.send(ev);
                }
            }
        }

        let _ = tx.send(AiEvent::Done);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

/// Parse a Python-style function call from a Gemini `MALFORMED_FUNCTION_CALL`
/// `finishMessage`, e.g.:
///   `Malformed function call: print(default_api.run_terminal_command(command='cat README.md', background=false))`
///
/// Returns `(command, background)` if parsing succeeds, `None` otherwise.
#[allow(dead_code)]
fn parse_malformed_gemini_call(msg: &str) -> Option<(String, bool)> {
    use regex::Regex;
    use std::sync::OnceLock;
    // Captures the command string (allowing escaped single quotes) and the background bool.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"run_terminal_command\(command='((?:[^'\\]|\\.)*)'\s*,\s*background=(true|false)\)"#)
            .expect("valid regex")
    });
    let caps = re.captures(msg)?;
    let cmd = caps[1].replace("\\'", "'");
    let bg  = &caps[2] == "true";
    Some((cmd, bg))
}

