use crate::daemon::server::{run_background_in_window, send_response_split, PendingCall};
use crate::daemon::session::{bg_done_subscribe, SessionStore};
use crate::daemon::utils::*;
use crate::ipc::{PaneInfo, Request, Response, ScheduleListItem, ScriptListItem};
use crate::scheduler::{ActionOn, ScheduleKind, ScheduledJob, ScheduleStore};
use crate::scripts;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::filter::mask_sensitive;
use crate::ai::next_tool_id;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;

pub async fn execute_tool_call(
    call: &PendingCall,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    session_id: Option<&str>,
    session_name: &str,
    chat_pane: Option<&str>,
    client_pane: Option<&str>,
    cache: &Arc<SessionCache>,
    sessions: &SessionStore,
    schedule_store: &Arc<ScheduleStore>,
) -> anyhow::Result<String> {
    let result: String = match call {
        PendingCall::Foreground { id, cmd, target, .. } => {
            send_response_split(tx, Response::ToolCallPrompt {
                id: id.clone(),
                command: cmd.clone(),
                background: false,
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(
                Duration::from_secs(60),
                rx.read_line(&mut line),
            ).await;

            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }

            let timed_out = read_result.is_err();
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::ToolCallResponse { id: resp_id, approved }) if resp_id == *id => approved,
                    _ => false,
                },
                _ => false,
            };

            if !approved {
                let decision = if timed_out { "timeout" } else { "denied" };
                log::info!("Foreground command {}: {}", decision, cmd);
                log_event("command_approval", serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": "foreground",
                    "cmd": cmd,
                    "decision": decision,
                }));
                log_command(session_id, "foreground", "", cmd, decision, "");
                if timed_out {
                    "Approval timed out (60 s); command not executed.".to_string()
                } else {
                    "User denied execution".to_string()
                }
            } else {
                log::info!("Foreground command approved: {}", cmd);
                log_event("command_approval", serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": "foreground",
                    "cmd": cmd,
                    "decision": "approved",
                }));
                let ai_target = target.as_deref().and_then(|tp: &str| {
                    if chat_pane == Some(tp) { return None; }
                    let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
                    if panes.contains_key(tp) { Some(tp.to_string()) } else { None::<String> }
                });

                let target_owned: String = if let Some(tp) = ai_target {
                    tp
                } else if let Some(cp) = client_pane.filter(|cp| chat_pane != Some(*cp)) {
                    cp.to_string()
                } else {
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
                        return Err(anyhow::anyhow!("EOF"));
                    }
                    let prompt_id = next_tool_id();
                    send_response_split(tx, Response::PaneSelectPrompt {
                        id: prompt_id.clone(),
                        panes: pane_list,
                    }).await?;
                    let mut pane_line = String::new();
                    rx.read_line(&mut pane_line).await?;
                    match serde_json::from_str::<Request>(pane_line.trim()) {
                        Ok(Request::PaneSelectResponse { pane_id, .. }) => pane_id,
                        _ => {
                            send_response_split(tx, Response::Error(
                                "Expected PaneSelectResponse".to_string()
                            )).await?;
                            return Err(anyhow::anyhow!("EOF"));
                        }
                    }
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
                        let hook_idx = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 10000;
                        let hook_name = format!("pane-title-changed[@de_fg_{}]", hook_idx);
                        let notify_cmd = format!(
                            "run-shell -b '{} notify activity {} 0 \"{}\"'",
                            current_exe.display(), target_str, session_name
                        );
                        let _ = std::process::Command::new("tmux")
                            .args(["set-hook", "-t", target_str, &hook_name, &notify_cmd])
                            .output();

                        let mut fg_rx = bg_done_subscribe();

                        match tmux::send_keys(target_str, cmd) {
                            Ok(()) => {
                                let mut switched_to_working = false;

                                if command_has_sudo(cmd) {
                                    let poll = Duration::from_millis(100);
                                    let mut waited = Duration::ZERO;
                                    let prompt_timeout = Duration::from_secs(3);
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
                                    let poll = Duration::from_millis(500); // Slower fallback poll
                                    let cmd_timeout = Duration::from_secs(30);
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
                                    let fast_poll = Duration::from_millis(25);
                                    let start_timeout = Duration::from_millis(300);
                                    let cmd_timeout = Duration::from_secs(45);
                                    let deadline = tokio::time::Instant::now() + cmd_timeout;

                                    let saw_child = tokio::time::timeout(start_timeout, async {
                                        loop {
                                            tokio::time::sleep(fast_poll).await;
                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                            if cur != idle_cmd { break; }
                                        }
                                    }).await.is_ok();

                                    if saw_child {
                                        let slow_poll = Duration::from_millis(500);
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

                                tokio::time::sleep(Duration::from_millis(50)).await;

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
        }

        PendingCall::Background { id, cmd, .. } => {
            send_response_split(tx, Response::ToolCallPrompt {
                id: id.clone(),
                command: cmd.clone(),
                background: true,
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(
                Duration::from_secs(60),
                rx.read_line(&mut line),
            ).await;

            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }

            let timed_out = read_result.is_err();
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::ToolCallResponse { id: resp_id, approved }) if resp_id == *id => approved,
                    _ => false,
                },
                _ => false,
            };

            if !approved {
                let decision = if timed_out { "timeout" } else { "denied" };
                log::info!("Background command {}: {}", decision, cmd);
                log_event("command_approval", serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": "background",
                    "cmd": cmd,
                    "decision": decision,
                }));
                log_command(session_id, "background", "", cmd, decision, "");
                if timed_out {
                    "Approval timed out (60 s); command not executed.".to_string()
                } else {
                    "User denied execution".to_string()
                }
            } else {
                log::info!("Background command approved: {}", cmd);
                log_event("command_approval", serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": "background",
                    "cmd": cmd,
                    "decision": "approved",
                }));
                let credential = if command_has_sudo(cmd) {
                    send_response_split(tx, Response::CredentialPrompt {
                        id: id.clone(),
                        prompt: format!("[sudo] password required for: {}", cmd),
                    }).await?;
                    let mut cred_line = String::new();
                    match tokio::time::timeout(
                        Duration::from_secs(120),
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
                Duration::from_secs(120),
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
                Duration::from_secs(120),
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
    };
    Ok(result)
}
