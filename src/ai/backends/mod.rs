pub mod anthropic;
pub mod gemini;
pub mod openai;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::ai::tools::{
    dispatch_tool_event, invalid_args_event, invalid_args_log_line, malformed_args_event,
};
use crate::ai::types::{AiEvent, TokenBreakdown};

/// Provider-specific SSE JSON frame handler driven by [`drive_sse_stream`].
pub(crate) trait SseFrameHandler {
    /// Human-readable provider name for malformed-frame warning logs.
    fn provider_name(&self) -> &'static str;

    /// Process one parsed JSON `data:` payload from the SSE stream.
    /// Sets `*first_token_seen = true` when the frame carries real output.
    fn on_frame(
        &mut self,
        v: &Value,
        tx: &UnboundedSender<AiEvent>,
        first_token_seen: &mut bool,
        usage: &mut TokenBreakdown,
    ) -> Result<()>;

    /// Called once when the stream closes cleanly, before `AiEvent::Done(usage)`
    /// is sent. Used by providers that accumulate tool-call fragments across
    /// frames (Anthropic, OpenAI) to flush any buffered tool calls.
    fn on_stream_end(&mut self, _tx: &UnboundedSender<AiEvent>) {}
}

/// Dispatch a parsed tool-call `args` JSON [`Value`], emitting either the
/// typed tool event, an `InvalidToolArgs` event for schema mismatches, or a
/// `Notice` when the model called an unknown tool name.
///
/// Returns `true` when a valid tool call event was dispatched (`dispatch_tool_event`
/// returned `Some`), which callers like Gemini use to mark `first_token_seen`.
pub(crate) fn emit_tool_call_value(
    tx: &UnboundedSender<AiEvent>,
    tool_id: &str,
    tool_name: &str,
    args: &Value,
    thought_sig: Option<String>,
) -> bool {
    if let Some(ev) = dispatch_tool_event(tool_id, tool_name, args, thought_sig.clone()) {
        let _ = tx.send(ev);
        true
    } else if let Some(ev) = invalid_args_event(tool_id, tool_name, args, thought_sig) {
        log::warn!("{}", invalid_args_log_line(tool_name, args));
        let _ = tx.send(ev);
        false
    } else {
        log::warn!("model called unknown tool '{tool_name}' — call dropped");
        let _ = tx.send(AiEvent::Notice(format!(
            "the model called unknown tool '{tool_name}' — the call was dropped"
        )));
        false
    }
}

/// Parse a raw JSON string of tool arguments (defaulting empty/whitespace to
/// `"{}"`) and dispatch the tool call via [`emit_tool_call_value`], or surface
/// malformed JSON as an `InvalidToolArgs` event (for known tools) / `Notice`
/// (for unknown tools).
pub(crate) fn emit_tool_call_raw_json(
    tx: &UnboundedSender<AiEvent>,
    tool_id: &str,
    tool_name: &str,
    raw_args: &str,
    thought_sig: Option<String>,
) {
    let args_str = if raw_args.trim().is_empty() {
        "{}"
    } else {
        raw_args
    };
    match serde_json::from_str::<Value>(args_str) {
        Ok(args) => {
            emit_tool_call_value(tx, tool_id, tool_name, &args, thought_sig);
        }
        Err(e) => {
            log::warn!("model emitted malformed JSON arguments for tool '{tool_name}': {e}");
            match malformed_args_event(tool_id, tool_name, raw_args, &e.to_string(), thought_sig) {
                Some(ev) => {
                    let _ = tx.send(ev);
                }
                None => {
                    log::warn!("model called unknown tool '{tool_name}' — malformed call dropped");
                    let _ = tx.send(AiEvent::Notice(format!(
                        "the model called unknown tool '{tool_name}' — the call was dropped"
                    )));
                }
            }
        }
    }
}

