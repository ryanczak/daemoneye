use anyhow::Result;
use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::tools::{dispatch_tool_event, get_tool_definition};
use crate::ai::types::{AiEvent, Message, TokenBreakdown};
use crate::ai::{AiClient, http, send_with_retry};

/// Parse the `message_start.message.usage` object from an Anthropic SSE event
/// into a `TokenBreakdown`.  Extracts `input_tokens`, `cache_creation_input_tokens`,
/// and `cache_read_input_tokens`; subtracts the cache fields from the total to
/// yield the uncached `input_tokens`.
pub(crate) fn parse_anthropic_start_usage(u: &serde_json::Map<String, Value>) -> TokenBreakdown {
    let total_input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let cache_write = u
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_read = u
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    TokenBreakdown {
        input_tokens: total_input
            .saturating_sub(cache_read)
            .saturating_sub(cache_write),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        output_tokens: 0,
    }
}

/// Apply the `message_delta.usage` object to an existing `TokenBreakdown`,
/// setting `output_tokens`.
pub(crate) fn apply_anthropic_output_tokens(
    usage: &mut TokenBreakdown,
    u: &serde_json::Map<String, Value>,
) {
    usage.output_tokens = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
}

/// Whether an Anthropic SSE `type` counts as real output (text, thinking,
/// or tool-call content) for first-token purposes. `message_start` carries
/// only usage and `ping` events are keepalives — neither counts.
fn is_content_event(msg_type: &str) -> bool {
    matches!(msg_type, "content_block_start" | "content_block_delta")
}

/// Dispatch one completed `tool_use` block, surfacing malformed argument
/// JSON as a visible error instead of silently dropping the call.
fn flush_tool_call(
    tx: &UnboundedSender<AiEvent>,
    tool_id: &str,
    tool_name: &str,
    tool_args: &str,
    thought_sig: Option<String>,
) {
    // A tool that takes no arguments streams no input_json_delta at all.
    let args_str = if tool_args.trim().is_empty() {
        "{}"
    } else {
        tool_args
    };
    match serde_json::from_str::<Value>(args_str) {
        Ok(args) => {
            if let Some(ev) = dispatch_tool_event(tool_id, tool_name, &args, thought_sig) {
                let _ = tx.send(ev);
            } else {
                log::warn!("model called unknown tool '{tool_name}' — call dropped");
            }
        }
        Err(e) => {
            log::warn!("model emitted malformed JSON arguments for tool '{tool_name}': {e}");
            let _ = tx.send(AiEvent::Error(format!(
                "model emitted malformed arguments for tool '{tool_name}': {e}"
            )));
        }
    }
}

