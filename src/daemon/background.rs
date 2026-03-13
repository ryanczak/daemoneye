use crate::daemon::session::{bg_done_subscribe, complete_subscribe, BgWindowInfo, SessionStore, append_session_message};
use crate::daemon::utils::{log_event, normalize_output, shell_escape_arg};
use crate::ipc::Response;
use crate::tmux;
use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Shell helpers
// ---------------------------------------------------------------------------

/// Returns the exit-code variable for the detected shell.
/// Fish and csh/tcsh use `$status`; all POSIX-compatible shells use `$?`.
fn shell_exit_var(shell_name: &str) -> &'static str {
    match shell_name.trim() {
        "fish" | "csh" | "tcsh" => "$status",
        _ => "$?",
    }
}

// ---------------------------------------------------------------------------
// Shared capture / archive / notify helpers
// ---------------------------------------------------------------------------

/// Capture and mask pane output, archive the full scrollback to `pane_logs/`.
/// Returns the masked body string suitable for the AI.
fn capture_and_archive(pane_id: &str, win_name: &str) -> String {
    let raw = tmux::capture_pane(pane_id, 5000).unwrap_or_default();
    let normalized = normalize_output(&raw);
    let body = if normalized.is_empty() {
        "(no output)".to_string()
    } else {
        mask_sensitive(&normalized)
    };
    let logs_dir = crate::config::config_dir().join("pane_logs");
    if std::fs::create_dir_all(&logs_dir).is_ok() {
        let _ = tmux::pane::capture_pane_to_file(pane_id, &logs_dir.join(format!("{}.log", win_name)));
    }
    body
}

/// Inject a `[Background Task Completed]` message into the session history,
/// update `exit_code` in `bg_windows`, and flash a `tmux display-message`.
///
/// `pane_persists` — if true, the window is still open and the AI can reuse it.
fn notify_session(
    sessions: &SessionStore,
    session_id: &str,
    pane_id: &str,
    cmd: &str,
    win_name: &str,
    exit_code: i32,
    body: &str,
    pane_persists: bool,
) {
    let Ok(mut store) = sessions.lock() else { return };
    let Some(entry) = store.get_mut(session_id) else { return };

    // Update exit_code in the bg_windows registry.
    if let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id) {
        w.exit_code = Some(exit_code);
    }

    let persist_note = if pane_persists {
        format!(
            "The window is still open (pane {pane_id}). \
             Use target=\"{pane_id}\" to run follow-up commands in the same shell."
        )
    } else {
        format!(
            "The window was closed. Full log: ~/.daemoneye/pane_logs/{win_name}.log"
        )
    };

    let history_content = format!(
        "Background command `{cmd}` in window {win_name} finished with exit code {exit_code}.\n\
         {persist_note}\n<output>\n{body}\n</output>"
    );
    let completion_msg = Message {
        role: "user".to_string(),
        content: format!("[Background Task Completed]\n{}", history_content),
        tool_calls: None,
        tool_results: None,
    };
    append_session_message(session_id, &completion_msg);
    entry.messages.push(completion_msg);

    let status_word = if exit_code == 0 { "succeeded" } else { "failed" };
    let alert = format!("`{cmd}` {status_word} in pane {pane_id}");
    if let Some(ref cp) = entry.chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
}

// ---------------------------------------------------------------------------
// Chat-session background execution
// ---------------------------------------------------------------------------

