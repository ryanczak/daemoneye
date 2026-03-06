use crate::daemon::session::{bg_done_subscribe, SessionStore, append_session_message};
use crate::daemon::utils::{log_event, normalize_output};
use crate::ipc::Response;
use crate::tmux;
use crate::ai::{Message};
use crate::ai::filter::mask_sensitive;
use std::time::Duration;

/// Run a command in a dedicated tmux window (`de-bg-<id_short>`) on the daemon host.
///
/// The window is always killed after the output is captured.
/// If the command contains sudo and a `credential` is provided, it is injected
/// into the window after the sudo password prompt is detected.
pub async fn run_background_in_window(
    session: &str,
    tool_id: &str,
    cmd: &str,
    credential: Option<&str>,
    session_id: Option<String>,
    sessions: SessionStore,
) -> String {
    let id_short = &tool_id[..tool_id.len().min(8)];
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let win_name = format!("{}{}-{}-{}", crate::daemon::BG_WINDOW_PREFIX, session, now, id_short);
    let wrapped = format!("{}; exit $?", cmd);

    let pane_id = match tmux::create_job_window(session, &win_name) {
        Ok(p) => p,
        Err(e) => return format!("Failed to create background window: {}", e),
    };
    
    let started_at = tokio::time::Instant::now();
    let pane_id_log = pane_id.clone();
    
    // P7: keep the pane alive in a '<dead>' state so we can query pane_dead_status.
    if let Err(e) = tmux::set_remain_on_exit(&pane_id, true) {
        log::warn!("Failed to set remain-on-exit for {}: {}", win_name, e);
    }

    if let Err(e) = tmux::send_keys(&pane_id, &wrapped) {
        let _ = tmux::kill_job_window(session, &win_name);
        return format!("Failed to send command to window: {}", e);
    }

    // If sudo is expected, watch for the password prompt and inject the credential.
    if let Some(cred) = credential {
        let poll = Duration::from_millis(200);
        let prompt_timeout = Duration::from_secs(10);
        let mut waited = Duration::ZERO;
        loop {
            tokio::time::sleep(poll).await;
            waited += poll;
            let snap = tmux::capture_pane(&pane_id, 50).unwrap_or_default();
            // Common sudo prompt patterns
            let has_prompt = snap.contains("password") || snap.contains("Password") || snap.contains("[sudo]");
            if has_prompt {
                let _ = tmux::send_keys(&pane_id, cred);
                break;
            }
            if waited >= prompt_timeout || tmux::pane_dead_status(&pane_id).is_some() {
                break;
            }
        }
    }

    let session_owned = session.to_string();
    let win_owned = win_name.clone();
    let cmd_owned = cmd.to_string();

    let session_id_clone = session_id.clone();
    tokio::spawn(async move {
        let mut rx = bg_done_subscribe();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3600); // 1 hour max
        
        let exit_code = loop {
            if let Some(code) = tmux::pane_dead_status(&pane_id) {
                break code;
            }
            if tokio::time::Instant::now() >= deadline {
                break 124;
            }
            tokio::select! {
                result = rx.recv() => {
                    if let Ok(notified_pane) = result {
                        if notified_pane == pane_id {
                            if let Some(code) = tmux::pane_dead_status(&pane_id) {
                                break code;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    break 124;
                }
            }
        };

        notify_job_completion(
            pane_id, cmd_owned, win_owned, session_owned, exit_code,
            session_id_clone, sessions, None, started_at
        ).await;
    });

    log_event("job_start", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "job_id": id_short,
        "job_name": win_name,
        "pane": pane_id_log,
    }));

    format!("Started background command in window {}", win_name)
}

/// Shared completion handler: called after any background pane exits.
///
/// Handles:
/// - Capture + normalize + mask pane output
/// - Archive to `pane_logs`
/// - Inject AI context message into session history (if `session_id` is set)
/// - Send `tmux display-message` overlay to the chat pane (if known)
/// - Send `Response::SystemMsg` via `notify_tx` (if set)
/// - Emits `job_complete` and `gc_window` events.
/// - GC: kill the job window after a delay
pub async fn notify_job_completion(
    pane_id: String,
    cmd: String,
    win_name: String,
    session: String,
    exit_code: i32,
    session_id: Option<String>,
    sessions: SessionStore,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
    started_at: tokio::time::Instant,
) {
    let raw = tmux::capture_pane(&pane_id, 5000).unwrap_or_default();
    let duration_ms = started_at.elapsed().as_millis() as u64;

    log_event("job_complete", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "job_id": win_name.split('-').last().unwrap_or(""),
        "job_name": win_name,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
    }));

    // Archive logs
    let logs_dir = crate::config::config_dir().join("pane_logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        log::error!("Failed to create pane_logs directory: {}", e);
    } else if let Err(e) = tmux::pane::capture_pane_to_file(&pane_id, &logs_dir.join(format!("{}.log", win_name))) {
        log::error!("Failed to archive pane logs for {}: {}", win_name, e);
    }

    let normalized = normalize_output(&raw);
    let body = if normalized.is_empty() {
        "(no output)".to_string()
    } else {
        mask_sensitive(&normalized)
    };

    let status_word = if exit_code == 0 { "succeeded" } else { "failed" };
    let alert_msg = format!("`{}` {} in pane {}", cmd, status_word, pane_id);

    // Inject AI context + tmux display-message (if a session is associated)
    if let Some(ref sid) = session_id {
        if let Ok(mut store) = sessions.lock() {
            if let Some(entry) = store.get_mut(sid) {
                let history_msg = format!(
                    "Background command `{}` in window {} finished with exit code {}.\n<output>\n{}\n</output>",
                    cmd, win_name, exit_code, body
                );
                let completion_msg = Message {
                    role: "user".to_string(),
                    content: format!("[Background Task Completed]\n{}", history_msg),
                    tool_calls: None,
                    tool_results: None,
                };
                append_session_message(sid, &completion_msg);
                entry.messages.push(completion_msg);

                if let Some(ref cp) = entry.chat_pane {
                    if let Err(e) = std::process::Command::new("tmux")
                        .args(["display-message", "-d", "5000", "-t", cp, &alert_msg])
                        .output()
                    {
                        log::error!("Failed to send tmux display-message to chat pane: {}", e);
                    }
                }
            }
        }
    }

    // Also send as a SystemMsg to any listening chat client
    if let Some(ref tx) = notify_tx {
        let _ = tx.send(Response::SystemMsg(alert_msg));
    }

    // GC: kill window after a delay (keep failed windows open longer for inspection)
    let gc_delay = if exit_code == 0 { Duration::from_secs(5) } else { Duration::from_secs(60) };
    tokio::time::sleep(gc_delay).await;

    let reason = if exit_code == 0 { "done" } else if exit_code == 124 { "timeout" } else { "error" };
    log_event("gc_window", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "win_name": win_name,
        "pane": pane_id,
        "reason": reason,
    }));
    if let Err(e) = tmux::kill_job_window(&session, &win_name) {
        log::error!("Failed to GC kill background window {}: {}", win_name, e);
        if let Some(ref tx) = notify_tx {
            let msg = format!("System warning: failed to close background window {}. You may need to close it manually.", win_name);
            let _ = tx.send(Response::SystemMsg(msg));
        }
    }
}
