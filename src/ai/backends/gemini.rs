use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::tools::{dispatch_tool_event, get_gemini_tool_definition};
use crate::ai::types::{AiEvent, Message};
use crate::ai::{AiClient, http, next_tool_id, send_with_retry};

/// Returns `(command, background)` if parsing succeeds, `None` otherwise.
///
/// Gemini thinking models sometimes emit Python-style function call syntax instead of
/// the structured JSON the API expects, e.g.:
///   `print(default_api.run_terminal_command(background = false, command = "ls", target_pane = None))`
///
/// This parser handles any argument order, both quote styles, optional spaces around `=`,
/// and wrapper expressions like `print(default_api.run_terminal_command(...))`.
///
/// Regexes are applied only to the argument list extracted between the parentheses of
/// `run_terminal_command(...)`, preventing model commentary elsewhere in the message
/// from accidentally matching `command = '...'`.
fn parse_malformed_gemini_call(msg: &str) -> Option<(String, bool)> {
    use regex::Regex;
    use std::sync::OnceLock;

    // Find the start of the function call.
    let call_start = msg.find("run_terminal_command(")?;
    let after_open = &msg[call_start + "run_terminal_command(".len()..];

    // Extract only the content inside the outermost parentheses.
    let call_body = {
        let mut depth: usize = 1;
        let mut end = None;
        for (i, ch) in after_open.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        &after_open[..end?]
    };

    // Match: command = "value" or command = 'value', within the argument list only.
    static CMD_RE: OnceLock<Regex> = OnceLock::new();
    let cmd_re = CMD_RE.get_or_init(|| {
        Regex::new(r#"command\s*=\s*["']((?:[^"'\\]|\\.)*)["']"#).expect("valid regex")
    });
    let cmd = cmd_re.captures(call_body)?[1]
        .replace("\\'", "'")
        .replace("\\\"", "\"");

    // Match: background = true|false (optional; defaults to false).
    static BG_RE: OnceLock<Regex> = OnceLock::new();
    let bg_re =
        BG_RE.get_or_init(|| Regex::new(r#"background\s*=\s*(true|false)"#).expect("valid regex"));
    let bg = bg_re
        .captures(call_body)
        .map(|c| &c[1] == "true")
        .unwrap_or(false);

    log::warn!(
        "Gemini MALFORMED_FUNCTION_CALL fallback invoked: cmd={:?} background={}",
        cmd,
        bg
    );
    Some((cmd, bg))
}

/// Google Gemini API backend.
pub struct GeminiClient {
    api_key: String,
    model: String,
}

impl GeminiClient {
    /// Create a new Gemini client.
    pub fn new(api_key: String, model: String) -> Self {
        GeminiClient { api_key, model }
    }

    fn convert_messages(&self, messages: Vec<Message>) -> Vec<Value> {
        let mut result = Vec::new();
        for m in messages {
            if let Some(trs) = m.tool_results {
                // Gemini batches all function responses into one user turn.
                let parts: Vec<Value> = trs
                    .into_iter()
                    .map(|tr| {
                        json!({
                            "functionResponse": {
                                "name": tr.tool_name,
                                "response": {
                                    "name": tr.tool_name,
                                    "content": tr.content
                                }
                            }
                        })
                    })
                    .collect();
                result.push(json!({"role": "user", "parts": parts}));
            } else if let Some(tcs) = m.tool_calls {
                let mut parts = Vec::new();
                if !m.content.is_empty() {
                    parts.push(json!({"text": m.content}));
                }
                for tc in tcs {
                    let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                    let mut fc_part = json!({
                        "functionCall": {
                            "name": tc.name,
                            "args": args
                        }
                    });
                    if let Some(ts) = &tc.thought_signature {
                        fc_part["thoughtSignature"] = json!(ts);
                    }
                    parts.push(fc_part);
                }
                result.push(json!({
                    "role": "model",
                    "parts": parts
                }));
            } else {
                result.push(json!({
                    "role": if m.role == "assistant" { "model" } else { "user" },
                    "parts": [{"text": m.content}]
                }));
            }
        }
        result
    }
}

#[async_trait]
impl AiClient for GeminiClient {
    async fn chat(
        &self,
        system: &str,
        messages: Vec<Message>,
        tx: UnboundedSender<AiEvent>,
        use_tools: bool,
    ) -> Result<()> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.model, self.api_key
        );
        let converted = self.convert_messages(messages);
        let mut body = json!({
            "system_instruction": {"parts": [{"text": system}]},
            "contents": converted,
        });
        if use_tools {
            body["tools"] = json!([{"function_declarations": get_gemini_tool_definition()}]);
        } else {
            // Explicitly disable function calling so the model is forced to
            // respond with plain text (e.g. watchdog analysis calls).
            body["toolConfig"] = json!({
                "functionCallingConfig": {"mode": "NONE"}
            });
        }

        let response = send_with_retry(|| http().post(&url).json(&body)).await?;

        let mut stream = response.bytes_stream();
        let mut leftover = String::new();
        let mut usage = crate::ai::types::AiUsage::default();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            leftover.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = leftover.find('\n') {
                let line = leftover[..pos].trim().to_string();
                leftover = leftover[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array()) {
                            if let Some(candidate) = candidates.get(0) {
                                // Gemini 2.5 Flash (thinking model) sometimes produces a
                                // Python-style function call string instead of a structured
                                // functionCall block.  The API signals this with finishReason
                                // "MALFORMED_FUNCTION_CALL" and a finishMessage containing
                                // the raw call text.  Recover by parsing the finishMessage.
                                if candidate.get("finishReason").and_then(|r| r.as_str())
                                    == Some("MALFORMED_FUNCTION_CALL")
                                {
                                    if let Some(msg) =
                                        candidate.get("finishMessage").and_then(|m| m.as_str())
                                    {
                                        if let Some((cmd, bg)) = parse_malformed_gemini_call(msg) {
                                            let _ = tx.send(AiEvent::ToolCall(
                                                next_tool_id(),
                                                cmd,
                                                bg,
                                                None,
                                                None,
                                                None,
                                            ));
                                        } else {
                                            let _ = tx.send(AiEvent::Error(format!(
                                                "Gemini produced a malformed function call \
                                                 that could not be recovered.\n\
                                                 Raw: {msg}"
                                            )));
                                            return Ok(());
                                        }
                                    }
                                    continue;
                                }

                                if let Some(parts) =
                                    candidate["content"].get("parts").and_then(|p| p.as_array())
                                {
                                    for part in parts {
                                        if let Some(t) =
                                            part.get("text").and_then(|text| text.as_str())
                                        {
                                            if !t.is_empty() {
                                                let _ = tx.send(AiEvent::Token(t.to_string()));
                                            }
                                        }
                                        if let Some(call) = part.get("functionCall") {
                                            let fn_name = call["name"].as_str().unwrap_or("");
                                            if let Some(args) = call.get("args") {
                                                let id = next_tool_id();
                                                let thought_sig = part
                                                    .get("thoughtSignature")
                                                    .and_then(|v| v.as_str())
                                                    .map(String::from);
                                                if let Some(ev) = dispatch_tool_event(
                                                    &id,
                                                    fn_name,
                                                    args,
                                                    thought_sig,
                                                ) {
                                                    let _ = tx.send(ev);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(u) = v.get("usageMetadata").and_then(|m| m.as_object()) {
                            usage.prompt_tokens =
                                u.get("promptTokenCount")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                            usage.completion_tokens =
                                u.get("candidatesTokenCount")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                        }
                    }
                }
            }
        }
        let _ = tx.send(AiEvent::Done(usage));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::backends::anthropic::AnthropicClient;
    use crate::ai::types::ToolCall;
    use crate::ai::{ToolResult, make_client};

    fn user_msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
        }
    }
    fn assistant_msg(content: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
        }
    }
    fn client() -> AnthropicClient {
        AnthropicClient::new("key".to_string(), "model".to_string())
    }

    // ── AnthropicClient::convert_messages ─────────────────────────────────────

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
        };
        let out = client().convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        // Content should be an array with text + tool_use blocks.
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
            content: String::new(), // no prose
            tool_calls: Some(vec![tc]),
            tool_results: None,
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
        // Test AnthropicClient format (tool_result blocks)
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
        };
        let out = client().convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        let content = out[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tc_1");
        assert_eq!(content[0]["content"], "output here");
    }

    #[test]
    fn gemini_convert_tool_results_uses_correct_function_name() {
        use crate::ai::backends::gemini::GeminiClient;
        let tr = ToolResult {
            tool_call_id: "tc_1".to_string(),
            tool_name: "list_schedules".to_string(),
            content: "[]".to_string(),
        };
        let msg = Message {
            role: "user".to_string(),
            content: String::new(),
            tool_calls: None,
            tool_results: Some(vec![tr]),
        };
        let gemini = GeminiClient::new("key".to_string(), "gemini-2.0-flash".to_string());
        let out = gemini.convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        let parts = out[0]["parts"].as_array().expect("parts array");
        assert_eq!(parts[0]["functionResponse"]["name"], "list_schedules");
        assert_eq!(
            parts[0]["functionResponse"]["response"]["name"],
            "list_schedules"
        );
        assert_eq!(parts[0]["functionResponse"]["response"]["content"], "[]");
    }

    // ── make_client dispatch ──────────────────────────────────────────────────

    #[test]
    fn make_client_unknown_defaults_to_anthropic() {
        // We just verify make_client doesn't panic for unknown providers.
        let _c = make_client(
            "unknown_provider",
            "key".to_string(),
            "model".to_string(),
            String::new(),
        );
    }

    #[test]
    fn make_client_openai() {
        let _c = make_client(
            "openai",
            "key".to_string(),
            "gpt-4o".to_string(),
            String::new(),
        );
    }

    #[test]
    fn make_client_gemini() {
        let _c = make_client(
            "gemini",
            "key".to_string(),
            "gemini-2.0-flash".to_string(),
            String::new(),
        );
    }

    #[test]
    fn make_client_ollama() {
        let _c = make_client(
            "ollama",
            "local".to_string(),
            "llama3.2".to_string(),
            "http://localhost:11434/v1".to_string(),
        );
    }

    #[test]
    fn make_client_lmstudio() {
        let _c = make_client(
            "lmstudio",
            "local".to_string(),
            "some-model".to_string(),
            "http://localhost:1234/v1".to_string(),
        );
    }

    // ── Message serialization ─────────────────────────────────────────────────

    #[test]
    fn message_roundtrip_plain() {
        let msg = user_msg("test content");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "user");
        assert_eq!(back.content, "test content");
        assert!(back.tool_calls.is_none());
        assert!(back.tool_results.is_none());
    }

    #[test]
    fn message_tool_calls_skipped_when_none() {
        let msg = user_msg("hi");
        let json = serde_json::to_string(&msg).unwrap();
        // `tool_calls` and `tool_results` should not appear in the JSON at all.
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_results"));
    }

    #[test]
    fn tool_call_roundtrip() {
        let tc = ToolCall {
            id: "tc_99".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"echo hi","background":true}"#.to_string(),
            thought_signature: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "tc_99");
        assert_eq!(back.name, "run_terminal_command");
    }

    // ── parse_malformed_gemini_call ───────────────────────────────────────────

    #[test]
    fn parse_malformed_gemini_call_basic() {
        let msg = "Malformed function call: print(default_api.run_terminal_command(command='cat README.md', background=false))";
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("cat README.md".to_string(), false)));
    }

    #[test]
    fn parse_malformed_gemini_call_background_true() {
        let msg = "Malformed function call: print(default_api.run_terminal_command(command='df -h', background=true))";
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("df -h".to_string(), true)));
    }

    #[test]
    fn parse_malformed_gemini_call_escaped_quote_in_command() {
        let msg = r"Malformed function call: print(default_api.run_terminal_command(command='echo \'hello\'', background=false))";
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("echo 'hello'".to_string(), false)));
    }

    #[test]
    fn parse_malformed_gemini_call_unrecognised_format_returns_none() {
        let msg = "something completely different";
        assert!(parse_malformed_gemini_call(msg).is_none());
    }

    /// Commentary that mentions `command = 'rm -rf /'` but outside a real call must not match.
    #[test]
    fn parse_malformed_gemini_call_rejects_commentary_outside_call() {
        let msg = "the user might try: command = 'rm -rf /'";
        assert!(parse_malformed_gemini_call(msg).is_none());
    }

    /// `command = '...'` in commentary that accompanies a real (but different) call must
    /// not bleed into the extracted command value.
    #[test]
    fn parse_malformed_gemini_call_uses_only_call_body() {
        // The commentary "command = 'danger'" appears outside the parens.
        let msg =
            r#"Note: command = 'danger'. run_terminal_command(command = "ls", background = false)"#;
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("ls".to_string(), false)));
    }

    /// Real failure: args in different order, double-quoted, extra `target_pane = None`.
    #[test]
    fn parse_malformed_gemini_call_double_quotes_reordered_args() {
        let msg = r#"Malformed function call: print(default_api.run_terminal_command(background = false, command = "cat ~/.daemoneye/config.toml", target_pane = None))"#;
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(
            result,
            Some(("cat ~/.daemoneye/config.toml".to_string(), false))
        );
    }

    #[test]
    fn parse_malformed_gemini_call_double_quotes_background_true() {
        let msg = r#"run_terminal_command(command = "df -h", background = true)"#;
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("df -h".to_string(), true)));
    }
}
