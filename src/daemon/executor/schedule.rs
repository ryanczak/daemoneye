use super::{ToolCallOutcome, USER_PROMPT_TIMEOUT, reply_approval};
use crate::daemon::utils::log_event;
use crate::daemon::utils::send_response_split;
use crate::ipc::{Response, ScheduleListItem};
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

fn invalid_interval(iso: &str) -> String {
    format!(
        "Invalid interval '{}'. Use ISO 8601 duration format, e.g. PT1M (1 minute), PT5M (5 minutes), PT1H (1 hour), P1D (1 day).",
        iso
    )
}

fn resolve_schedule_action(
    command: &str,
    is_script: bool,
    runbook: Option<&str>,
    ghost_runbook: Option<&str>,
) -> Result<(ActionOn, Option<String>), String> {
    if let Some(rb) = ghost_runbook {
        return Ok((
            ActionOn::Ghost {
                runbook: rb.to_string(),
            },
            None,
        ));
    }
    if command.is_empty() {
        if !is_script && let Some(rb) = runbook {
            // AI put the runbook name in the watchdog `runbook` field instead of `ghost_runbook`.
            // Infer ghost mode and clear the runbook field so it isn't misused as a watchdog key.
            log::warn!(
                "schedule_command: ghost_runbook not set but command is empty and runbook='{}'; \
                 inferring ghost mode. Use ghost_runbook for ghost jobs.",
                rb
            );
            return Ok((
                ActionOn::Ghost {
                    runbook: rb.to_string(),
                },
                None,
            ));
        }
        return Err("Error: command is empty and ghost_runbook is not set. \
             To schedule a Ghost Shell job set ghost_runbook to a runbook name. \
             To schedule a script job set command to a script name and is_script=true."
            .to_string());
    }
    if is_script {
        Ok((
            ActionOn::Script(command.to_string()),
            runbook.map(str::to_string),
        ))
    } else {
        Err(format!(
            "Error: raw command scheduling is not supported (got command={command:?} with is_script=false). \
             Write the script to ~/.daemoneye/scripts/ with write_script and schedule it with is_script=true, \
             or set ghost_runbook to schedule a Ghost Shell job."
        ))
    }
}

