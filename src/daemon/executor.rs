use crate::daemon::session::{bg_done_subscribe, SessionStore};
use crate::daemon::utils::*;
use crate::daemon::background::{run_background_in_window};
use crate::ipc::{MemoryListItem, PaneInfo, Request, Response, RunbookListItem, ScheduleListItem, ScriptListItem};
use crate::scheduler::{ActionOn, ScheduleKind, ScheduledJob, ScheduleStore};
use crate::scripts;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::{mask_sensitive, next_tool_id, PendingCall};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;

// ---------------------------------------------------------------------------
// Timing constants — all durations used by tool execution in one place.
// ---------------------------------------------------------------------------

/// How long a user has to approve or deny a foreground/background tool call.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a user has to respond to a credential or write prompt (sudo password, schedule, script).
const USER_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Poll interval when detecting whether a sudo password prompt has appeared.
const SUDO_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Window within which a sudo password prompt must appear before giving up.
const SUDO_DETECT_WINDOW: Duration = Duration::from_secs(3);
/// Poll interval for remote-pane (SSH/mosh) output-stability check.
const REMOTE_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Max time to wait for a command to complete in a remote pane.
const REMOTE_CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// Fast poll used to detect that a child process has started in a local pane.
const LOCAL_CHILD_POLL: Duration = Duration::from_millis(25);
/// Window within which a child process must appear before falling back to hook-only wait.
const LOCAL_CHILD_START_WINDOW: Duration = Duration::from_millis(300);
/// Max time to wait for a command to complete in a local pane.
const LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(45);
/// Slow poll used while waiting for a local command to return to the shell prompt.
const LOCAL_SLOW_POLL: Duration = Duration::from_millis(500);
/// Delay after command completion before capturing output, to let the shell flush.
const POST_CMD_CAPTURE_DELAY: Duration = Duration::from_millis(50);

/// Send a `ToolCallPrompt` to the client, wait up to [`APPROVAL_TIMEOUT`] for
/// the user's [`Request::ToolCallResponse`], and log the outcome.
///
/// Returns `Ok(None)` when the user approves.  Returns `Ok(Some(msg))` when
/// the user denies or the wait times out — the caller should use `msg` as the
/// tool result.  Returns `Err` on connection EOF.
async fn prompt_and_await_approval(
    id: &str,
    cmd: &str,
    background: bool,
    session_id: Option<&str>,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<Option<String>> {
    let mode = if background { "background" } else { "foreground" };
    send_response_split(tx, Response::ToolCallPrompt {
        id: id.to_string(),
        command: cmd.to_string(),
        background,
    }).await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(APPROVAL_TIMEOUT, rx.read_line(&mut line)).await;

    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }

    let timed_out = read_result.is_err();
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ToolCallResponse { id: resp_id, approved }) if resp_id == id => approved,
            _ => false,
        },
        _ => false,
    };

    let decision = if approved { "approved" } else if timed_out { "timeout" } else { "denied" };
    log::info!("{} command {}: {}", mode, decision, cmd);
    log_event("command_approval", serde_json::json!({
        "session": session_id.unwrap_or("-"),
        "mode": mode,
        "cmd": cmd,
        "decision": decision,
    }));

    if !approved {
        log_command(session_id, mode, "", cmd, decision, "");
        let msg = if timed_out {
            format!("Approval timed out ({} s); command not executed.", APPROVAL_TIMEOUT.as_secs())
        } else {
            "User denied execution".to_string()
        };
        return Ok(Some(msg));
    }

    Ok(None)
}

