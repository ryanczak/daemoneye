use anyhow::Result;
use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::tools::get_tool_definition;
use crate::ai::types::{AiEvent, Message, TokenBreakdown};
use crate::ai::{AiClient, http};

use super::{SseFrameHandler, drive_sse_stream, emit_tool_call_raw_json};

/// Parse the `message_start.message.usage` object from an Anthropic SSE event
/// into a `TokenBreakdown`.  Anthropic's `input_tokens` is the uncached
/// remainder — tokens processed at full price, already excluding
/// `cache_creation_input_tokens` and `cache_read_input_tokens` — so each field
/// is copied across as sent with no subtraction.  Total prompt size is
/// `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
pub(crate) fn parse_anthropic_start_usage(u: &serde_json::Map<String, Value>) -> TokenBreakdown {
    let uncached_input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let cache_write = u
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_read = u
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    TokenBreakdown {
        input_tokens: uncached_input,
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
    emit_tool_call_raw_json(tx, tool_id, tool_name, tool_args, thought_sig);
}

struct AnthropicFrameHandler<'a> {
    model: &'a str,
    tool_id: String,
    tool_name: String,
    tool_args: String,
    in_thinking: bool,
    thinking_text: String,
    thinking_sig: String,
    pending_thought_sig: Option<String>,
}

impl<'a> AnthropicFrameHandler<'a> {
    fn new(model: &'a str) -> Self {
        Self {
            model,
            tool_id: String::new(),
            tool_name: String::new(),
            tool_args: String::new(),
            in_thinking: false,
            thinking_text: String::new(),
            thinking_sig: String::new(),
            pending_thought_sig: None,
        }
    }
}

