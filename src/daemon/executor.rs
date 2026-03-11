use crate::daemon::session::{bg_done_subscribe, append_session_message, FG_HOOK_COUNTER, SessionStore};
use crate::daemon::utils::*;
use crate::daemon::background::{run_background_in_window};
use crate::ipc::{MemoryListItem, PaneInfo, Request, Response, RunbookListItem, ScheduleListItem, ScriptListItem};
use crate::scheduler::{ActionOn, JobStatus, ScheduleKind, ScheduledJob, ScheduleStore};
use crate::scripts;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::{mask_sensitive, next_tool_id, PendingCall};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;

/// The outcome of a single tool call execution.
pub enum ToolCallOutcome {
    /// Normal result string to feed back to the AI.
    Result(String),
    /// The user typed a corrective message at the approval prompt.
    /// The caller must abort the current tool chain and inject this text as a
    /// new user turn so the AI can course-correct without seeing a synthetic
    /// tool error.
    UserMessage(String),
}

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

/// Return true when `cmd` is a shell name, meaning the pane is at a prompt.
fn is_shell_prompt(cmd: &str) -> bool {
    matches!(
        cmd.trim(),
        "bash" | "zsh" | "fish" | "sh" | "ksh" | "csh" | "tcsh" | "dash"
            | "nu" | "pwsh" | "elvish" | "xonsh" | "yash"
    )
}

