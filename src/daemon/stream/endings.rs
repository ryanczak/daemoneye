//! Four of `run_conversation_loop`'s terminal replies: the turn timeout,
//! the backend error, the stream that kept ending early, and the client that
//! disappeared mid tool call. Each finishes the run log; the first three also
//! send their frame. A child module of `src/daemon/stream.rs`.

use super::*;

/// The tool-result content a pending call gets when the chat client
/// disconnected before the call finished. Names what `list_shells` and
/// `read_shell` will find, so the model can recover the output it never
/// received.
fn client_gone_result_content(tool_name: &str) -> String {
    format!(
        "Error: the chat client disconnected while this `{tool_name}` call was \
         still running, so no result ever reached it. The command may still be \
         running — list_shells shows the shells that are live and read_shell \
         reads what one has produced. The conversation was persisted up to \
         this point; resume with `chat --resume`."
    )
}

/// One complete tool-results list for a turn that ended with the client gone:
/// the results gathered so far (in call order), then a synthetic result for
/// every pending call that has none. The assistant message listing the calls
/// is already in the history, so after this every `tool_call` id has exactly
/// one result and `chat --resume` sends a valid history. Pure — no tx, no
/// store, no clock.
fn complete_results_for_pending_calls(
    pending_calls: &[PendingCall],
    mut gathered: Vec<ToolResult>,
) -> Vec<ToolResult> {
    let mut results = Vec::with_capacity(pending_calls.len());
    for call in pending_calls {
        match gathered.iter().position(|r| r.tool_call_id == call.id()) {
            Some(at) => {
                results.push(gathered.remove(at));
            }
            None => results.push(ToolResult {
                tool_call_id: call.id().to_string(),
                tool_name: call.tool_name().to_string(),
                content: client_gone_result_content(call.tool_name()),
            }),
        }
    }
    results
}

/// What one client-gone ending needs about the turn, so `end_client_gone`
/// keeps a short parameter list. `session_id` is `None` for a ghost or ask
/// session, whose history is not persisted.
pub(super) struct ClientGoneTurn<'a> {
    pub session_id: Option<&'a str>,
    pub post_trim_len: usize,
    pub this_turn_count: usize,
    pub tool_name: &'static str,
    pub error: &'a anyhow::Error,
}

/// The loop state the ending reads and mutates.
pub(super) struct ClientGoneState<'m, 's> {
    pub messages: &'m mut Vec<Message>,
    pub pending_calls: &'m [PendingCall],
    pub tool_results: Vec<ToolResult>,
    pub result_char_cap: Option<usize>,
    pub sessions: &'s SessionStore,
}

/// A tool call failed because the client is gone (its transport closed — the
/// failure surfaces as an error out of `execute_tool_call`). There is nobody
/// to send a frame to, so nothing is written; the turn is instead **kept**:
/// the messages so far are persisted with one result for every tool call, and
/// the run log records why it ended. Returns `Ok(())`, as the bare `return
/// Ok(())` it replaces did.
pub(super) async fn end_client_gone<W>(
    tx: &mut W,
    turn: ClientGoneTurn<'_>,
    state: ClientGoneState<'_, '_>,
    run_log: &mut crate::daemon::run::RunLogGuard,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let ClientGoneTurn {
        session_id,
        post_trim_len,
        this_turn_count,
        tool_name,
        error,
    } = turn;
    let ClientGoneState {
        messages,
        pending_calls,
        tool_results,
        result_char_cap,
        sessions,
    } = state;
    log::warn!(
        "tool call to a disconnected client: tool {tool_name} errored for \
         session {}: {error:#}; keeping and persisting the turn",
        session_id.unwrap_or("(ghost)"),
    );

    let results = truncate_tool_results(
        complete_results_for_pending_calls(pending_calls, tool_results),
        result_char_cap,
    );
    debug_assert_eq!(
        results.len(),
        pending_calls.len(),
        "one result per pending call"
    );
    messages.push(Message {
        role: "user".to_string(),
        content: String::new(),
        tool_calls: None,
        tool_results: Some(results),
        turn: Some(this_turn_count),
    });

    // The same persistence the cancel arm does: the store entry, `dirty`, and
    // the archive append for everything past the trim point.
    if let Some(id) = session_id {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(id) {
                entry.finish_turn(messages.clone());
                entry.dirty = true;
            }
        });
        for msg in &messages[post_trim_len..] {
            append_session_message(id, msg);
        }
    }
    let _ = tx;

    run_log.finish(crate::daemon::run::RunState::HardFail(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!(
                "the chat client disconnected while the {tool_name} tool call was \
                 running: {error:#}. The turn was persisted with a result for every \
                 tool call; `chat --resume` continues it."
            )),
            None,
        ),
    ));
    Ok(())
}

