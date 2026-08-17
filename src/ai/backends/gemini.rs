use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::tools::{dispatch_tool_event, get_gemini_tool_definition};
use crate::ai::types::{AiEvent, Message, TokenBreakdown};
use crate::ai::{AiClient, http, next_tool_id, send_with_retry};

/// Whether a Gemini SSE frame counts as real output (a non-empty text part or a
/// `functionCall` part) for first-token purposes. A frame carrying only
/// `finishReason`/`usageMetadata` — or a `functionCall` naming an unknown tool,
/// which is dropped — does not count.
fn part_counts_as_token(v: &serde_json::Value) -> bool {
    let Some(candidate) = v
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
    else {
        return false;
    };
    let Some(parts) = candidate["content"].get("parts").and_then(|p| p.as_array()) else {
        return false;
    };
    for part in parts {
        if let Some(t) = part.get("text").and_then(|text| text.as_str())
            && !t.is_empty()
        {
            return true;
        }
        if let Some(call) = part.get("functionCall") {
            let fn_name = call["name"].as_str().unwrap_or("");
            // Same `args` default as the drain loop below: Gemini omits `args`
            // entirely for calls that take no arguments, and serde rejects a
            // bare `null` for a struct even when every field is optional. Using
            // `&call["args"]` here would make this predicate answer `false` for
            // a no-argument call the drain loop goes on to emit.
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            if dispatch_tool_event("", fn_name, &args, None).is_some() {
                return true;
            }
        }
    }
    false
}

