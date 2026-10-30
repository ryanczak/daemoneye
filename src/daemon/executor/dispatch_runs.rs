//! `execute_tool_call`'s arms for the run-log tools: `RunLogTail`, `RunLogSearch`, `GetTurn`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) fn dispatch(call: &PendingCall) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::run::{get_turn, run_log_search, run_log_tail};
    match call {
        PendingCall::RunLogTail { run_id, limit, .. } => {
            with_run_id(run_id, |id| run_log_tail(id, *limit))
        }
        PendingCall::RunLogSearch {
            run_id,
            query,
            limit,
            ..
        } => with_run_id(run_id, |id| run_log_search(id, query, *limit)),
        PendingCall::GetTurn { run_id, turn, .. } => with_run_id(run_id, |id| get_turn(id, *turn)),

        other => anyhow::bail!("{} is not one of the run-log tools", other.tool_name()),
    }
}

/// Parse the run id a run-log tool was given and run `read` on it. An id
/// that does not parse gets the run-log tools' shared rejection instead.
fn with_run_id(
    run_id: &str,
    read: impl FnOnce(&crate::daemon::run::RunId) -> String,
) -> anyhow::Result<ToolCallOutcome> {
    let result = match crate::daemon::run::RunId::parse(run_id) {
        Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
        Ok(id) => read(&id),
    };
    Ok(ToolCallOutcome::Result(result))
}