/// Run a command in a dedicated tmux window (`de-bg-*`) on the daemon host.
///
/// Returns **immediately** after sending the command.  A background task
/// monitors for completion via two paths:
///
/// - **Path A — pane died**: the shell exited (`pane-died` hook → `BG_DONE_TX`
///   broadcast).  Output is captured, a `[Background Task Completed]` context
///   message is injected, and the window is GC'd.
/// - **Path B — exit marker found**: the command finished but the shell is still
///   alive.  A `DAEMONEYE_EXIT_<id>:<N>` marker appended to the command detects
///   this by scanning the pane scrollback every second.  Output is captured,
///   context is injected, and the window is left open for follow-up commands.
///
/// The AI receives `[Background Task Completed]` asynchronously in its next
/// turn.  The returned string includes the pane ID so the AI can direct
/// follow-up commands there via `target="<pane_id>"`.
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

    let pane_id = match tmux::create_job_window(session, &win_name) {
        Ok(p) => p,
        Err(e) => return format!("Failed to create background window: {}", e),
    };

    let started_at = tokio::time::Instant::now();

    // remain-on-exit lets us query pane_dead_status on shell crash (fallback path).
    if let Err(e) = tmux::set_remain_on_exit(&pane_id, true) {
        log::warn!("Failed to set remain-on-exit for {}: {}", win_name, e);
    }

    // Detect the shell to select the right exit-code variable.
    let shell_name = tmux::pane_current_command(&pane_id).unwrap_or_default();
    let exit_var = shell_exit_var(&shell_name);

    // Wrap the command so it notifies the daemon on completion via IPC.
    // The shell stays alive for follow-up commands (no `exit`).
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());
    let notify = format!(
        "{exe} notify complete {pane_id} $__de_ec {session}",
        pane_id = pane_id,
        session = shell_escape_arg(session),
    );
    let wrapped = if exit_var == "$status" {
        // fish: use set to capture status before running notify
        format!("{cmd}; set __de_ec $status; {notify}")
    } else {
        // bash / zsh / sh / ksh / dash / ...
        format!("{cmd}; __de_ec=$?; {notify}")
    };

    if let Err(e) = tmux::send_keys(&pane_id, &wrapped) {
        let _ = tmux::kill_job_window(session, &win_name);
        return format!("Failed to send command to window: {}", e);
    }

    // Inject sudo credential synchronously (≤10 s); must happen before we return.
    if let Some(cred) = credential {
        let poll = Duration::from_millis(200);
        let prompt_timeout = Duration::from_secs(10);
        let mut waited = Duration::ZERO;
        loop {
            tokio::time::sleep(poll).await;
            waited += poll;
            let snap = tmux::capture_pane(&pane_id, 50).unwrap_or_default();
            if snap.contains("password") || snap.contains("Password") || snap.contains("[sudo]") {
                let _ = tmux::send_keys(&pane_id, cred);
                break;
            }
            if waited >= prompt_timeout || tmux::pane_dead_status(&pane_id).is_some() {
                break;
            }
        }
    }

    // Register in the session's bg_windows list (cap enforcement runs in executor).
    if let Some(ref sid) = session_id {
        if let Ok(mut store) = sessions.lock() {
            if let Some(entry) = store.get_mut(sid) {
                entry.bg_windows.push(BgWindowInfo {
                    pane_id: pane_id.clone(),
                    window_name: win_name.clone(),
                    command: cmd.to_string(),
                    tmux_session: session.to_string(),
                    started_at: std::time::Instant::now(),
                    exit_code: None,
                });
            }
        }
    }

    log_event("job_start", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "job_id": id_short,
        "job_name": win_name,
        "pane": pane_id,
    }));

    // Spawn completion monitor.
    let pane_id_bg    = pane_id.clone();
    let win_name_bg   = win_name.clone();
    let cmd_bg        = cmd.to_string();
    let session_bg    = session.to_string();
    let session_id_bg = session_id.clone();
    let sessions_bg   = sessions.clone();

    tokio::spawn(async move {
        // Subscribe before any await so we don't miss an early signal.
        let mut complete_rx = complete_subscribe();
        let mut died_rx     = bg_done_subscribe();

        // Primary path: IPC signal from the command wrapper carries the exit code.
        // Fallback path: pane-died hook fires when the shell itself crashes or exits.
        let (exit_code, pane_persists) = tokio::time::timeout(
            Duration::from_secs(3600),
            async {
                loop {
                    tokio::select! {
                        result = complete_rx.recv() => {
                            if let Ok((pid, code)) = result {
                                if pid == pane_id_bg { return (code, true); }
                            }
                        }
                        result = died_rx.recv() => {
                            if let Ok(pid) = result {
                                if pid == pane_id_bg {
                                    let code = tmux::pane_dead_status(&pane_id_bg).unwrap_or(-1);
                                    return (code, false);
                                }
                            }
                        }
                    }
                }
            }
        ).await.unwrap_or((124, false));

        let duration_ms = started_at.elapsed().as_millis() as u64;
        let body = capture_and_archive(&pane_id_bg, &win_name_bg);

        log_event("job_complete", serde_json::json!({
            "session": session_id_bg.as_deref().unwrap_or("-"),
            "job_name": win_name_bg,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
            "pane_persists": pane_persists,
        }));

        if let Some(ref sid) = session_id_bg {
            notify_session(&sessions_bg, sid, &pane_id_bg, &cmd_bg, &win_name_bg, exit_code, &body, pane_persists);
        }

        if !pane_persists {
            // Path A / timeout: window is dead or timed out — clean it up.
            let reason = if exit_code == 124 { "timeout" } else { "pane-died" };
            log_event("gc_window", serde_json::json!({
                "session": session_id_bg.as_deref().unwrap_or("-"),
                "win_name": win_name_bg,
                "reason": reason,
            }));
            if let Err(e) = tmux::kill_job_window(&session_bg, &win_name_bg) {
                log::error!("Failed to GC dead bg window {}: {}", win_name_bg, e);
            }
            // Remove from bg_windows — window no longer exists.
            if let Some(ref sid) = session_id_bg {
                if let Ok(mut store) = sessions_bg.lock() {
                    if let Some(entry) = store.get_mut(sid) {
                        entry.bg_windows.retain(|w| w.pane_id != pane_id_bg);
                    }
                }
            }
        }
        // Path B: window persists — leave it in bg_windows for reuse / eviction.
    });

    // Return immediately with pane coordinates.
    format!(
        "Background command sent to pane {pane_id} (window {win_name}). \
         You will receive a [Background Task Completed] context message when it finishes. \
         Use target=\"{pane_id}\" to run follow-up commands in the same shell."
    )
}