/// Drain one HTTP SSE response byte stream through `handler`, enforcing the
/// two-phase (first-token / stream-idle) timeout budgets and reassembling lines
/// with [`crate::ai::SseBuffer`].
async fn drain_sse_response<H: SseFrameHandler>(
    response: reqwest::Response,
    handler: &mut H,
    tx: &UnboundedSender<AiEvent>,
    first_token_seen: &mut bool,
    usage: &mut TokenBreakdown,
    malformed_frames: &mut u32,
) -> Result<()> {
    let mut stream = response.bytes_stream();
    let mut sse = crate::ai::SseBuffer::new();
    let timeouts = crate::ai::stream_timeouts();

    loop {
        let timeout = crate::ai::select_timeout(*first_token_seen, timeouts);
        match crate::ai::stream_next_with_timeout(&mut stream, timeout, *first_token_seen).await {
            Some(Ok(bytes)) => {
                sse.push(&bytes)?;
                while let Some(data) = sse.next_data() {
                    if data == "[DONE]" {
                        return Ok(());
                    }
                    match serde_json::from_str::<Value>(&data) {
                        Ok(v) => handler.on_frame(&v, tx, first_token_seen, usage)?,
                        Err(_e) => {
                            *malformed_frames += 1;
                            if *malformed_frames == 1 {
                                log::warn!(
                                    "malformed SSE frame from {}: {}",
                                    handler.provider_name(),
                                    crate::util::truncate_chars(&data, 120)
                                );
                            }
                        }
                    }
                }
            }
            Some(Err(e)) => return Err(e),
            None => return Ok(()),
        }
    }
}

/// Execute an SSE streaming AI request with pre-first-token transport and
/// stall retries, circuit-breaker accounting, malformed-frame notices, and
/// final `AiEvent::Done(usage)` delivery.
pub(crate) async fn drive_sse_stream<F, M, H>(
    build_request: F,
    mut make_handler: M,
    tx: UnboundedSender<AiEvent>,
) -> Result<()>
where
    F: Fn() -> reqwest::RequestBuilder,
    M: FnMut() -> H,
    H: SseFrameHandler,
{
    const MAX_FIRST_TOKEN_RETRIES: u32 = 2;
    const MAX_STREAM_RETRIES: u32 = 3;
    let mut first_token_seen = false;
    let mut stall_retries: u32 = 0;
    let mut transport_retries: u32 = 0;

    // One breaker outcome per call: the permit is acquired once and held
    // across every internal retry, so its resolution — not the header
    // exchange — is what the breaker records. Retries run under this same
    // permit and never re-enter `send_with_retry`, which would be refused
    // by the probe's own breaker in half-open (bug-03-1 §4).
    let (mut response, mut permit) = crate::ai::send_with_retry(&build_request).await?;

    loop {
        let mut handler = make_handler();
        let mut usage = TokenBreakdown::default();
        let mut malformed_frames: u32 = 0;

        let drain = drain_sse_response(
            response,
            &mut handler,
            &tx,
            &mut first_token_seen,
            &mut usage,
            &mut malformed_frames,
        )
        .await;

        match drain {
            Ok(()) => {
                permit.record_success();
                if malformed_frames > 0 {
                    let _ = tx.send(AiEvent::Notice(format!(
                        "provider sent {malformed_frames} malformed stream frame(s) — the response may be incomplete"
                    )));
                }
                handler.on_stream_end(&tx);
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
                        response = crate::ai::send_under_permit(&build_request, &mut permit)
                            .await
                            .inspect_err(|_| {
                                permit.record_failure();
                            })?;
                        continue;
                    }
                    if stall_retries < MAX_FIRST_TOKEN_RETRIES {
                        stall_retries += 1;
                        log::warn!(
                            "AI stream failed before first token (attempt {stall_retries}/{MAX_FIRST_TOKEN_RETRIES}): {e}"
                        );
                        response = crate::ai::send_under_permit(&build_request, &mut permit)
                            .await
                            .inspect_err(|_| {
                                permit.record_failure();
                            })?;
                        continue;
                    }
                }
                permit.record_failure();
                return Err(e);
            }
        }
    }
}