/// Send a `ToolCallPrompt` to the client, wait up to [`APPROVAL_TIMEOUT`] for
/// the user's [`Request::ToolCallResponse`], and log the outcome.
///
/// Returns `Ok(None)` when the user approves.
/// Returns `Ok(Some(ToolCallOutcome::Result(msg)))` when the user denies or
/// the wait times out — the caller should propagate this as the tool result.
/// Returns `Ok(Some(ToolCallOutcome::UserMessage(text)))` when the user typed
/// a corrective message; the caller should abort the tool chain and inject the
/// text as a new user turn.
/// Returns `Err` on connection EOF.
async fn prompt_and_await_approval(
    id: &str,
    cmd: &str,
    background: bool,
    session_id: Option<&str>,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<Option<ToolCallOutcome>> {
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

    // Parse the response, checking for a user_message redirect first.
    enum Parsed { Approved, Denied, UserMessage(String) }
    let parsed = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ToolCallResponse { id: resp_id, approved, user_message }) if resp_id == id => {
                if let Some(msg) = user_message {
                    Parsed::UserMessage(msg)
                } else if approved {
                    Parsed::Approved
                } else {
                    Parsed::Denied
                }
            }
            _ => Parsed::Denied,
        },
        _ => Parsed::Denied,
    };

    match parsed {
        Parsed::Approved => {
            log::info!("{} command approved: {}", mode, cmd);
            log_event("command_approval", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": mode,
                "cmd": cmd,
                "decision": "approved",
            }));
            Ok(None)
        }
        Parsed::Denied => {
            let decision = if timed_out { "timeout" } else { "denied" };
            log::info!("{} command {}: {}", mode, decision, cmd);
            log_event("command_approval", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": mode,
                "cmd": cmd,
                "decision": decision,
            }));
            log_command(session_id, mode, "", cmd, decision, "");
            let msg = if timed_out {
                format!("Approval timed out ({} s); command not executed.", APPROVAL_TIMEOUT.as_secs())
            } else {
                "User denied execution".to_string()
            };
            Ok(Some(ToolCallOutcome::Result(msg)))
        }
        Parsed::UserMessage(text) => {
            log::info!("{} command redirected by user message: {}", mode, cmd);
            log_event("command_approval", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": mode,
                "cmd": cmd,
                "decision": "user_message",
            }));
            Ok(Some(ToolCallOutcome::UserMessage(text)))
        }
    }
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
) -> anyhow::Result<ToolCallOutcome> {
    let result: String = match call {
        PendingCall::Foreground { id, cmd, target, .. } => {
            if let Some(outcome) = prompt_and_await_approval(id, cmd, false, session_id, tx, rx).await? {
                return Ok(outcome);
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
            // Enforce per-session cap on open background windows.
            // All lock work is done inside this block so the guard is dropped before any await.
            const MAX_BG_WINDOWS_PER_SESSION: usize = 5;
            let cap_denial: Option<String> = {
                let mut denial = None;
                if let Some(sid) = session_id {
                    if let Ok(mut store) = sessions.lock() {
                        if let Some(entry) = store.get_mut(sid) {
                            if entry.bg_windows.len() >= MAX_BG_WINDOWS_PER_SESSION {
                                let evict_idx = entry.bg_windows.iter()
                                    .position(|w| w.exit_code.is_some());
                                match evict_idx {
                                    Some(i) => {
                                        let evicted = entry.bg_windows.remove(i);
                                        log::info!("Evicting completed bg window {} to stay under cap", evicted.window_name);
                                        if let Err(e) = crate::tmux::kill_job_window(&evicted.tmux_session, &evicted.window_name) {
                                            log::warn!("Failed to evict bg window {}: {}", evicted.window_name, e);
                                        }
                                    }
                                    None => {
                                        denial = Some(format!(
                                            "Background window cap ({}) reached and all windows are still running. \
                                             Wait for one to complete, or ask the user to close one of the open \
                                             background windows ({}) before starting another.",
                                            MAX_BG_WINDOWS_PER_SESSION,
                                            entry.bg_windows.iter().map(|w| w.window_name.as_str()).collect::<Vec<_>>().join(", ")
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                denial
            };
            if let Some(msg) = cap_denial {
                send_response_split(tx, Response::ToolResult(msg.clone())).await?;
                return Ok(ToolCallOutcome::Result(msg));
            }

            if let Some(outcome) = prompt_and_await_approval(id, cmd, true, session_id, tx, rx).await? {
                return Ok(outcome);
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
                let secs = match crate::scheduler::parse_iso_duration(iso) {
                    Some(s) => s,
                    None => return Ok(ToolCallOutcome::Result(format!(
                        "Invalid interval '{}'. Use ISO 8601 duration format, e.g. PT1M (1 minute), PT5M (5 minutes), PT1H (1 hour), P1D (1 day).",
                        iso
                    ))),
                };
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
                        format!("Scheduled job '{}' created (id: {})", name, job_id)
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
                // Only show next_run for pending jobs; for succeeded/failed/cancelled
                // jobs it would be a stale past timestamp that confuses the AI into
                // thinking the job needs to be re-scheduled.
                next_run: if matches!(j.status, JobStatus::Pending) {
                    j.kind.next_run().map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                } else {
                    None
                },
            }).collect();
            let count = items.len();
            let _ = send_response_split(tx, Response::ScheduleList { jobs: items.clone() }).await;
            // Build a full job listing for the AI so it has IDs for cancel/delete.
            if count == 0 {
                "No scheduled jobs.".to_string()
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
                lines
            }
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

        PendingCall::WatchPane { pane_id, timeout_secs, .. } => {
            // Sample the current foreground command so we know when the shell returns to a prompt.
            let initial_cmd = tmux::pane_current_command(pane_id).unwrap_or_default();

            // Install a pane-title-changed hook as a fast-path IPC signal.
            let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let hook_name = format!("pane-title-changed[@de_wp_{}]", hook_idx);
            let current_exe = std::env::current_exe()
                .unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
            let notify_cmd = format!(
                "run-shell -b '{} notify activity {} 0 \"{}\"'",
                current_exe.display(), pane_id, shell_escape_arg(session_name)
            );
            let _ = std::process::Command::new("tmux")
                .args(["set-hook", "-t", pane_id, &hook_name, &notify_cmd])
                .output();

            // Subscribe before spawning to avoid missing early signals.
            let mut wp_rx = bg_done_subscribe();

            let pane_id_owned = pane_id.to_string();
            let session_id_owned = session_id.unwrap_or("-").to_string();
            let sessions_clone = Arc::clone(sessions);
            let timeout = Duration::from_secs(*timeout_secs);

            log::info!("watch_pane: monitoring {} (initial_cmd={:?}) for session {}", pane_id, initial_cmd, session_id_owned);
            log_event("watch_pane", serde_json::json!({
                "session": session_id_owned,
                "pane_id": pane_id,
                "status": "active"
            }));

            tokio::spawn(async move {
                let slow_poll = Duration::from_millis(500);
                let start_wait = Duration::from_secs(5);

                let completed = tokio::time::timeout(timeout, async {
                    // If the pane is already at a shell prompt, first wait up to 5s for a
                    // command to start before we start watching for completion.
                    if is_shell_prompt(&initial_cmd) {
                        let _ = tokio::time::timeout(start_wait, async {
                            loop {
                                tokio::time::sleep(slow_poll).await;
                                let cur = tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                if !is_shell_prompt(&cur) { break; }
                            }
                        }).await;
                    }

                    // Race: pane-title-changed IPC signal vs 500 ms poll.
                    // Stop when pane_current_command returns to a shell name (command is done).
                    loop {
                        tokio::select! {
                            result = wp_rx.recv() => {
                                if let Ok(notified_pane) = result {
                                    if notified_pane == pane_id_owned {
                                        let cur = tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                        if is_shell_prompt(&cur) { break; }
                                    }
                                }
                            }
                            _ = tokio::time::sleep(slow_poll) => {
                                let cur = tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                if is_shell_prompt(&cur) { break; }
                            }
                        }
                    }
                }).await.is_ok();

                // Remove the pane-title-changed hook.
                let _ = std::process::Command::new("tmux")
                    .args(["set-hook", "-u", "-t", &pane_id_owned, &hook_name])
                    .output();

                // Capture and mask pane output.
                let raw = tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
                let body = crate::ai::filter::mask_sensitive(&normalize_output(&raw));

                let content = if completed {
                    format!(
                        "[Watch Pane Complete] Command finished in pane {}.\n<output>\n{}\n</output>",
                        pane_id_owned, body
                    )
                } else {
                    format!(
                        "[Watch Pane Timeout] Timed out waiting for command in pane {} to finish.\n<output>\n{}\n</output>",
                        pane_id_owned, body
                    )
                };

                let watch_msg = crate::ai::Message {
                    role: "user".to_string(),
                    content,
                    tool_calls: None,
                    tool_results: None,
                };

                if let Ok(mut store) = sessions_clone.lock() {
                    if let Some(entry) = store.get_mut(&session_id_owned) {
                        append_session_message(&session_id_owned, &watch_msg);
                        entry.messages.push(watch_msg);

                        let alert = if completed {
                            format!("Watched pane {} command completed", pane_id_owned)
                        } else {
                            format!("Watched pane {} timed out", pane_id_owned)
                        };
                        if let Some(ref cp) = entry.chat_pane {
                            let _ = std::process::Command::new("tmux")
                                .args(["display-message", "-d", "5000", "-t", cp, &alert])
                                .output();
                        }
                    }
                }
                log::info!("watch_pane {}: {}", pane_id_owned, if completed { "completed" } else { "timed out" });
            });

            format!(
                "Now watching pane {} for command completion. \
                 You will receive a [Watch Pane Complete] context message when the command finishes, \
                 or [Watch Pane Timeout] if it doesn't complete within {} seconds.",
                pane_id, timeout_secs
            )
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
            let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    category
                )));
            };
            if value.trim().is_empty() {
                return Ok(ToolCallOutcome::Result(
                    "Error: memory value cannot be empty.".to_string(),
                ));
            }
            match crate::memory::add_memory(key, value, cat) {
                Ok(()) => format!("Memory '{}' stored in {}", key, category),
                Err(e) => format!("Error storing memory: {}", e),
            }
        }

        PendingCall::DeleteMemory { key, category, .. } => {
            let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    category
                )));
            };
            match crate::memory::delete_memory(key, cat) {
                Ok(()) => format!("Memory '{}' deleted from {}", key, category),
                Err(e) => format!("Error deleting memory: {}", e),
            }
        }

        PendingCall::ReadMemory { key, category, .. } => {
            let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    category
                )));
            };
            match crate::memory::read_memory(key, cat) {
                Ok(content) => crate::ai::filter::mask_sensitive(&content),
                Err(e) => format!("Error reading memory '{}': {}", key, e),
            }
        }

        PendingCall::ListMemories { category, .. } => {
            let cat = match category.as_deref() {
                None => None,
                Some(s) => match crate::memory::MemoryCategory::from_str(s) {
                    Some(c) => Some(c),
                    None => return Ok(ToolCallOutcome::Result(format!(
                        "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                        s
                    ))),
                },
            };
            let entries = crate::memory::list_memories(cat).unwrap_or_default();
            let count = entries.len();
            let items: Vec<MemoryListItem> = entries.iter()
                .map(|(c, k)| MemoryListItem { category: c.clone(), key: k.clone() })
                .collect();
            let _ = send_response_split(tx, Response::MemoryList { entries: items }).await;
            if count == 0 {
                "No memory entries found.".to_string()
            } else {
                let lines: Vec<String> = entries.iter()
                    .map(|(c, k)| format!("[{}] {}", c, k))
                    .collect();
                format!("{} memory entries:\n{}", count, lines.join("\n"))
            }
        }

        PendingCall::SearchRepository { query, kind, .. } => {
            let results = crate::search::search_repository(query, kind, 2);
            crate::search::format_results(&results)
        }

        PendingCall::GetTerminalContext { .. } => {
            cache.get_labeled_context(chat_pane, chat_pane)
        }

        PendingCall::ListPanes { .. } => {
            let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
            let session = cache.session_name.read().unwrap_or_else(|e| e.into_inner()).clone();

            // Collect panes, excluding the chat pane (never a valid command target).
            let mut rows: Vec<_> = panes
                .iter()
                .filter(|(id, _)| chat_pane.map_or(true, |c| c != id.as_str()))
                .collect();
            rows.sort_by_key(|(id, _)| id.as_str());

            if rows.is_empty() {
                return Ok(ToolCallOutcome::Result(format!(
                    "No targetable panes found in session '{}'.", session
                )));
            }

            let mut out = format!(
                "{} pane{} in session '{}' (chat pane excluded):\n",
                rows.len(),
                if rows.len() == 1 { "" } else { "s" },
                session
            );
            for (id, state) in &rows {
                // Title: omit when it's identical to the command (redundant).
                let title_part = if !state.pane_title.is_empty() && state.pane_title != state.current_cmd {
                    format!("  title:{}", mask_sensitive(&state.pane_title))
                } else {
                    String::new()
                };
                let sync_part  = if state.synchronized { "  [synchronized]" } else { "" };
                let dead_part  = if state.dead {
                    format!("  [dead: {}]", state.dead_status.unwrap_or(0))
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "  {}  window:{:<12}  cmd:{:<8}  cwd:{}{}{}{}\n",
                    id,
                    state.window_name,
                    state.current_cmd,
                    state.current_path,
                    title_part,
                    sync_part,
                    dead_part,
                ));
            }
            out.push_str(
                "\nUse the pane ID as target_pane in run_terminal_command to execute a command there."
            );
            out
        }
    };
    Ok(ToolCallOutcome::Result(result))
}