// ---------------------------------------------------------------------------
// Retry via respawn-pane (N11)
// ---------------------------------------------------------------------------

/// Re-run a command in an existing background pane using `tmux respawn-pane`.
///
/// Unlike [`run_background_in_window`], this does NOT create a new tmux window.
/// It respawns a fresh shell in the existing pane (`-k` kills any running process),
/// then sends the wrapped command.  The pane's scrollback is preserved, so the
/// AI can see both the original failure output and the retry output in the same
/// window.  Useful when the AI wants to retry a failed background command without
/// cluttering the session with extra windows.
///
/// `pane_id` must be a valid, existing pane (caller verifies via `tmux::pane_exists`).
/// `win_name` is the existing window name (used for logging and archive paths).
pub async fn respawn_background_in_pane(
    pane_id: &str,
    win_name: &str,
    cmd: &str,
    session: &str,
    session_id: Option<String>,
    sessions: SessionStore,
) -> String {
    // Respawn: start a fresh shell in the pane, killing anything running.
    let respawn_status = std::process::Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", pane_id])
        .status();
    if !respawn_status.map(|s| s.success()).unwrap_or(false) {
        return format!("Error: failed to respawn pane {} (pane may no longer exist)", pane_id);
    }

    // Brief yield so tmux can start the shell before we query it.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started_at = tokio::time::Instant::now();

    // Detect shell for exit-code variable selection.
    let shell_name = tmux::pane_current_command(pane_id).unwrap_or_default();
    let exit_var = shell_exit_var(&shell_name);

    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());
    let notify = format!(
        "{exe} notify complete {pane_id} $__de_ec {session}",
        pane_id = pane_id,
        session = shell_escape_arg(session),
    );
    let wrapped = if exit_var == "$status" {
        format!("{cmd}; set __de_ec $status; {notify}")
    } else {
        format!("{cmd}; __de_ec=$?; {notify}")
    };

    if let Err(e) = tmux::send_keys(pane_id, &wrapped) {
        return format!("Error: failed to send retry command to pane {}: {}", pane_id, e);
    }

    // Reset exit_code in bg_windows so the session knows it's running again.
    if let Some(ref sid) = session_id {
        if let Ok(mut store) = sessions.lock() {
            if let Some(entry) = store.get_mut(sid) {
                if let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id) {
                    w.exit_code = None;
                    w.command = cmd.to_string();
                }
            }
        }
    }

    log_event("job_retry", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "pane": pane_id,
        "win_name": win_name,
    }));

    // Spawn completion monitor (same logic as the original run).
    let pane_id_bg    = pane_id.to_string();
    let win_name_bg   = win_name.to_string();
    let cmd_bg        = cmd.to_string();
    let session_bg    = session.to_string();
    let session_id_bg = session_id.clone();
    let sessions_bg   = sessions.clone();

    tokio::spawn(async move {
        let mut complete_rx = complete_subscribe();
        let mut died_rx     = bg_done_subscribe();

        let (exit_code, pane_persists) = tokio::time::timeout(
            Duration::from_secs(3600),
            async {
                loop {
                    tokio::select! {
                        result = complete_rx.recv() => {
                            if let Ok((pid, code)) = result {
                                if pid == pane_id_bg { return (code, true); }
                            }
                        }
                        result = died_rx.recv() => {
                            if let Ok(pid) = result {
                                if pid == pane_id_bg {
                                    let code = tmux::pane_dead_status(&pane_id_bg).unwrap_or(-1);
                                    return (code, false);
                                }
                            }
                        }
                    }
                }
            }
        ).await.unwrap_or((124, false));

        let body = capture_and_archive(&pane_id_bg, &win_name_bg);

        log_event("job_complete", serde_json::json!({
            "session": session_id_bg.as_deref().unwrap_or("-"),
            "job_name": win_name_bg,
            "exit_code": exit_code,
            "duration_ms": started_at.elapsed().as_millis() as u64,
            "pane_persists": pane_persists,
            "retry": true,
        }));

        if let Some(ref sid) = session_id_bg {
            notify_session(&sessions_bg, sid, &pane_id_bg, &cmd_bg, &win_name_bg, exit_code, &body, pane_persists);
        }

        if !pane_persists {
            if let Err(e) = tmux::kill_job_window(&session_bg, &win_name_bg) {
                log::error!("Failed to GC retried bg window {}: {}", win_name_bg, e);
            }
            if let Some(ref sid) = session_id_bg {
                if let Ok(mut store) = sessions_bg.lock() {
                    if let Some(entry) = store.get_mut(sid) {
                        entry.bg_windows.retain(|w| w.pane_id != pane_id_bg);
                    }
                }
            }
        }
    });

    format!(
        "Retry command sent to existing pane {pane_id} (window {win_name}). \
         The previous output remains visible in scrollback above the new run. \
         You will receive a [Background Task Completed] message when the retry finishes."
    )
}