/// The turn ran past `[limits] turn_timeout_secs`.
pub(super) async fn end_turn_timeout<W>(
    tx: &mut W,
    config: &Config,
    run_log: &mut crate::daemon::run::RunLogGuard,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    send_response_split(
        tx,
        Response::Error(format!(
            "Turn exceeded [limits] turn_timeout_secs ({}s) — aborting.",
            config.limits.turn_timeout_secs
        )),
    )
    .await?;
    run_log.finish(crate::daemon::run::RunState::BudgetExceeded(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!(
                "the turn exceeded [limits] turn_timeout_secs ({}s) and was aborted",
                config.limits.turn_timeout_secs
            )),
            None,
        ),
    ));
    Ok(())
}

/// The AI backend reported an error.
pub(super) async fn end_backend_error<W>(
    tx: &mut W,
    run_log: &mut crate::daemon::run::RunLogGuard,
    e: String,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    run_log.metrics_mut().record_backend_error();
    send_response_split(tx, Response::Error(e.clone())).await?;
    run_log.finish(crate::daemon::run::RunState::HardFail(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!("the AI backend reported an error: {e}")),
            None,
        ),
    ));
    Ok(())
}

/// The AI event channel closed without `Done` or `Error` more than
/// `MAX_CHANNEL_CLOSED_RETRIES` times.
pub(super) async fn end_stream_closed<W>(
    tx: &mut W,
    run_log: &mut crate::daemon::run::RunLogGuard,
    channel_closed_retries: u32,
    cause: String,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    run_log.metrics_mut().record_backend_error();
    log::error!(
        "AI event channel closed without Done/Error, giving up after \
         {MAX_CHANNEL_CLOSED_RETRIES} retries: {cause}"
    );
    send_response_split(
        tx,
        Response::Error(format!(
            "AI backend ended the stream without completing \
             (after {} attempts): {cause}",
            channel_closed_retries
        )),
    )
    .await?;
    run_log.finish(crate::daemon::run::RunState::HardFail(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!(
                "the AI backend ended the stream without completing \
                 (after {channel_closed_retries} attempts): {cause}"
            )),
            None,
        ),
    ));
    Ok(())
}

#[cfg(test)]
mod client_gone_tests {
    use super::*;

    /// Two pending `run_terminal_command` calls, ids `a` and `b`.
    fn two_calls() -> Vec<PendingCall> {
        vec![
            PendingCall::Foreground {
                id: "a".to_string(),
                thought_signature: None,
                cmd: "sleep 30".to_string(),
                shell: None,
                host: None,
                timeout_secs: Some(60),
                interactive: false,
            },
            PendingCall::Foreground {
                id: "b".to_string(),
                thought_signature: None,
                cmd: "echo hi".to_string(),
                shell: None,
                host: None,
                timeout_secs: Some(60),
                interactive: false,
            },
        ]
    }

    fn gathered_a() -> Vec<ToolResult> {
        vec![ToolResult {
            tool_call_id: "a".to_string(),
            tool_name: "run_terminal_command".to_string(),
            content: "a's own output".to_string(),
        }]
    }

    #[test]
    fn client_gone_persists_the_turn_with_a_result_for_every_call() {
        let results = complete_results_for_pending_calls(&two_calls(), gathered_a());
        assert_eq!(results.len(), 2, "one result per pending call");
        assert_eq!(results[0].tool_call_id, "a", "call order preserved");
        assert_eq!(
            results[0].content, "a's own output",
            "a's gathered result is unchanged"
        );
        assert_eq!(results[1].tool_call_id, "b");
        assert!(
            results[1].content.contains("disconnected"),
            "b's synthetic result names the disconnect: {}",
            results[1].content
        );
        assert!(
            results[1].content.contains("list_shells"),
            "b's synthetic result names where the output can be found: {}",
            results[1].content
        );
    }

    #[test]
    fn with_no_gathered_results_every_call_gets_a_synthetic_one() {
        let results = complete_results_for_pending_calls(&two_calls(), Vec::new());
        assert_eq!(results.len(), 2, "one result per pending call");
        for (call, result) in two_calls().iter().zip(results.iter()) {
            assert_eq!(result.tool_call_id, call.id(), "call order preserved");
            assert!(
                result.content.contains("disconnected"),
                "every call without a result gets the disconnect text: {}",
                result.content
            );
        }
    }
}