/// Anthropic API backend (Claude family).
pub struct AnthropicClient {
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicClient {
    /// Create a new Anthropic client for the given model.
    pub fn new(api_key: String, model: String, max_tokens: u32) -> Self {
        AnthropicClient {
            api_key,
            model,
            max_tokens,
        }
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
                let content: Vec<Value> = trs
                    .into_iter()
                    .map(|tr| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": tr.tool_call_id,
                            "content": tr.content
                        })
                    })
                    .collect();
                result.push(json!({"role": "user", "content": content}));
            } else if let Some(tcs) = m.tool_calls {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(json!({"type": "text", "text": m.content}));
                }
                for tc in tcs {
                    // If this tool call was preceded by an extended-thinking block, echo
                    // the full thinking block back (Anthropic requires it for multi-turn).
                    // The block is stored as JSON: {"thinking": "...", "signature": "..."}.
                    if let Some(ts) = &tc.thought_signature
                        && let Ok(block) = serde_json::from_str::<Value>(ts)
                    {
                        content.push(json!({
                            "type": "thinking",
                            "thinking": block["thinking"],
                            "signature": block["signature"]
                        }));
                    }
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
    async fn chat(
        &self,
        system: &str,
        messages: Vec<Message>,
        tx: UnboundedSender<AiEvent>,
        use_tools: bool,
        loaded_tools: Vec<String>,
    ) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(&self.api_key)?);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let converted = self.convert_messages(messages);

        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "stream": true,
            "system": system,
            "messages": converted,
        });
        if use_tools {
            body["tools"] = json!(get_tool_definition(&loaded_tools));
        }

        const MAX_FIRST_TOKEN_RETRIES: u32 = 2;
        const MAX_STREAM_RETRIES: u32 = 3;
        let mut first_token_seen = false;
        let mut stall_retries: u32 = 0;
        let mut transport_retries: u32 = 0;

        'attempt: loop {
            let response = send_with_retry(|| {
                http()
                    .post("https://api.anthropic.com/v1/messages")
                    .headers(headers.clone())
                    .json(&body)
            })
            .await?;

            let mut stream = response.bytes_stream();
            let mut tool_id = String::new();
            let mut tool_name = String::new();
            let mut tool_args = String::new();
            // Extended thinking support: collect thinking text + signature so they can be
            // echoed back in subsequent turns (Anthropic requires the full thinking block).
            let mut in_thinking = false;
            let mut thinking_text = String::new();
            let mut thinking_sig = String::new();
            // Holds the JSON-encoded thinking block from the most recent thinking content
            // block; passed to the next tool call dispatched in this response.
            let mut pending_thought_sig: Option<String> = None;
            let mut sse = crate::ai::SseBuffer::new();
            let mut usage = TokenBreakdown::default();

            let drain: anyhow::Result<()> = 'drain: loop {
                let timeout =
                    crate::ai::select_timeout(first_token_seen, crate::ai::stream_timeouts());
                match crate::ai::stream_next_with_timeout(&mut stream, timeout, first_token_seen)
                    .await
                {
                    Some(Ok(bytes)) => {
                        if let Err(e) = sse.push(&bytes) {
                            break 'drain Err(e);
                        }
                        while let Some(data) = sse.next_data() {
                            if data == "[DONE]" {
                                break 'drain Ok(());
                            }
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                let msg_type = v["type"].as_str().unwrap_or("");
                                if !first_token_seen && is_content_event(msg_type) {
                                    first_token_seen = true;
                                }

                                // Anthropic reports failures (overloaded, invalid request
                                // discovered mid-stream) as an in-stream error event after
                                // the 200; dropping it would surface as a silent empty
                                // response.
                                if msg_type == "error" {
                                    let msg = v["error"]["message"]
                                        .as_str()
                                        .unwrap_or("unknown provider error");
                                    break 'drain Err(anyhow::anyhow!(
                                        "AI stream returned an error: {msg}"
                                    ));
                                }

                                if msg_type == "content_block_start" {
                                    if v["content_block"]["type"] == "tool_use" {
                                        if let Some(id) = v["content_block"]["id"].as_str() {
                                            tool_id = id.to_string();
                                            tool_name = v["content_block"]["name"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();
                                            tool_args.clear();
                                        }
                                    } else if v["content_block"]["type"] == "thinking" {
                                        in_thinking = true;
                                        thinking_text.clear();
                                        thinking_sig.clear();
                                    }
                                } else if msg_type == "content_block_delta" {
                                    if v["delta"]["type"] == "text_delta" {
                                        if let Some(t) = v["delta"]["text"].as_str()
                                            && !t.is_empty()
                                        {
                                            let _ = tx.send(AiEvent::Token(t.to_string()));
                                        }
                                    } else if v["delta"]["type"] == "input_json_delta" {
                                        if let Some(partial) = v["delta"]["partial_json"].as_str() {
                                            tool_args.push_str(partial);
                                        }
                                    } else if v["delta"]["type"] == "thinking_delta" {
                                        if let Some(t) = v["delta"]["thinking"].as_str() {
                                            thinking_text.push_str(t);
                                        }
                                    } else if v["delta"]["type"] == "signature_delta"
                                        && let Some(s) = v["delta"]["signature"].as_str()
                                    {
                                        thinking_sig.push_str(s);
                                    }
                                } else if msg_type == "content_block_stop" {
                                    if in_thinking {
                                        // Encode both fields so convert_messages can reconstruct
                                        // the full thinking block for the round-trip.
                                        if !thinking_sig.is_empty() {
                                            pending_thought_sig = Some(
                                                json!({
                                                    "thinking": thinking_text,
                                                    "signature": thinking_sig
                                                })
                                                .to_string(),
                                            );
                                        }
                                        in_thinking = false;
                                    } else if !tool_id.is_empty() {
                                        flush_tool_call(
                                            &tx,
                                            &tool_id,
                                            &tool_name,
                                            &tool_args,
                                            pending_thought_sig.take(),
                                        );
                                        tool_id.clear();
                                        tool_name.clear();
                                        tool_args.clear();
                                    }
                                } else if msg_type == "message_start" {
                                    if let Some(u) = v["message"]["usage"].as_object() {
                                        usage = parse_anthropic_start_usage(u);
                                    }
                                } else if msg_type == "message_delta" {
                                    match v["delta"]["stop_reason"].as_str() {
                                        Some("max_tokens") => log::warn!(
                                            "Anthropic response truncated: stop_reason=max_tokens \
                                             for model {}",
                                            self.model
                                        ),
                                        Some("refusal") => log::warn!(
                                            "Anthropic response stopped: stop_reason=refusal \
                                             for model {}",
                                            self.model
                                        ),
                                        _ => {}
                                    }
                                    if let Some(u) = v["usage"].as_object() {
                                        apply_anthropic_output_tokens(&mut usage, u);
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => break 'drain Err(e),
                    None => break 'drain Ok(()),
                }
            };

            match drain {
                Ok(()) => {
                    crate::ai::record_stream_success();
                    // Flush any buffered tool call that ended without a
                    // content_block_stop event (e.g. network disconnect mid-stream).
                    if !tool_id.is_empty() {
                        flush_tool_call(
                            &tx,
                            &tool_id,
                            &tool_name,
                            &tool_args,
                            pending_thought_sig.take(),
                        );
                    }
                    let _ = tx.send(AiEvent::Done(usage));
                    return Ok(());
                }
                Err(e) => {
                    if !first_token_seen {
                        if crate::ai::is_retriable_transport(&e)
                            && transport_retries < MAX_STREAM_RETRIES
                        {
                            transport_retries += 1;
                            log::warn!(
                                "AI stream transport error before first token (attempt {transport_retries}/{MAX_STREAM_RETRIES}): {e}"
                            );
                            tokio::time::sleep(crate::ai::stream_retry_backoff(transport_retries))
                                .await;
                            continue 'attempt;
                        }
                        if stall_retries < MAX_FIRST_TOKEN_RETRIES {
                            stall_retries += 1;
                            log::warn!(
                                "AI stream failed before first token (attempt {stall_retries}/{MAX_FIRST_TOKEN_RETRIES}): {e}"
                            );
                            continue 'attempt;
                        }
                    }
                    crate::ai::record_stream_failure();
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::ToolResult;
    use crate::ai::types::ToolCall;

    fn user_msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }
    fn assistant_msg(content: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }
    fn client() -> AnthropicClient {
        AnthropicClient::new("key".to_string(), "model".to_string(), 4096)
    }

    #[test]
    fn convert_plain_conversation() {
        let msgs = vec![user_msg("hi"), assistant_msg("hello")];
        let out = client().convert_messages(msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
        assert_eq!(out[1]["role"], "assistant");
    }

    #[test]
    fn convert_tool_call_becomes_tool_use_block() {
        let tc = ToolCall {
            id: "tc_1".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"ls","background":true}"#.to_string(),
            thought_signature: None,
        };
        let msg = Message {
            role: "assistant".to_string(),
            content: "running ls".to_string(),
            tool_calls: Some(vec![tc]),
            tool_results: None,
            turn: None,
        };
        let out = client().convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        let content = out[0]["content"].as_array().expect("content array");
        assert!(
            content.iter().any(|b| b["type"] == "tool_use"),
            "no tool_use block"
        );
        assert!(content.iter().any(|b| b["type"] == "text"), "no text block");
    }

    #[test]
    fn convert_tool_call_without_text_omits_text_block() {
        let tc = ToolCall {
            id: "tc_2".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"pwd","background":false}"#.to_string(),
            thought_signature: None,
        };
        let msg = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![tc]),
            tool_results: None,
            turn: None,
        };
        let out = client().convert_messages(vec![msg]);
        let content = out[0]["content"].as_array().expect("content array");
        assert!(
            !content.iter().any(|b| b["type"] == "text"),
            "empty text block should be omitted"
        );
    }

    #[test]
    fn convert_tool_results_become_user_message_with_tool_result_blocks() {
        let tr = ToolResult {
            tool_call_id: "tc_1".to_string(),
            tool_name: "list_schedules".to_string(),
            content: "output here".to_string(),
        };
        let msg = Message {
            role: "user".to_string(),
            content: String::new(),
            tool_calls: None,
            tool_results: Some(vec![tr]),
            turn: None,
        };
        let out = client().convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        let content = out[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tc_1");
        assert_eq!(content[0]["content"], "output here");
    }

    // ── flush_tool_call ──────────────────────────────────────────────────

    #[test]
    fn flush_malformed_args_sends_visible_error() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        super::flush_tool_call(&tx, "tc_1", "list_panes", "{\"broken\":", None);
        match rx.try_recv().expect("an event must be sent") {
            crate::ai::AiEvent::Error(msg) => {
                assert!(msg.contains("list_panes"), "got: {msg}");
            }
            other => panic!("expected Error event, got {other:?}"),
        }
    }

    #[test]
    fn first_token_flips_on_content_block_not_ping() {
        assert!(super::is_content_event("content_block_delta"));
        assert!(super::is_content_event("content_block_start"));
        assert!(!super::is_content_event("ping"));
        assert!(!super::is_content_event("message_start"));
        assert!(!super::is_content_event("message_delta"));
        assert!(!super::is_content_event(""));
    }

    #[test]
    fn flush_empty_args_dispatches_as_empty_object() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        super::flush_tool_call(&tx, "tc_2", "list_panes", "", None);
        match rx.try_recv().expect("an event must be sent") {
            crate::ai::AiEvent::ListPanes { id, .. } => assert_eq!(id, "tc_2"),
            other => panic!("expected ListPanes event, got {other:?}"),
        }
    }

    #[test]
    fn flush_unknown_tool_sends_nothing() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        super::flush_tool_call(&tx, "tc_3", "not_a_real_tool", "{}", None);
        assert!(rx.try_recv().is_err(), "unknown tool must not send events");
    }

    // ── TokenBreakdown parsing from Anthropic SSE events ────────────────

    #[test]
    fn anthropic_parses_cache_creation_and_read_tokens() {
        let message_start_usage = serde_json::json!({
            "input_tokens": 1500,
            "cache_creation_input_tokens": 200,
            "cache_read_input_tokens": 800
        })
        .as_object()
        .cloned()
        .unwrap();

        let message_delta_usage = serde_json::json!({
            "output_tokens": 400
        })
        .as_object()
        .cloned()
        .unwrap();

        let mut usage = parse_anthropic_start_usage(&message_start_usage);
        apply_anthropic_output_tokens(&mut usage, &message_delta_usage);

        assert_eq!(usage.input_tokens, 500); // 1500 - 800 - 200
        assert_eq!(usage.cache_read_tokens, 800);
        assert_eq!(usage.cache_write_tokens, 200);
        assert_eq!(usage.output_tokens, 400);
        assert_eq!(usage.total(), 1900);
    }

    #[test]
    fn anthropic_parses_zero_cache_when_not_present() {
        let message_start_usage = serde_json::json!({
            "input_tokens": 1000
        })
        .as_object()
        .cloned()
        .unwrap();

        let usage = parse_anthropic_start_usage(&message_start_usage);

        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_write_tokens, 0);
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible
// ---------------------------------------------------------------------------