fn resolve_schedule_kind(
    cron: Option<&str>,
    interval: Option<&str>,
    run_at: Option<&str>,
) -> Result<ScheduleKind, String> {
    if let Some(expr) = cron {
        match crate::scheduler::parse_cron(expr) {
            Ok(sched) => match sched.upcoming(chrono::Utc).next() {
                Some(next) => Ok(ScheduleKind::Cron {
                    expression: expr.to_string(),
                    next_run: next,
                }),
                None => Err(format!(
                    "Cron expression '{}' has no future occurrences.",
                    expr
                )),
            },
            Err(e) => Err(format!(
                "Invalid cron expression '{}': {}. \
                 Use 5-field format: minute hour day-of-month month day-of-week. \
                 Example: '*/5 * * * *' (every 5 minutes).",
                expr, e
            )),
        }
    } else if let Some(iso) = interval {
        let Some(secs) = crate::scheduler::parse_iso_duration(iso) else {
            return Err(invalid_interval(iso));
        };
        let Some(next) = crate::scheduler::checked_next_run(chrono::Utc::now(), secs) else {
            return Err(invalid_interval(iso));
        };
        Ok(ScheduleKind::Every {
            interval_secs: secs,
            next_run: next,
        })
    } else if let Some(at_str) = run_at {
        let dt = chrono::DateTime::parse_from_rfc3339(at_str).map_err(|e| {
            format!(
                "Invalid run_at '{}': {}. Use ISO 8601 / RFC 3339 UTC datetime format, e.g. '2026-03-01T15:00:00Z'.",
                at_str, e
            )
        })?;
        Ok(ScheduleKind::Once {
            at: dt.with_timezone(&chrono::Utc),
        })
    } else {
        Ok(ScheduleKind::Once {
            at: chrono::Utc::now() + chrono::Duration::seconds(60),
        })
    }
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
    R: tokio::io::AsyncBufRead + Unpin,
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
    let (action, runbook) =
        match resolve_schedule_action(command, is_script, runbook, ghost_runbook) {
            Ok(pair) => pair,
            Err(msg) => return Ok(ToolCallOutcome::Result(msg)),
        };
    let kind = match resolve_schedule_kind(cron, interval, run_at) {
        Ok(k) => k,
        Err(msg) => return Ok(ToolCallOutcome::Result(msg)),
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

    let mut line = Vec::new();
    let read_result = tokio::time::timeout(
        USER_PROMPT_TIMEOUT,
        crate::ipc::read_bounded_line(rx, &mut line, crate::ipc::MAX_IPC_MESSAGE_BYTES),
    )
    .await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => reply_approval(call_id, &line, "schedule write"),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(anyhow::anyhow!(
                "schedule reply line too large ({e}); refusing the prompt and ending the turn"
            ));
        }
        _ => false,
    };

    if approved {
        let job = ScheduledJob::new(name.to_string(), kind.clone(), action, runbook);
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

pub(super) async fn cancel_schedule<W, R>(
    call_id: &str,
    job_id: &str,
    is_ghost: bool,
    schedule_store: &Arc<ScheduleStore>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufRead + Unpin,
{
    remove_schedule(
        call_id,
        job_id,
        is_ghost,
        schedule_store,
        session_id,
        false,
        tx,
        rx,
    )
    .await
}

pub(super) async fn delete_schedule<W, R>(
    call_id: &str,
    job_id: &str,
    is_ghost: bool,
    schedule_store: &Arc<ScheduleStore>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufRead + Unpin,
{
    remove_schedule(
        call_id,
        job_id,
        is_ghost,
        schedule_store,
        session_id,
        true,
        tx,
        rx,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn remove_schedule<W, R>(
    call_id: &str,
    job_id: &str,
    is_ghost: bool,
    schedule_store: &Arc<ScheduleStore>,
    session_id: Option<&str>,
    delete: bool,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufRead + Unpin,
{
    let verb = if delete { "delete" } else { "cancel" };
    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot cancel or delete scheduled jobs in a Ghost Shell (requires user approval)."
                .to_string(),
        ));
    }
    let Some(job) = schedule_store.list().into_iter().find(|j| j.id == job_id) else {
        return Ok(ToolCallOutcome::Result(format!("Job {} not found", job_id)));
    };
    let (name, kind, action) = (job.name.clone(), job.kind.describe(), job.action.describe());
    send_response_split(
        tx,
        Response::ScheduleRemovePrompt {
            id: call_id.to_string(),
            job_id: job_id.to_string(),
            name,
            kind,
            action,
            delete,
        },
    )
    .await?;

    let mut line = Vec::new();
    let read_result = tokio::time::timeout(
        USER_PROMPT_TIMEOUT,
        crate::ipc::read_bounded_line(rx, &mut line, crate::ipc::MAX_IPC_MESSAGE_BYTES),
    )
    .await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => reply_approval(call_id, &line, "schedule removal"),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(anyhow::anyhow!(
                "schedule removal reply line too large ({e}); refusing the prompt and ending the turn"
            ));
        }
        _ => false,
    };

    if !approved {
        log_event(
            "command_approval",
            serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": if delete { "schedule_delete" } else { "schedule_cancel" },
                "job_id": job_id,
                "decision": "denied",
            }),
        );
        return Ok(ToolCallOutcome::Result(
            "Job removal denied by user".to_string(),
        ));
    }

    let result = if delete {
        schedule_store.delete(job_id)
    } else {
        schedule_store.cancel(job_id)
    };
    match result {
        Ok(true) => {
            let short = &job_id[..job_id.len().min(8)];
            if delete {
                log::info!("Job deleted: {}", short);
                log_event(
                    "job_deleted",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "job_id": job_id }),
                );
            } else {
                log::info!("Job canceled: {}", short);
                log_event(
                    "job_canceled",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "job_id": job_id }),
                );
            }
            Ok(ToolCallOutcome::Result(if delete {
                format!("Job {} deleted permanently", short)
            } else {
                format!("Job {} cancelled", short)
            }))
        }
        Ok(false) => Ok(ToolCallOutcome::Result(format!("Job {} not found", job_id))),
        Err(e) => Ok(ToolCallOutcome::Result(format!(
            "Failed to {} job: {}",
            verb, e
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Request;

    #[test]
    fn resolve_schedule_action_supports_script_and_ghost_modes() {
        let (action, rb) = resolve_schedule_action("backup.sh", true, Some("disk-check"), None)
            .expect("script mode with watchdog runbook succeeds");
        assert!(matches!(action, ActionOn::Script(ref s) if s == "backup.sh"));
        assert_eq!(rb.as_deref(), Some("disk-check"));

        let (action, rb) = resolve_schedule_action("", false, None, Some("disk-alert"))
            .expect("explicit ghost_runbook mode succeeds");
        assert!(matches!(action, ActionOn::Ghost { ref runbook } if runbook == "disk-alert"));
        assert_eq!(rb, None);

        let (action, rb) = resolve_schedule_action("", false, Some("inferred-rb"), None)
            .expect("empty command with runbook infers ghost mode");
        assert!(matches!(action, ActionOn::Ghost { ref runbook } if runbook == "inferred-rb"));
        assert_eq!(rb, None);
    }

    #[test]
    fn resolve_schedule_action_refuses_raw_commands_and_empty_args() {
        let err = resolve_schedule_action("echo hi", false, None, None)
            .expect_err("raw command with is_script=false must be refused");
        assert!(
            err.contains("raw command scheduling is not supported")
                && err.contains("is_script=true"),
            "error guides the model to write_script + is_script=true: {err}"
        );

        let empty_err = resolve_schedule_action("", false, None, None)
            .expect_err("empty command without runbook must be refused");
        assert!(
            empty_err.contains("command is empty and ghost_runbook is not set"),
            "empty error names both missing parameters: {empty_err}"
        );
    }

    #[test]
    fn resolve_schedule_kind_validates_cron_interval_and_run_at() {
        let cron = resolve_schedule_kind(Some("*/5 * * * *"), None, None).expect("valid cron");
        assert!(matches!(cron, ScheduleKind::Cron { .. }));

        let bad_cron =
            resolve_schedule_kind(Some("not a cron"), None, None).expect_err("invalid cron");
        assert!(bad_cron.contains("Invalid cron expression"));

        let every = resolve_schedule_kind(None, Some("PT5M"), None).expect("valid ISO interval");
        assert!(matches!(
            every,
            ScheduleKind::Every {
                interval_secs: 300,
                ..
            }
        ));

        let bad_interval =
            resolve_schedule_kind(None, Some("5m"), None).expect_err("invalid interval");
        assert!(bad_interval.contains("Invalid interval '5m'"));

        let once = resolve_schedule_kind(None, None, Some("2026-03-01T15:00:00Z"))
            .expect("valid RFC 3339 run_at");
        assert!(matches!(once, ScheduleKind::Once { .. }));

        let bad_run_at = resolve_schedule_kind(None, None, Some("tomorrow at 3pm"))
            .expect_err("malformed run_at must be rejected instead of falling back to +60s");
        assert!(
            bad_run_at.contains("Invalid run_at 'tomorrow at 3pm'")
                && bad_run_at.contains("RFC 3339"),
            "error guides the model to RFC 3339 format: {bad_run_at}"
        );
    }

    #[tokio::test]
    async fn run_schedule_command_refuses_invalid_calls_before_prompting() {
        let store = Arc::new(ScheduleStore::new_empty());
        let mut out = Vec::new();
        let mut empty_rx = tokio::io::BufReader::new(tokio::io::empty());

        // 1. Raw command (is_script=false) refused without writing a prompt.
        let outcome = run_schedule_command(
            ScheduleArgs {
                call_id: "c1",
                name: "raw-cmd",
                command: "rm -rf /tmp/foo",
                is_script: false,
                run_at: None,
                interval: Some("PT5M"),
                runbook: None,
                ghost_runbook: None,
                cron: None,
            },
            Some("sess-1"),
            false,
            &store,
            &mut out,
            &mut empty_rx,
        )
        .await
        .expect("refusal returns Ok(ToolCallOutcome)");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("raw command scheduling is not supported"));
        assert!(out.is_empty(), "no prompt sent on invalid action");
        assert!(store.list().is_empty(), "no job added to store");

        // 2. Malformed run_at refused without writing a prompt.
        let outcome = run_schedule_command(
            ScheduleArgs {
                call_id: "c2",
                name: "bad-time",
                command: "check.sh",
                is_script: true,
                run_at: Some("2026-03-01 15:00"),
                interval: None,
                runbook: None,
                ghost_runbook: None,
                cron: None,
            },
            Some("sess-1"),
            false,
            &store,
            &mut out,
            &mut empty_rx,
        )
        .await
        .expect("refusal returns Ok(ToolCallOutcome)");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("Invalid run_at '2026-03-01 15:00'"));
        assert!(out.is_empty(), "no prompt sent on invalid run_at");
        assert!(store.list().is_empty(), "no job added to store");
    }

    fn canned_reader(reply: &Request) -> tokio::io::BufReader<std::io::Cursor<Vec<u8>>> {
        tokio::io::BufReader::new(std::io::Cursor::new(
            format!("{}\n", serde_json::to_string(reply).unwrap()).into_bytes(),
        ))
    }

    fn parse_first_frame(out: &[u8]) -> Response {
        let end = out.iter().position(|&b| b == b'\n').expect("a frame");
        serde_json::from_slice(&out[..end]).expect("valid response frame")
    }

    fn test_store() -> (Arc<ScheduleStore>, String) {
        let store = Arc::new(ScheduleStore::new_empty());
        let job_id = store
            .add(ScheduledJob::new(
                "nightly".to_string(),
                ScheduleKind::Once {
                    at: chrono::Utc::now(),
                },
                ActionOn::Alert,
                None,
            ))
            .unwrap();
        (store, job_id)
    }

    #[tokio::test]
    async fn delete_schedule_from_chat_asks_first() {
        let _guard = crate::test_home();
        // Denied leg.
        let (store, job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut rx = canned_reader(&Request::ScheduleRemoveResponse {
            id: "c1".to_string(),
            approved: false,
        });
        let outcome = delete_schedule(
            "c1",
            &job_id,
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut rx,
        )
        .await
        .expect("denial returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("denied"), "denied text: {msg}");
        match parse_first_frame(&out) {
            Response::ScheduleRemovePrompt { name, delete, .. } => {
                assert_eq!(name, "nightly");
                assert!(delete);
            }
            other => panic!("expected ScheduleRemovePrompt, got {:?}", other.kind()),
        }
        assert_eq!(store.list().len(), 1, "job still listed after denial");

        // Approved leg.
        let (store, job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut rx = canned_reader(&Request::ScheduleRemoveResponse {
            id: "c2".to_string(),
            approved: true,
        });
        let outcome = delete_schedule(
            "c2",
            &job_id,
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut rx,
        )
        .await
        .expect("approval returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("deleted permanently"), "deleted text: {msg}");
        assert!(store.list().is_empty(), "store empty after approval");
    }

    #[tokio::test]
    async fn cancel_schedule_from_chat_asks_first() {
        let _guard = crate::test_home();
        // Denied leg.
        let (store, job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut rx = canned_reader(&Request::ScheduleRemoveResponse {
            id: "c1".to_string(),
            approved: false,
        });
        let outcome = cancel_schedule(
            "c1",
            &job_id,
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut rx,
        )
        .await
        .expect("denial returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("denied"), "denied text: {msg}");
        match parse_first_frame(&out) {
            Response::ScheduleRemovePrompt { delete, .. } => assert!(!delete),
            other => panic!("expected ScheduleRemovePrompt, got {:?}", other.kind()),
        }
        assert_eq!(
            store.list()[0].status,
            JobStatus::Pending,
            "job still Pending after denial"
        );

        // Approved leg.
        let (store, job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut rx = canned_reader(&Request::ScheduleRemoveResponse {
            id: "c2".to_string(),
            approved: true,
        });
        let outcome = cancel_schedule(
            "c2",
            &job_id,
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut rx,
        )
        .await
        .expect("approval returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("cancelled"), "cancelled text: {msg}");
        assert_eq!(store.list().len(), 1, "cancelled job stays listed");
        assert_eq!(
            store.list()[0].status,
            JobStatus::Cancelled,
            "job status is Cancelled after approval"
        );
    }

    #[tokio::test]
    async fn schedule_removal_from_a_ghost_is_refused() {
        let (store, job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut empty_rx = tokio::io::BufReader::new(tokio::io::empty());
        for call_id in ["g1", "g2"] {
            let outcome = cancel_schedule(
                call_id,
                &job_id,
                true,
                &store,
                Some("sess-1"),
                &mut out,
                &mut empty_rx,
            )
            .await
            .expect("ghost refusal returns Ok");
            let ToolCallOutcome::Result(msg) = outcome else {
                panic!("expected Result");
            };
            assert!(msg.contains("Ghost Shell"), "ghost refusal text: {msg}");
            let outcome = delete_schedule(
                call_id,
                &job_id,
                true,
                &store,
                Some("sess-1"),
                &mut out,
                &mut empty_rx,
            )
            .await
            .expect("ghost refusal returns Ok");
            let ToolCallOutcome::Result(msg) = outcome else {
                panic!("expected Result");
            };
            assert!(msg.contains("Ghost Shell"), "ghost refusal text: {msg}");
        }
        assert!(out.is_empty(), "no prompt sent to a ghost");
        assert_eq!(store.list().len(), 1, "job unchanged after ghost refusal");
        assert_eq!(store.list()[0].status, JobStatus::Pending, "job unchanged");
    }

    #[tokio::test]
    async fn delete_schedule_refuses_a_reply_for_another_prompt() {
        let _guard = crate::test_home();
        let (store, job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut rx = canned_reader(&Request::ScheduleRemoveResponse {
            id: "other".to_string(),
            approved: true,
        });
        let outcome = delete_schedule(
            "c1",
            &job_id,
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut rx,
        )
        .await
        .expect("mismatched-id reply returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("denied"), "denial text: {msg}");
        assert_eq!(store.list().len(), 1, "job is still in the store");
    }

    #[tokio::test]
    async fn schedule_removal_of_an_unknown_id_sends_no_prompt() {
        let (store, _job_id) = test_store();
        let mut out: Vec<u8> = Vec::new();
        let mut empty_rx = tokio::io::BufReader::new(tokio::io::empty());
        let outcome = delete_schedule(
            "u1",
            "no-such-job",
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut empty_rx,
        )
        .await
        .expect("unknown id returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("not found"), "not-found text: {msg}");
        assert!(out.is_empty(), "no prompt sent for an unknown id");

        let outcome = cancel_schedule(
            "u2",
            "no-such-job",
            false,
            &store,
            Some("sess-1"),
            &mut out,
            &mut empty_rx,
        )
        .await
        .expect("unknown id returns Ok");
        let ToolCallOutcome::Result(msg) = outcome else {
            panic!("expected Result");
        };
        assert!(msg.contains("not found"), "not-found text: {msg}");
        assert!(out.is_empty(), "no prompt sent for an unknown id");
    }
}
