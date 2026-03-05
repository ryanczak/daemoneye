use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::{AiClient, send_with_retry, http};
use crate::ai::types::{AiEvent, Message};
use crate::ai::tools::{dispatch_tool_event, get_tool_definition};

/// Anthropic API backend (Claude family).
pub struct AnthropicClient {
    api_key: String,
    model: String,
}

impl AnthropicClient {
    /// Create a new Anthropic client for the given model.
    pub fn new(api_key: String, model: String) -> Self {
        AnthropicClient { api_key, model }
    }

    /// Convert the internal `Message` history into Anthropic's JSON format.
    ///
    /// Anthropic's message structure differs from the internal representation:
    /// - Tool results must be batched into a single `role: "user"` message with
    ///   an array of `tool_result` content blocks.
    /// - Tool calls from the assistant must be expressed as `tool_use` content
    ///   blocks alongside any plain text content.
    pub fn convert_messages(&self, messages: Vec<Message>) -> Vec<Value> {
        let mut result = Vec::new();
        for m in messages {
            if let Some(trs) = m.tool_results {
                // Anthropic requires all tool results in a single user message.
                let content: Vec<Value> = trs.into_iter().map(|tr| json!({
                    "type": "tool_result",
                    "tool_use_id": tr.tool_call_id,
                    "content": tr.content
                })).collect();
                result.push(json!({"role": "user", "content": content}));
            } else if let Some(tcs) = m.tool_calls {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(json!({"type": "text", "text": m.content}));
                }
                for tc in tcs {
                    let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                    content.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": args
                    }));
                }
                result.push(json!({
                    "role": "assistant",
                    "content": content
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
impl AiClient for AnthropicClient {
    async fn chat(&self, system: &str, messages: Vec<Message>, tx: UnboundedSender<AiEvent>) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(&self.api_key)?);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let converted = self.convert_messages(messages);

        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "stream": true,
            "system": system,
            "messages": converted,
            "tools": get_tool_definition()
        });

        let response = send_with_retry(|| {
            http()
                .post("https://api.anthropic.com/v1/messages")
                .headers(headers.clone())
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
                        let msg_type = v["type"].as_str().unwrap_or("");

                        if msg_type == "content_block_start" {
                            if v["content_block"]["type"] == "tool_use" {
                                if let Some(id) = v["content_block"]["id"].as_str() {
                                    tool_id = id.to_string();
                                    tool_name = v["content_block"]["name"]
                                        .as_str().unwrap_or("").to_string();
                                    tool_args.clear();
                                }
                            }
                        } else if msg_type == "content_block_delta" {
                            if v["delta"]["type"] == "text_delta" {
                                if let Some(t) = v["delta"]["text"].as_str() {
                                    if !t.is_empty() {
                                        let _ = tx.send(AiEvent::Token(t.to_string()));
                                    }
                                }
                            } else if v["delta"]["type"] == "input_json_delta" {
                                if let Some(partial) = v["delta"]["partial_json"].as_str() {
                                    tool_args.push_str(partial);
                                }
                            }
                        } else if msg_type == "content_block_stop" {
                            if !tool_id.is_empty() {
                                if let Ok(args) = serde_json::from_str::<Value>(&tool_args) {
                                    if let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args, None) {
                                        let _ = tx.send(ev);
                                    }
                                }
                                tool_id.clear();
                                tool_name.clear();
                                tool_args.clear();
                            }
                        }
                    }
                }
            }
        }
        let _ = tx.send(AiEvent::Done);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible
// ---------------------------------------------------------------------------