impl SseFrameHandler for AnthropicFrameHandler<'_> {
    fn provider_name(&self) -> &'static str {
        "Anthropic"
    }

    fn on_frame(
        &mut self,
        v: &Value,
        tx: &UnboundedSender<AiEvent>,
        first_token_seen: &mut bool,
        usage: &mut TokenBreakdown,
    ) -> Result<()> {
        let msg_type = v["type"].as_str().unwrap_or("");
        if !*first_token_seen && is_content_event(msg_type) {
            *first_token_seen = true;
        }

        // Anthropic reports failures (overloaded, invalid request discovered
        // mid-stream) as an in-stream error event after the 200; dropping it
        // would surface as a silent empty response.
        if msg_type == "error" {
            let msg = v["error"]["message"]
                .as_str()
                .unwrap_or("unknown provider error");
            anyhow::bail!("AI stream returned an error: {msg}");
        }

        match msg_type {
            "content_block_start" => {
                if v["content_block"]["type"] == "tool_use" {
                    if let Some(id) = v["content_block"]["id"].as_str() {
                        self.tool_id = id.to_string();
                        self.tool_name = v["content_block"]["name"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        self.tool_args.clear();
                    }
                } else if v["content_block"]["type"] == "thinking" {
                    self.in_thinking = true;
                    self.thinking_text.clear();
                    self.thinking_sig.clear();
                }
            }
            "content_block_delta" => match v["delta"]["type"].as_str() {
                Some("text_delta") => {
                    if let Some(t) = v["delta"]["text"].as_str()
                        && !t.is_empty()
                    {
                        let _ = tx.send(AiEvent::Token(t.to_string()));
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = v["delta"]["partial_json"].as_str() {
                        self.tool_args.push_str(partial);
                    }
                }
                Some("thinking_delta") => {
                    if let Some(t) = v["delta"]["thinking"].as_str() {
                        self.thinking_text.push_str(t);
                    }
                }
                Some("signature_delta") => {
                    if let Some(s) = v["delta"]["signature"].as_str() {
                        self.thinking_sig.push_str(s);
                    }
                }
                _ => {}
            },
            "content_block_stop" => {
                if self.in_thinking {
                    if !self.thinking_sig.is_empty() {
                        self.pending_thought_sig = Some(
                            json!({
                                "thinking": self.thinking_text,
                                "signature": self.thinking_sig
                            })
                            .to_string(),
                        );
                    }
                    self.in_thinking = false;
                } else if !self.tool_id.is_empty() {
                    flush_tool_call(
                        tx,
                        &self.tool_id,
                        &self.tool_name,
                        &self.tool_args,
                        self.pending_thought_sig.take(),
                    );
                    self.tool_id.clear();
                    self.tool_name.clear();
                    self.tool_args.clear();
                }
            }
            "message_start" => {
                if let Some(u) = v["message"]["usage"].as_object() {
                    *usage = parse_anthropic_start_usage(u);
                }
            }
            "message_delta" => {
                match v["delta"]["stop_reason"].as_str() {
                    Some("max_tokens") => {
                        log::warn!(
                            "Anthropic response truncated: stop_reason=max_tokens for model {}",
                            self.model
                        );
                        let _ = tx.send(AiEvent::Notice(
                            "response truncated: the model hit its max_tokens limit — the answer may be incomplete"
                                .to_string(),
                        ));
                    }
                    Some("refusal") => {
                        log::warn!(
                            "Anthropic response stopped: stop_reason=refusal for model {}",
                            self.model
                        );
                        let _ = tx.send(AiEvent::Notice(
                            "the provider stopped this response (refusal)".to_string(),
                        ));
                    }
                    _ => {}
                }
                if let Some(u) = v["usage"].as_object() {
                    apply_anthropic_output_tokens(usage, u);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_stream_end(&mut self, tx: &UnboundedSender<AiEvent>) {
        // Flush any buffered tool call that ended without a content_block_stop
        // event (e.g. network disconnect mid-stream).
        if !self.tool_id.is_empty() {
            flush_tool_call(
                tx,
                &self.tool_id,
                &self.tool_name,
                &self.tool_args,
                self.pending_thought_sig.take(),
            );
            self.tool_id.clear();
            self.tool_name.clear();
            self.tool_args.clear();
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

        drive_sse_stream(
            || {
                http()
                    .post("https://api.anthropic.com/v1/messages")
                    .headers(headers.clone())
                    .json(&body)
            },
            || AnthropicFrameHandler::new(&self.model),
            tx,
        )
        .await
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
        super::flush_tool_call(&tx, "tc_2", "list_shells", "", None);
        match rx.try_recv().expect("an event must be sent") {
            crate::ai::AiEvent::Tool(crate::ai::PendingCall::ListShells { id, .. }) => {
                assert_eq!(id, "tc_2")
            }
            other => panic!("expected ListShells event, got {other:?}"),
        }
    }

    #[test]
    fn flush_unknown_tool_sends_notice() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        super::flush_tool_call(&tx, "tc_3", "not_a_real_tool", "{}", None);
        match rx.try_recv() {
            Ok(crate::ai::AiEvent::Notice(msg)) => assert_eq!(
                msg,
                "the model called unknown tool 'not_a_real_tool' — the call was dropped"
            ),
            other => panic!("expected Notice, got {other:?}"),
        }
    }

    // ── TokenBreakdown parsing from Anthropic SSE events ────────────────

    #[test]
    fn anthropic_input_tokens_are_already_uncached() {
        let usage = serde_json::json!({
            "input_tokens": 50,
            "cache_read_input_tokens": 800,
            "cache_creation_input_tokens": 200
        })
        .as_object()
        .cloned()
        .unwrap();

        let parsed = parse_anthropic_start_usage(&usage);

        assert_eq!(parsed.input_tokens, 50);
        assert_eq!(parsed.cache_read_tokens, 800);
        assert_eq!(parsed.cache_write_tokens, 200);
    }

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

        assert_eq!(usage.input_tokens, 1500);
        assert_eq!(usage.cache_read_tokens, 800);
        assert_eq!(usage.cache_write_tokens, 200);
        assert_eq!(usage.output_tokens, 400);
        assert_eq!(usage.total(), 2900);
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

    #[test]
    fn anthropic_message_start_keeps_uncached_input() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut handler = AnthropicFrameHandler::new("claude-sonnet-4-6");
        let mut first_token_seen = false;
        let mut usage = TokenBreakdown::default();

        handler
            .on_frame(
                &json!({
                    "type": "message_start",
                    "message": {"usage": {
                        "input_tokens": 50,
                        "cache_read_input_tokens": 800,
                        "cache_creation_input_tokens": 200
                    }}
                }),
                &tx,
                &mut first_token_seen,
                &mut usage,
            )
            .unwrap();

        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 800);
        assert_eq!(usage.cache_write_tokens, 200);
    }

    #[test]
    fn frame_handler_streams_tokens_flushes_tool_and_rejects_error_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut handler = AnthropicFrameHandler::new("claude-sonnet-4-6");
        let mut first_token_seen = false;
        let mut usage = TokenBreakdown::default();

        handler
            .on_frame(
                &json!({"type": "message_start", "message": {"usage": {"input_tokens": 100}}}),
                &tx,
                &mut first_token_seen,
                &mut usage,
            )
            .unwrap();
        assert!(!first_token_seen, "message_start does not count as a token");
        assert_eq!(usage.input_tokens, 100);

        handler
            .on_frame(
                &json!({"type": "content_block_delta", "delta": {"type": "text_delta", "text": "hello"}}),
                &tx,
                &mut first_token_seen,
                &mut usage,
            )
            .unwrap();
        assert!(first_token_seen);
        assert!(matches!(rx.try_recv(), Ok(AiEvent::Token(t)) if t == "hello"));

        // Unclosed tool_use block should flush on_stream_end
        handler
            .on_frame(
                &json!({"type": "content_block_start", "content_block": {"type": "tool_use", "id": "tc_9", "name": "list_shells"}}),
                &tx,
                &mut first_token_seen,
                &mut usage,
            )
            .unwrap();
        handler.on_stream_end(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(AiEvent::Tool(crate::ai::PendingCall::ListShells { id, .. })) if id == "tc_9"
        ));

        let err = handler
            .on_frame(
                &json!({"type": "error", "error": {"message": "overloaded"}}),
                &tx,
                &mut first_token_seen,
                &mut usage,
            )
            .unwrap_err();
        assert!(err.to_string().contains("overloaded"));
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible
// ---------------------------------------------------------------------------

// ── M36 phase-11 (B15) — malformed arguments are a tool result ───────

#[test]
fn tool_args_json_anthropic_flush_is_tool_result() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    crate::ai::backends::anthropic::flush_tool_call(
        &tx,
        "tc_1",
        "list_shells",
        "{\"broken\":",
        None,
    );
    let ev = rx.try_recv().expect("an event must be sent");
    let crate::ai::AiEvent::Tool(crate::ai::PendingCall::InvalidArgs {
        id,
        name,
        arguments,
        error,
        ..
    }) = ev
    else {
        panic!("expected InvalidArgs, got {ev:?}");
    };
    assert_eq!(id, "tc_1");
    assert_eq!(name, "list_shells");
    assert_eq!(arguments, "{\"broken\":", "the raw text is kept");
    assert!(
        error.contains("JSON") && error.contains("scope") || error.contains("expected"),
        "the error names the expected parameters: {error}"
    );
    assert!(
        rx.try_recv().is_err(),
        "flush_tool_call sends exactly one event — never an Error"
    );
}
