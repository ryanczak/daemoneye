//! `execute_tool_call`'s arms for the run-log tools: `RunLogTail`, `RunLogSearch`, `GetTurn`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) fn dispatch(call: &PendingCall) -> anyhow::Result<ToolCallOutcome> {
    match call {
        PendingCall::RunLogTail { run_id, limit, .. } => {
            let result = match crate::daemon::run::RunId::parse(run_id) {
                Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
                Ok(id) => crate::daemon::run::run_log_tail(&id, *limit),
            };
            Ok(ToolCallOutcome::Result(result))
        }

        PendingCall::RunLogSearch {
            run_id,
            query,
            limit,
            ..
        } => {
            let result = match crate::daemon::run::RunId::parse(run_id) {
                Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
                Ok(id) => crate::daemon::run::run_log_search(&id, query, *limit),
            };
            Ok(ToolCallOutcome::Result(result))
        }

        PendingCall::GetTurn { run_id, turn, .. } => {
            let result = match crate::daemon::run::RunId::parse(run_id) {
                Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
                Ok(id) => crate::daemon::run::get_turn(&id, *turn),
            };
            Ok(ToolCallOutcome::Result(result))
        }

        other => anyhow::bail!("{} is not one of the run-log tools", other.tool_name()),
    }
}