/// Parse the `usageMetadata` object from a Gemini SSE event into a `TokenBreakdown`.
/// Reads `promptTokenCount`, `candidatesTokenCount`, and `cachedContentTokenCount`;
/// subtracts cached tokens from the total prompt to yield uncached `input_tokens`.
pub(crate) fn parse_gemini_usage(u: &serde_json::Map<String, Value>) -> TokenBreakdown {
    let total_prompt = u
        .get("promptTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let output_tokens = u
        .get("candidatesTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_read = u
        .get("cachedContentTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    TokenBreakdown {
        input_tokens: total_prompt.saturating_sub(cache_read),
        output_tokens,
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
    }
}

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
        // INVARIANT: literal is a valid regex
        Regex::new(r#"command\s*=\s*["']((?:[^"'\\]|\\.)*)["']"#).expect("valid regex")
    });
    let cmd = cmd_re.captures(call_body)?[1]
        .replace("\\'", "'")
        .replace("\\\"", "\"");

    // Match: background = true|false (optional; defaults to false).
    static BG_RE: OnceLock<Regex> = OnceLock::new();
    let bg_re = BG_RE.get_or_init(|| {
        // INVARIANT: literal is a valid regex
        Regex::new(r#"background\s*=\s*(true|false)"#).expect("valid regex")
    });
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
    max_tokens: u32,
}

impl GeminiClient {
    /// Create a new Gemini client.
    pub fn new(api_key: String, model: String, max_tokens: u32) -> Self {
        GeminiClient {
            api_key,
            model,
            max_tokens,
        }
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
        loaded_tools: Vec<String>,
    ) -> Result<()> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.model, self.api_key
        );
        let converted = self.convert_messages(messages);
        let mut body = json!({
            "system_instruction": {"parts": [{"text": system}]},
            "contents": converted,
            "generationConfig": {"maxOutputTokens": self.max_tokens},
        });
        if use_tools {
            body["tools"] =
                json!([{"function_declarations": get_gemini_tool_definition(&loaded_tools)}]);
        } else {
            // Explicitly disable function calling so the model is forced to
            // respond with plain text (e.g. watchdog analysis calls).
            body["toolConfig"] = json!({
                "functionCallingConfig": {"mode": "NONE"}
            });
        }

        const MAX_FIRST_TOKEN_RETRIES: u32 = 2;
        const MAX_STREAM_RETRIES: u32 = 3;
        let mut first_token_seen = false;
        let mut stall_retries: u32 = 0;
        let mut transport_retries: u32 = 0;

        'attempt: loop {
            let response = send_with_retry(|| http().post(&url).json(&body)).await?;

            let mut stream = response.bytes_stream();
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
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                // Gemini reports failures as an in-stream error payload
                                // after the 200; dropping it would surface as a silent
                                // empty response.
                                if let Some(err) = v.get("error") {
                                    let msg = err
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .map(str::to_string)
                                        .unwrap_or_else(|| err.to_string());
                                    break 'drain Err(anyhow::anyhow!(
                                        "AI stream returned an error: {msg}"
                                    ));
                                }
                                if let Some(candidates) =
                                    v.get("candidates").and_then(|c| c.as_array())
                                    && let Some(candidate) = candidates.first()
                                {
                                    if part_counts_as_token(&v) && !first_token_seen {
                                        first_token_seen = true;
                                    }
                                    match candidate.get("finishReason").and_then(|r| r.as_str()) {
                                        Some("MAX_TOKENS") => log::warn!(
                                            "Gemini response truncated: finishReason=MAX_TOKENS \
                                 for model {}",
                                            self.model
                                        ),
                                        Some("SAFETY") => log::warn!(
                                            "Gemini response stopped by safety filter for model {}",
                                            self.model
                                        ),
                                        _ => {}
                                    }
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
                                            if let Some((cmd, bg)) =
                                                parse_malformed_gemini_call(msg)
                                            {
                                                if !first_token_seen {
                                                    first_token_seen = true;
                                                }
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
                                                break 'drain Err(anyhow::anyhow!(
                                                    "unrecoverable malformed function call"
                                                ));
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
                                                && !t.is_empty()
                                            {
                                                if !first_token_seen {
                                                    first_token_seen = true;
                                                }
                                                let _ = tx.send(AiEvent::Token(t.to_string()));
                                            }
                                            if let Some(call) = part.get("functionCall") {
                                                let fn_name = call["name"].as_str().unwrap_or("");
                                                // Gemini omits `args` entirely for calls
                                                // that take no arguments.
                                                let args = call
                                                    .get("args")
                                                    .cloned()
                                                    .unwrap_or_else(|| json!({}));
                                                let id = next_tool_id();
                                                let thought_sig = part
                                                    .get("thoughtSignature")
                                                    .and_then(|v| v.as_str())
                                                    .map(String::from);
                                                if let Some(ev) = dispatch_tool_event(
                                                    &id,
                                                    fn_name,
                                                    &args,
                                                    thought_sig,
                                                ) {
                                                    if !first_token_seen {
                                                        first_token_seen = true;
                                                    }
                                                    let _ = tx.send(ev);
                                                } else {
                                                    log::warn!(
                                                        "model called unknown tool '{fn_name}' — \
                                             call dropped"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                if let Some(u) = v.get("usageMetadata").and_then(|m| m.as_object())
                                {
                                    usage = parse_gemini_usage(u);
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

    /// Whether a Gemini SSE frame counts as real output (a non-empty text part or a
    /// `functionCall` part) for first-token purposes.
    #[test]
    fn finish_reason_only_frame_is_not_a_token() {
        assert!(!super::part_counts_as_token(&json!({
            "candidates": [{"finishReason": "STOP"}]
        })));
        assert!(super::part_counts_as_token(&json!({
            "candidates": [{"content": {"parts": [{"text": "hi"}]}}]
        })));
        assert!(!super::part_counts_as_token(&json!({
            "candidates": [{"content": {"parts": []}}]
        })));
        assert!(super::part_counts_as_token(&json!({
            "candidates": [{"content": {"parts": [{"functionCall": {"name": "list_panes"}}]}}]
        })));
        assert!(!super::part_counts_as_token(&json!({
            "candidates": [{"content": {"parts": [{"functionCall": {"name": "not_a_tool"}}]}}]
        })));
        // A known tool called with no `args` at all — the form Gemini actually
        // sends for zero-argument tools. This is the case that separates the
        // predicate's `args` handling from the drain loop's: passing the bare
        // `null` here answers `false` while the drain loop emits the call.
        assert!(super::part_counts_as_token(&json!({
            "candidates": [{"content": {"parts": [
                {"functionCall": {"name": "get_terminal_context"}}
            ]}}]
        })));
    }

    /// Pins the reason the predicate defaults missing `args` to `{}`:
    /// `dispatch_tool_event` accepts the two forms differently for a tool whose
    /// arguments are all optional, so the predicate and the drain loop must
    /// agree on which form they pass.
    #[test]
    fn dispatch_treats_null_args_and_empty_object_differently() {
        use crate::ai::tools::dispatch_tool_event;

        // All-optional args (`dispatch::<GetTerminalContextArgs>`): serde
        // rejects `null` for the struct but accepts `{}`.
        assert!(dispatch_tool_event("id", "get_terminal_context", &json!(null), None).is_none());
        assert!(dispatch_tool_event("id", "get_terminal_context", &json!({}), None).is_some());

        // Unconditional-Some tools never deserialize args, so both forms agree
        // — which is why a sample drawn only from these cannot detect the above.
        assert!(dispatch_tool_event("id", "list_panes", &json!(null), None).is_some());
        assert!(dispatch_tool_event("id", "list_panes", &json!({}), None).is_some());
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
            turn: None,
        };
        let gemini = GeminiClient::new("key".to_string(), "gemini-2.0-flash".to_string(), 4096);
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

    // ── TokenBreakdown parsing from Gemini usageMetadata ────────────────

    #[test]
    fn gemini_parses_cached_content_token_count() {
        let usage_obj = serde_json::json!({
            "promptTokenCount": 2000,
            "candidatesTokenCount": 500,
            "cachedContentTokenCount": 1200
        })
        .as_object()
        .cloned()
        .unwrap();

        let usage = parse_gemini_usage(&usage_obj);

        assert_eq!(usage.input_tokens, 800); // 2000 - 1200
        assert_eq!(usage.cache_read_tokens, 1200);
        assert_eq!(usage.cache_write_tokens, 0);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.total(), 2500);
    }

    #[test]
    fn gemini_parses_zero_cache_when_field_absent() {
        let usage_obj = serde_json::json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 300
        })
        .as_object()
        .cloned()
        .unwrap();

        let usage = parse_gemini_usage(&usage_obj);

        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_write_tokens, 0);
    }
}
