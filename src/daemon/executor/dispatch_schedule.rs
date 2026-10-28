//! `execute_tool_call`'s arms for the schedule tools: `ScheduleCommand`, `ListSchedules`, `CancelSchedule`, `DeleteSchedule`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    session_id: Option<&str>,
    is_ghost: bool,
    schedule_store: &Arc<ScheduleStore>,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    match call {
        PendingCall::ScheduleCommand {
            id: call_id,
            name,
            command,
            is_script,
            run_at,
            interval,
            runbook,
            ghost_runbook,
            cron,
            ..
        } => {
            schedule::run_schedule_command(
                schedule::ScheduleArgs {
                    call_id,
                    name,
                    command,
                    is_script: *is_script,
                    run_at: run_at.as_deref(),
                    interval: interval.as_deref(),
                    runbook: runbook.as_deref(),
                    ghost_runbook: ghost_runbook.as_deref(),
                    cron: cron.as_deref(),
                },
                session_id,
                is_ghost,
                schedule_store,
                tx,
                rx,
            )
            .await
        }

        PendingCall::ListSchedules { .. } => schedule::list_schedules(schedule_store, tx).await,

        PendingCall::CancelSchedule { job_id, .. } => Ok(ToolCallOutcome::Result(
            schedule::cancel_schedule(schedule_store, job_id, session_id),
        )),

        PendingCall::DeleteSchedule { job_id, .. } => Ok(ToolCallOutcome::Result(
            schedule::delete_schedule(schedule_store, job_id, session_id),
        )),

        other => anyhow::bail!("{} is not one of the schedule tools", other.tool_name()),
    }
}