async fn find_best_target_pane(
    target: Option<&str>,
    chat_pane: Option<&str>,
    cache: &Arc<SessionCache>,
    sessions: &SessionStore,
    session_id: Option<&str>,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<String> {
    let ai_target = target.and_then(|tp: &str| {
        if chat_pane == Some(tp) { return None; }
        let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
        if panes.contains_key(tp) { Some(tp.to_string()) } else { None::<String> }
    });

    if let Some(tp) = ai_target {
        return Ok(tp);
    }
    
    // Check for a user-selected default target pane in the session
    if let Some(sid) = session_id {
        if let Ok(store) = sessions.lock() {
            if let Some(entry) = store.get(sid) {
                if let Some(ref dtp) = entry.default_target_pane {
                    if chat_pane.as_deref() != Some(dtp.as_str()) {
                        let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
                        if panes.contains_key(dtp) {
                            return Ok(dtp.clone());
                        }
                    }
                }
            }
        }
    }

    let pane_list: Vec<PaneInfo> = {
        let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<PaneInfo> = panes.iter()
            .map(|(pid, state)| PaneInfo {
                id: pid.clone(),
                current_cmd: state.current_cmd.clone(),
                summary: state.summary.clone(),
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    };
    
    if pane_list.is_empty() {
        send_response_split(tx, Response::Error(
            "No tmux panes available".to_string()
        )).await?;
        return Err(anyhow::anyhow!("No active pane found."));
    }
    
    let prompt_id = next_tool_id();
    send_response_split(tx, Response::PaneSelectPrompt {
        id: prompt_id.clone(),
        panes: pane_list,
    }).await?;
    
    let mut pane_line = String::new();
    rx.read_line(&mut pane_line).await?;
    match serde_json::from_str::<Request>(pane_line.trim()) {
        Ok(Request::PaneSelectResponse { pane_id, .. }) => {
            // Save user choice as default for the session
            if let Some(sid) = session_id {
                if let Ok(mut store) = sessions.lock() {
                    if let Some(entry) = store.get_mut(sid) {
                        entry.default_target_pane = Some(pane_id.clone());
                    }
                }
            }
            Ok(pane_id)
        },
        _ => {
            send_response_split(tx, Response::Error(
                "Expected PaneSelectResponse".to_string()
            )).await?;
            Err(anyhow::anyhow!("User aborted or invalid response"))
        }
    }
}

pub async fn execute_tool_call(
    call: &PendingCall,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    session_id: Option<&str>,
    session_name: &str,
    chat_pane: Option<&str>,
    cache: &Arc<SessionCache>,
    sessions: &SessionStore,
    schedule_store: &Arc<ScheduleStore>,
) -> anyhow::Result<String> {
    let result: String = match call {
        PendingCall::Foreground { id, cmd, target, .. } => {
            if let Some(denial) = prompt_and_await_approval(id, cmd, false, session_id, tx, rx).await? {
                return Ok(denial);
            }
            let target_owned = match find_best_target_pane(target.as_deref(), chat_pane, cache, sessions, session_id, tx, rx).await {
                    Ok(tp) => tp,
                    Err(_) => return Err(anyhow::anyhow!("EOF")),
                };
                
                let target_str = target_owned.as_str();
                if target_str.is_empty() {
                    "No active pane found.".to_string()
                } else {
                    let is_synchronized = {
                        let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
                        panes.get(target_str).map(|p| p.synchronized).unwrap_or(false)
                    };
                    if is_synchronized {
                        let msg = format!(
                            "Pane {} has synchronized input enabled — sending a command \
                             would broadcast to all synchronized panes simultaneously. \
                             Disable synchronization first:\n  \
                             tmux set-option -t {} synchronize-panes off",
                            target_str, target_str
                        );
                        send_response_split(tx, Response::SystemMsg(msg.clone())).await?;
                        msg
                    } else {
                        let idle_cmd = tmux::pane_current_command(target_str)
                            .unwrap_or_default();
                        let is_remote_pane = get_pane_remote_host(target_str).is_some();

                        let current_exe = std::env::current_exe()
                            .unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
                        let hook_idx = crate::daemon::session::FG_HOOK_COUNTER
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let hook_name = format!("pane-title-changed[@de_fg_{}]", hook_idx);
                        let notify_cmd = format!(
                            "run-shell -b '{} notify activity {} 0 \"{}\"'",
                            current_exe.display(), target_str, shell_escape_arg(session_name)
                        );
                        let _ = std::process::Command::new("tmux")
                            .args(["set-hook", "-t", target_str, &hook_name, &notify_cmd])
                            .output();

                        let mut fg_rx = bg_done_subscribe();

                        match tmux::send_keys(target_str, cmd) {
                            Ok(()) => {
                                let mut switched_to_working = false;

                                if command_has_sudo(cmd) {
                                    let poll = SUDO_POLL_INTERVAL;
                                    let mut waited = Duration::ZERO;
                                    let prompt_timeout = SUDO_DETECT_WINDOW;
                                    let needs_password = loop {
                                        tokio::time::sleep(poll).await;
                                        waited += poll;
                                        let cur = tmux::pane_current_command(target_str)
                                            .unwrap_or_default();
                                        if cur == "sudo"   { break true;  }
                                        if cur == idle_cmd { break false; }
                                        if waited >= prompt_timeout { break false; }
                                    };

                                    if needs_password {
                                        send_response_split(tx, Response::SystemMsg(
                                            "sudo password prompt detected — \
                                             switching to your terminal pane. \
                                             Type your password there.".to_string()
                                        )).await?;
                                        let _ = tmux::select_pane(target_str);
                                        switched_to_working = true;
                                    }
                                }

                                if is_remote_pane {
                                    let mut prev_snap = String::new();
                                    let mut stable_ticks = 0u32;
                                    let poll = REMOTE_POLL_INTERVAL;
                                    let cmd_timeout = REMOTE_CMD_TIMEOUT;
                                    let deadline = tokio::time::Instant::now() + cmd_timeout;
                                    
                                    loop {
                                        if tokio::time::Instant::now() >= deadline { break; }
                                        tokio::select! {
                                            result = fg_rx.recv() => {
                                                if let Ok(notified_pane) = result {
                                                    if notified_pane == target_str {
                                                        stable_ticks = 0;
                                                    }
                                                }
                                            }
                                            _ = tokio::time::sleep(poll) => {
                                                let snap = tmux::capture_pane(target_str, 10).unwrap_or_default();
                                                if snap == prev_snap && !snap.is_empty() {
                                                    stable_ticks += 1;
                                                    if stable_ticks >= 2 { break; }
                                                } else {
                                                    stable_ticks = 0;
                                                    prev_snap = snap;
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    let fast_poll = LOCAL_CHILD_POLL;
                                    let start_timeout = LOCAL_CHILD_START_WINDOW;
                                    let cmd_timeout = LOCAL_CMD_TIMEOUT;
                                    let deadline = tokio::time::Instant::now() + cmd_timeout;

                                    let saw_child = tokio::time::timeout(start_timeout, async {
                                        loop {
                                            tokio::time::sleep(fast_poll).await;
                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                            if cur != idle_cmd { break; }
                                        }
                                    }).await.is_ok();

                                    if saw_child {
                                        let slow_poll = LOCAL_SLOW_POLL;
                                        loop {
                                            if tokio::time::Instant::now() >= deadline { break; }
                                            tokio::select! {
                                                result = fg_rx.recv() => {
                                                    if let Ok(notified_pane) = result {
                                                        if notified_pane == target_str {
                                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                            if cur == idle_cmd { break; }
                                                        }
                                                    }
                                                }
                                                _ = tokio::time::sleep(slow_poll) => {
                                                    let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                    if cur == idle_cmd { break; }
                                                }
                                            }
                                        }
                                    }
                                }
                                
                                let _ = std::process::Command::new("tmux")
                                    .args(["set-hook", "-u", "-t", target_str, &hook_name])
                                    .output();

                                tokio::time::sleep(POST_CMD_CAPTURE_DELAY).await;

                                let output = match tmux::capture_pane(target_str, 200) {
                                    Ok(snap) => {
                                        let extracted = extract_command_output(&snap, cmd);
                                        mask_sensitive(&normalize_output(&extracted))
                                    }
                                    Err(_) => "Command sent but could not capture output.".to_string(),
                                };

                                if switched_to_working {
                                    if let Some(cp) = chat_pane {
                                        let _ = tmux::select_pane(cp);
                                    }
                                }

                                send_response_split(tx, Response::ToolResult(output.clone())).await?;
                                log_command(session_id, "foreground", target_str, cmd, "approved", &output);
                                output
                            }
                            Err(e) => {
                                let msg = format!("Failed to send command: {}", e);
                                log_command(session_id, "foreground", target_str, cmd, "send-failed", &msg);
                                msg
                            }
                        }
                    }
                }
        }

        PendingCall::Background { id, cmd, .. } => {
            if let Some(denial) = prompt_and_await_approval(id, cmd, true, session_id, tx, rx).await? {
                return Ok(denial);
            }
            let credential = if command_has_sudo(cmd) {
                    send_response_split(tx, Response::CredentialPrompt {
                        id: id.clone(),
                        prompt: format!("[sudo] password required for: {}", cmd),
                    }).await?;
                    let mut cred_line = String::new();
                    match tokio::time::timeout(
                        USER_PROMPT_TIMEOUT,
                        rx.read_line(&mut cred_line),
                    ).await {
                        Ok(Ok(_)) => match serde_json::from_str::<Request>(cred_line.trim()) {
                            Ok(Request::CredentialResponse { credential, .. }) => Some(credential),
                            _ => None,
                        },
                        _ => None,
                    }
                } else {
                    None
                };

                let session_id_owned = session_id.map(|s| s.to_string());
                let output = run_background_in_window(
                    session_name,
                    id,
                    cmd,
                    credential.as_deref(),
                    session_id_owned,
                    sessions.clone(),
                ).await;
                send_response_split(tx, Response::ToolResult(output.clone())).await?;
                log_command(session_id, "background", "", cmd, "approved", &output);
                output
        }

        PendingCall::ScheduleCommand { id: call_id, name, command, is_script, run_at, interval, runbook, .. } => {
            let action = if *is_script {
                ActionOn::Script(command.clone())
            } else {
                ActionOn::Command(command.clone())
            };
            let kind = if let Some(iso) = interval {
                let secs = crate::scheduler::parse_iso_duration(iso).unwrap_or(3600);
                let next = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
                ScheduleKind::Every { interval_secs: secs, next_run: next }
            } else if let Some(at_str) = run_at {
                let at = chrono::DateTime::parse_from_rfc3339(at_str).map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::seconds(60));
                ScheduleKind::Once { at }
            } else {
                ScheduleKind::Once { at: chrono::Utc::now() + chrono::Duration::seconds(60) }
            };

            send_response_split(tx, Response::ScheduleWritePrompt {
                id: call_id.clone(),
                name: name.clone(),
                kind: kind.describe(),
                action: action.describe(),
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(
                USER_PROMPT_TIMEOUT,
                rx.read_line(&mut line),
            ).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::ScheduleWriteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                let job = ScheduledJob::new(name.clone(), kind.clone(), action, runbook.clone());
                match schedule_store.add(job) {
                    Ok(job_id) => {
                        log::info!("Job scheduled: '{}' ({})", name, &job_id[..8]);
                        log_event("job_scheduled", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "job_id": &job_id,
                            "job_name": name,
                            "kind": kind.describe(),
                        }));
                        format!("Scheduled job '{}' created (id: {})", name, &job_id[..8])
                    }
                    Err(e) => format!("Failed to schedule job: {}", e),
                }
            } else {
                log_event("command_approval", serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": "schedule",
                    "cmd": command,
                    "decision": "denied",
                }));
                "Job scheduling denied by user".to_string()
            }
        }

        PendingCall::ListSchedules { .. } => {
            let jobs = schedule_store.list();
            let items: Vec<ScheduleListItem> = jobs.iter().map(|j| ScheduleListItem {
                id: j.id.clone(),
                name: j.name.clone(),
                kind: j.kind.describe(),
                action: j.action.describe(),
                status: j.status.describe(),
                last_run: j.last_run.map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
                next_run: j.kind.next_run().map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
            }).collect();
            let count = items.len();
            let _ = send_response_split(tx, Response::ScheduleList { jobs: items }).await;
            format!("{} scheduled job(s)", count)
        }

        PendingCall::CancelSchedule { job_id, .. } => {
            match schedule_store.cancel(job_id) {
                Ok(true) => {
                    log::info!("Job canceled: {}", &job_id[..job_id.len().min(8)]);
                    log_event("job_canceled", serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "job_id": job_id,
                    }));
                    format!("Job {} cancelled", &job_id[..job_id.len().min(8)])
                }
                Ok(false) => format!("Job {} not found", job_id),
                Err(e)  => format!("Failed to cancel job: {}", e),
            }
        }

        PendingCall::DeleteSchedule { job_id, .. } => {
            match schedule_store.delete(job_id) {
                Ok(true) => {
                    log::info!("Job deleted: {}", &job_id[..job_id.len().min(8)]);
                    log_event("job_deleted", serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "job_id": job_id,
                    }));
                    format!("Job {} deleted permanently", &job_id[..job_id.len().min(8)])
                }
                Ok(false) => format!("Job {} not found", job_id),
                Err(e)  => format!("Failed to delete job: {}", e),
            }
        }

        PendingCall::WriteScript { id, script_name, content, .. } => {
            send_response_split(tx, Response::ScriptWritePrompt {
                id: id.clone(),
                script_name: script_name.clone(),
                content: content.clone(),
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(
                USER_PROMPT_TIMEOUT,
                rx.read_line(&mut line),
            ).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::ScriptWriteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                match scripts::write_script(script_name, content) {
                    Ok(()) => format!("Script '{}' written successfully", script_name),
                    Err(e) => format!("Failed to write script: {}", e),
                }
            } else {
                "Script write denied by user".to_string()
            }
        }

        PendingCall::ListScripts { .. } => {
            let script_list = scripts::list_scripts().unwrap_or_default();
            let items: Vec<ScriptListItem> = script_list.iter()
                .map(|s| ScriptListItem { name: s.name.clone(), size: s.size })
                .collect();
            let count = items.len();
            let _ = send_response_split(tx, Response::ScriptList { scripts: items }).await;
            format!("{} script(s) in ~/.daemoneye/scripts/", count)
        }

        PendingCall::ReadScript { script_name, .. } => {
            match scripts::read_script(script_name) {
                Ok(content) => content,
                Err(e) => format!("Error reading script '{}': {}", script_name, e),
            }
        }

        PendingCall::WatchPane { pane_id, .. } => {
            let session_owned = session_name.to_string();
            match crate::tmux::install_passive_activity_hook(pane_id, &session_owned) {
                Ok(_) => {
                    if let Some(sid) = session_id {
                        if let Ok(mut store) = sessions.lock() {
                            if let Some(entry) = store.get_mut(sid) {
                                entry.watched_panes.insert(pane_id.clone());
                            }
                        }
                    }
                    log::info!("Watch placed on pane {} for session {}", pane_id, session_id.unwrap_or("-"));
                    log_event("watch_pane", serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "pane_id": pane_id,
                        "status": "active"
                    }));
                    format!("Pane {} has been flagged for passive monitoring. You will be notified out-of-band via a [System] message when it produces output.", pane_id)
                }
                Err(e) => {
                    log::warn!("Failed to monitor pane {}: {}", pane_id, e);
                    format!("Failed to monitor pane {}: {}", pane_id, e)
                }
            }
        }

        PendingCall::WriteRunbook { id, name, content, .. } => {
            send_response_split(tx, Response::RunbookWritePrompt {
                id: id.clone(),
                runbook_name: name.clone(),
                content: content.clone(),
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::RunbookWriteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                match crate::runbook::write_runbook(name, content) {
                    Ok(()) => {
                        log::info!("Runbook '{}' written", name);
                        log_event("runbook_write", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "runbook": name,
                        }));
                        format!("Runbook '{}' written to ~/.daemoneye/runbooks/{}.md", name, name)
                    }
                    Err(e) => format!("Failed to write runbook: {}", e),
                }
            } else {
                "Runbook write denied by user".to_string()
            }
        }

        PendingCall::DeleteRunbook { id, name, .. } => {
            // Check for active scheduled jobs that reference this runbook
            let active_jobs: Vec<String> = schedule_store.list()
                .into_iter()
                .filter(|j| j.runbook.as_deref() == Some(name))
                .map(|j| j.name)
                .collect();

            send_response_split(tx, Response::RunbookDeletePrompt {
                id: id.clone(),
                runbook_name: name.clone(),
                active_jobs,
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::RunbookDeleteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                match crate::runbook::delete_runbook(name) {
                    Ok(()) => {
                        log::info!("Runbook '{}' deleted", name);
                        log_event("runbook_delete", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "runbook": name,
                        }));
                        format!("Runbook '{}' deleted", name)
                    }
                    Err(e) => format!("Failed to delete runbook: {}", e),
                }
            } else {
                "Runbook delete denied by user".to_string()
            }
        }

        PendingCall::ReadRunbook { name, .. } => {
            match crate::runbook::load_runbook(name) {
                Ok(rb) => rb.content,
                Err(e) => format!("Error reading runbook '{}': {}", name, e),
            }
        }

        PendingCall::ListRunbooks { .. } => {
            let items = crate::runbook::list_runbooks().unwrap_or_default();
            let count = items.len();
            let runbook_items: Vec<RunbookListItem> = items.iter()
                .map(|r| RunbookListItem { name: r.name.clone(), tags: r.tags.clone() })
                .collect();
            let _ = send_response_split(tx, Response::RunbookList { runbooks: runbook_items }).await;
            format!("{} runbook(s) in ~/.daemoneye/runbooks/", count)
        }

        PendingCall::AddMemory { key, value, category, .. } => {
            let cat = crate::memory::MemoryCategory::from_str(category)
                .unwrap_or(crate::memory::MemoryCategory::Knowledge);
            match crate::memory::add_memory(key, value, cat) {
                Ok(()) => format!("Memory '{}' stored in {}", key, category),
                Err(e) => format!("Error storing memory: {}", e),
            }
        }

        PendingCall::DeleteMemory { key, category, .. } => {
            let cat = crate::memory::MemoryCategory::from_str(category)
                .unwrap_or(crate::memory::MemoryCategory::Knowledge);
            match crate::memory::delete_memory(key, cat) {
                Ok(()) => format!("Memory '{}' deleted from {}", key, category),
                Err(e) => format!("Error deleting memory: {}", e),
            }
        }

        PendingCall::ReadMemory { key, category, .. } => {
            let cat = crate::memory::MemoryCategory::from_str(category)
                .unwrap_or(crate::memory::MemoryCategory::Knowledge);
            match crate::memory::read_memory(key, cat) {
                Ok(content) => content,
                Err(e) => format!("Error reading memory '{}': {}", key, e),
            }
        }

        PendingCall::ListMemories { category, .. } => {
            let cat = category.as_deref().and_then(crate::memory::MemoryCategory::from_str);
            let entries = crate::memory::list_memories(cat).unwrap_or_default();
            let count = entries.len();
            let items: Vec<MemoryListItem> = entries.iter()
                .map(|(c, k)| MemoryListItem { category: c.clone(), key: k.clone() })
                .collect();
            let _ = send_response_split(tx, Response::MemoryList { entries: items }).await;
            format!("{} memory entry/entries", count)
        }

        PendingCall::SearchRepository { query, kind, .. } => {
            let results = crate::search::search_repository(query, kind, 2);
            crate::search::format_results(&results)
        }
    };
    Ok(result)
}
