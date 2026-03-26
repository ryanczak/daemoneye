use super::ToolCallOutcome;
use super::USER_PROMPT_TIMEOUT;
use crate::daemon::utils::log_event;
use crate::daemon::utils::send_response_split;
use crate::ipc::{Request, Response, ScheduleListItem};
use crate::scheduler::{ActionOn, JobStatus, ScheduleKind, ScheduleStore, ScheduledJob};
use std::sync::Arc;

pub(super) struct ScheduleArgs<'a> {
    pub call_id: &'a str,
    pub name: &'a str,
    pub command: &'a str,
    pub is_script: bool,
    pub run_at: Option<&'a str>,
    pub interval: Option<&'a str>,
    pub runbook: Option<&'a str>,
    pub ghost_runbook: Option<&'a str>,
    pub cron: Option<&'a str>,
}

pub(super) async fn run_schedule_command<W, R>(
    args: ScheduleArgs<'_>,
    session_id: Option<&str>,
    is_ghost: bool,
    schedule_store: &Arc<ScheduleStore>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let ScheduleArgs {
        call_id,
        name,
        command,
        is_script,
        run_at,
        interval,
        runbook,
        ghost_runbook,
        cron,
    } = args;
    #[allow(deprecated)]
    let (action, runbook) = if let Some(rb) = ghost_runbook {
        (
            ActionOn::Ghost {
                runbook: rb.to_string(),
            },
            None,
        )
    } else if command.is_empty() && !is_script {
        if let Some(rb) = runbook {
            // AI put the runbook name in the watchdog `runbook` field instead of `ghost_runbook`.
            // Infer ghost mode and clear the runbook field so it isn't misused as a watchdog key.
            log::warn!(
                "schedule_command: ghost_runbook not set but command is empty and runbook='{}'; \
                 inferring ghost mode. Use ghost_runbook for ghost jobs.",
                rb
            );
            (
                ActionOn::Ghost {
                    runbook: rb.to_string(),
                },
                None,
            )
        } else {
            // No ghost_runbook, no command, no script — nothing to run.
            return Ok(ToolCallOutcome::Result(
                "Error: command is empty and ghost_runbook is not set. \
                 To schedule a Ghost Shell job set ghost_runbook to a runbook name. \
                 To schedule a script job set command to a script name and is_script=true."
                    .to_string(),
            ));
        }
    } else if is_script {
        (ActionOn::Script(command.to_string()), runbook)
    } else {
        (ActionOn::Command(command.to_string()), runbook)
    };

    let kind = if let Some(expr) = cron {
        match crate::scheduler::parse_cron(expr) {
            Ok(sched) => match sched.upcoming(chrono::Utc).next() {
                Some(next) => ScheduleKind::Cron {
                    expression: expr.to_string(),
                    next_run: next,
                },
                None => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Cron expression '{}' has no future occurrences.",
                        expr
                    )));
                }
            },
            Err(e) => {
                return Ok(ToolCallOutcome::Result(format!(
                    "Invalid cron expression '{}': {}. \
                 Use 5-field format: minute hour day-of-month month day-of-week. \
                 Example: '*/5 * * * *' (every 5 minutes).",
                    expr, e
                )));
            }
        }
    } else if let Some(iso) = interval {
        let secs = match crate::scheduler::parse_iso_duration(iso) {
            Some(s) => s,
            None => {
                return Ok(ToolCallOutcome::Result(format!(
                    "Invalid interval '{}'. Use ISO 8601 duration format, e.g. PT1M (1 minute), PT5M (5 minutes), PT1H (1 hour), P1D (1 day).",
                    iso
                )));
            }
        };
        let next = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
        ScheduleKind::Every {
            interval_secs: secs,
            next_run: next,
        }
    } else if let Some(at_str) = run_at {
        let at = chrono::DateTime::parse_from_rfc3339(at_str)
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::seconds(60));
        ScheduleKind::Once { at }
    } else {
        ScheduleKind::Once {
            at: chrono::Utc::now() + chrono::Duration::seconds(60),
        }
    };

    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot create scheduled jobs in a Ghost Shell (requires user approval)."
                .to_string(),
        ));
    }

    send_response_split(
        tx,
        Response::ScheduleWritePrompt {
            id: call_id.to_string(),
            name: name.to_string(),
            kind: kind.describe(),
            action: action.describe(),
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ScheduleWriteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        let job = ScheduledJob::new(
            name.to_string(),
            kind.clone(),
            action,
            runbook.map(|s| s.to_string()),
        );
        match schedule_store.add(job) {
            Ok(job_id) => {
                log::info!("Job scheduled: '{}' ({})", name, &job_id[..8]);
                log_event(
                    "job_scheduled",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "job_id": &job_id,
                        "job_name": name,
                        "kind": kind.describe(),
                    }),
                );
                Ok(ToolCallOutcome::Result(format!(
                    "Scheduled job '{}' created (id: {})",
                    name, job_id
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to schedule job: {}",
                e
            ))),
        }
    } else {
        log_event(
            "command_approval",
            serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": "schedule",
                "cmd": command,
                "decision": "denied",
            }),
        );
        Ok(ToolCallOutcome::Result(
            "Job scheduling denied by user".to_string(),
        ))
    }
}