// ---------------------------------------------------------------------------
// Scheduled / watchdog job completion handler
// ---------------------------------------------------------------------------

/// Completion handler for scheduled and watchdog jobs (called from `server.rs`).
///
/// - Captures and archives pane output.
/// - Sends a `SystemMsg` notification to any listening chat client.
/// - **GC**: destroys the window on success (FR-1.2.10); leaves it open on
///   failure so the user can inspect it via `daemoneye schedule windows`.
pub async fn notify_job_completion(
    pane_id: String,
    cmd: String,
    win_name: String,
    session: String,
    exit_code: i32,
    session_id: Option<String>,
    _sessions: SessionStore,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
    started_at: tokio::time::Instant,
) {
    let duration_ms = started_at.elapsed().as_millis() as u64;

    log_event("job_complete", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "job_id": win_name.split('-').last().unwrap_or(""),
        "job_name": win_name,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
    }));

    // Archive logs.
    let logs_dir = crate::config::config_dir().join("pane_logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        log::error!("Failed to create pane_logs directory: {}", e);
    } else if let Err(e) = tmux::pane::capture_pane_to_file(&pane_id, &logs_dir.join(format!("{}.log", win_name))) {
        log::error!("Failed to archive pane logs for {}: {}", win_name, e);
    }

    let status_word = if exit_code == 0 { "succeeded" } else { "failed" };
    let alert_msg = format!("`{}` {} in pane {}", cmd, status_word, pane_id);

    if let Some(ref tx) = notify_tx {
        let _ = tx.send(Response::SystemMsg(alert_msg));
    }

    // FR-1.2.10: destroy on success, leave open on failure for inspection.
    if exit_code == 0 {
        log_event("gc_window", serde_json::json!({
            "session": session_id.as_deref().unwrap_or("-"),
            "win_name": win_name,
            "reason": "done",
        }));
        if let Err(e) = tmux::kill_job_window(&session, &win_name) {
            log::error!("Failed to GC scheduled job window {}: {}", win_name, e);
        }
    }
    // On failure: leave open indefinitely. User closes manually or via
    // `daemoneye schedule windows`.
}