pub(super) async fn list_schedules<W>(
    schedule_store: &Arc<ScheduleStore>,
    tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let jobs = schedule_store.list();
    let items: Vec<ScheduleListItem> = jobs
        .iter()
        .map(|j| ScheduleListItem {
            id: j.id.clone(),
            name: j.name.clone(),
            kind: j.kind.describe(),
            action: j.action.describe(),
            status: j.status.describe(),
            last_run: j
                .last_run
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
            next_run: if matches!(j.status, JobStatus::Pending) {
                j.kind
                    .next_run()
                    .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            } else {
                None
            },
        })
        .collect();
    let count = items.len();
    let _ = send_response_split(
        tx,
        Response::ScheduleList {
            jobs: items.clone(),
        },
    )
    .await;
    if count == 0 {
        Ok(ToolCallOutcome::Result("No scheduled jobs.".to_string()))
    } else {
        let mut lines = format!("{} scheduled job(s):\n", count);
        for item in &items {
            let next = item.next_run.as_deref().unwrap_or("n/a");
            let last = item.last_run.as_deref().unwrap_or("never");
            lines.push_str(&format!(
                "- {} (id: {}): {}, status: {}, next: {}, last: {}\n",
                item.name, item.id, item.kind, item.status, next, last
            ));
        }
        Ok(ToolCallOutcome::Result(lines))
    }
}

pub(super) fn cancel_schedule(
    schedule_store: &Arc<ScheduleStore>,
    job_id: &str,
    session_id: Option<&str>,
) -> String {
    match schedule_store.cancel(job_id) {
        Ok(true) => {
            log::info!("Job canceled: {}", &job_id[..job_id.len().min(8)]);
            log_event(
                "job_canceled",
                serde_json::json!({ "session": session_id.unwrap_or("-"), "job_id": job_id }),
            );
            format!("Job {} cancelled", &job_id[..job_id.len().min(8)])
        }
        Ok(false) => format!("Job {} not found", job_id),
        Err(e) => format!("Failed to cancel job: {}", e),
    }
}

pub(super) fn delete_schedule(
    schedule_store: &Arc<ScheduleStore>,
    job_id: &str,
    session_id: Option<&str>,
) -> String {
    match schedule_store.delete(job_id) {
        Ok(true) => {
            log::info!("Job deleted: {}", &job_id[..job_id.len().min(8)]);
            log_event(
                "job_deleted",
                serde_json::json!({ "session": session_id.unwrap_or("-"), "job_id": job_id }),
            );
            format!("Job {} deleted permanently", &job_id[..job_id.len().min(8)])
        }
        Ok(false) => format!("Job {} not found", job_id),
        Err(e) => format!("Failed to delete job: {}", e),
    }
}
