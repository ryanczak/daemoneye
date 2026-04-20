use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use crate::daemon::session::{
    BgWindowInfo, SessionStore, append_session_message, bg_done_subscribe, complete_subscribe,
};
use crate::daemon::utils::{
    command_has_sudo, is_fingerprint_prompt, log_event, normalize_output, shell_escape_arg,
    sudo_auth_failed, wait_for_sudo_prompt_and_inject,
};
use crate::ipc::Response;
use crate::tmux;
use crate::util::UnpoisonExt;
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

/// Maximum bytes of command output passed inline to the AI.
/// Outputs larger than this are trimmed to head + tail with an omission note.
const OUTPUT_INLINE_LIMIT: usize = 50_000;

/// Trim `raw` to at most `limit` bytes, preserving the first and last halves
/// and inserting an omission note with the archive path in between.
///
/// Splits are rounded to newline boundaries so lines are never cut mid-stream.
fn trim_large_output(raw: &str, limit: usize, win_name: &str) -> String {
    if raw.len() <= limit {
        return raw.to_string();
    }
    let half = limit / 2;

    // Head: up to `half` bytes, rounded down to the last newline.
    let head_end = raw[..half].rfind('\n').map(|i| i + 1).unwrap_or(half);
    let head = &raw[..head_end];

    // Tail: last `half` bytes, rounded up to the next newline.
    let tail_raw_start = raw.len() - half;
    let tail_start = raw[tail_raw_start..]
        .find('\n')
        .map(|i| tail_raw_start + i + 1)
        .unwrap_or(tail_raw_start);
    let tail = &raw[tail_start..];

    let omitted = raw
        .lines()
        .count()
        .saturating_sub(head.lines().count())
        .saturating_sub(tail.lines().count());

    // Use the absolute path so the agent can pass it directly to read_file.
    let archive = crate::config::pane_logs_dir()
        .join(format!("{}.log", win_name))
        .to_string_lossy()
        .to_string();
    // head already ends with '\n'; trim it so the format string doesn't insert a blank line.
    let head = head.trim_end_matches('\n');
    format!("{head}\n... ({omitted} lines omitted — full log: {archive}) ...\n{tail}")
}

/// Capture and mask pane output, archive the full output to `var/log/panes/`.
/// Returns the masked body string suitable for the AI.
///
/// `pipe_log` — path to the pipe-pane log file started before the command ran.
/// When present it is read directly (no scrollback cap) and then deleted.
/// Falls back to `capture_pane` if the file cannot be read.
///
/// The archive at `~/.daemoneye/var/log/panes/{win_name}.log` always uses the
/// best available content: the full pipe-log when present, otherwise the
/// scrollback-limited `capture_pane_to_file` fallback.  Ghost shell pane logs
/// are therefore never truncated due to scrollback limits.
fn capture_and_archive(
    pane_id: &str,
    win_name: &str,
    pipe_log: Option<std::path::PathBuf>,
) -> String {
    // Fix B: prefer pipe log over scrollback-limited capture_pane.
    let have_pipe_log = pipe_log.is_some();
    let raw = match pipe_log {
        Some(ref log_path) => match std::fs::read_to_string(log_path) {
            Ok(content) => {
                let _ = std::fs::remove_file(log_path);
                content
            }
            Err(e) => {
                log::warn!(
                    "Failed to read pipe log {:?}: {} — falling back to capture_pane",
                    log_path,
                    e
                );
                let _ = std::fs::remove_file(log_path);
                tmux::capture_pane(pane_id, 5000).unwrap_or_default()
            }
        },
        None => tmux::capture_pane(pane_id, 5000).unwrap_or_default(),
    };
    let trimmed = trim_large_output(&raw, OUTPUT_INLINE_LIMIT, win_name);
    let normalized = normalize_output(&trimmed);
    let body = if normalized.is_empty() {
        "(no output)".to_string()
    } else {
        mask_sensitive(&normalized)
    };
    let logs_dir = crate::config::pane_logs_dir();
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        log::warn!(
            "Failed to create pane_logs dir {}: {}",
            logs_dir.display(),
            e
        );
    } else {
        let archive_path = logs_dir.join(format!("{}.log", win_name));
        // When we have the full pipe-log content in `raw`, write it directly to the
        // archive so ghost shell pane logs are never truncated by scrollback limits.
        // Fall back to capture_pane_to_file only when no pipe log was available.
        if have_pipe_log && !raw.is_empty() {
            if let Err(e) = std::fs::write(&archive_path, raw.as_bytes()) {
                log::warn!("Failed to archive pane log for {}: {}", win_name, e);
            }
        } else if let Err(e) = tmux::pane::capture_pane_to_file(pane_id, &archive_path) {
            log::warn!("Failed to archive pane log for {}: {}", win_name, e);
        }
    }
    body
}

struct BgJobInfo<'a> {
    pane_id: &'a str,
    cmd: &'a str,
    win_name: &'a str,
    exit_code: i32,
    body: &'a str,
    pane_persists: bool,
}

/// Inject a `[Background Task Completed]` message into the session history,
/// update `exit_code` in `bg_windows`, and flash a `tmux display-message`.
///
/// `pane_persists` — if true, the window is still open and the AI can reuse it.
fn notify_session(sessions: &SessionStore, session_id: &str, job: BgJobInfo<'_>) {
    let BgJobInfo {
        pane_id,
        cmd,
        win_name,
        exit_code,
        body,
        pane_persists,
    } = job;
    let Ok(mut store) = sessions.lock() else {
        return;
    };
    let Some(entry) = store.get_mut(session_id) else {
        return;
    };

    // Update exit_code in the bg_windows registry.
    if let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id) {
        w.exit_code = Some(exit_code);
    }

    let persist_note = if pane_persists {
        format!(
            "The window is still open (pane {pane_id}). \
             Use target=\"{pane_id}\" to run follow-up commands in the same shell. \
             Call close_background_window(\"{pane_id}\") when you are done with this window."
        )
    } else {
        format!("The window was closed. Full log: ~/.daemoneye/var/log/panes/{win_name}.log")
    };

    let hints = crate::manifest::related_knowledge_hints(body);
    let hints_section = if !hints.is_empty() {
        format!("\n{}", hints)
    } else {
        String::new()
    };
    let history_content = format!(
        "Background command `{cmd}` in window {win_name} finished with exit code {exit_code}.\n\
         {persist_note}\n<output>\n{body}\n</output>{hints_section}"
    );
    let completion_msg = Message {
        role: "user".to_string(),
        content: format!("[Background Task Completed]\n{}", history_content),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    append_session_message(session_id, &completion_msg);
    entry.messages.push(completion_msg);

    let status_word = if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
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
use std::sync::Mutex;

pub static BG_COMMAND_MAP: std::sync::OnceLock<Mutex<std::collections::HashMap<String, usize>>> =
    std::sync::OnceLock::new();

pub async fn run_background_in_window(
    session: &str,
    _tool_id: &str,
    cmd_id: usize,
    cmd: &str,
    credential: Option<&str>,
    session_id: Option<String>,
    sessions: SessionStore,
) -> String {
    let prefix = if let Some(sid) = &session_id {
        if sid.starts_with("ghost-") {
            // Use the prefix registered on the session entry so webhook-triggered,
            // scheduler-triggered and interactive ghost shells get distinct prefixes.
            let store = sessions.lock().unwrap_or_log();
            store
                .get(sid.as_str())
                .map(|e| e.ghost_bg_prefix)
                .unwrap_or(crate::daemon::GS_BG_WINDOW_PREFIX)
        } else {
            crate::daemon::BG_WINDOW_PREFIX
        }
    } else {
        crate::daemon::BG_WINDOW_PREFIX
    };

    // Create the window with a temporary name first; we need the pane ID
    // (returned by create_job_window) to build the final name.
    let unix_ts = chrono::Utc::now().timestamp();
    let temp_name = format!("{}tmp-{}", prefix, unix_ts);

    let pane_id = match tmux::create_job_window(session, &temp_name) {
        Ok(p) => p,
        Err(e) => return format!("Failed to create background window: {}", e),
    };

    // Build final name: prefix + pane-number + unix-ts + command-slug.
    let pane_num = pane_id.trim_start_matches('%');
    let cmd_slug = crate::daemon::utils::sanitize_cmd_for_window(cmd, 30);
    let final_name = format!("{}{}-{}-{}", prefix, pane_num, unix_ts, cmd_slug);
    let win_name = match tmux::rename_window(session, &temp_name, &final_name) {
        Ok(()) => final_name,
        Err(e) => {
            log::warn!(
                "Failed to rename bg window {} -> {}: {}",
                temp_name,
                final_name,
                e
            );
            temp_name
        }
    };

    if let Ok(mut map) = BG_COMMAND_MAP
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
    {
        map.insert(pane_id.clone(), cmd_id);
    }

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
    //
    // On Linux, if the binary was replaced after the daemon started (e.g. a
    // `cargo build` while the daemon runs), the kernel appends " (deleted)"
    // to the /proc/self/exe path returned by current_exe().  Strip it so the
    // notify call remains valid — the original path still resolves on disk.
    // Then shell-quote the path to handle any spaces in the binary location.
    let exe_raw = std::env::current_exe()
        .map(|p| {
            p.to_string_lossy()
                .trim_end_matches(" (deleted)")
                .to_string()
        })
        .unwrap_or_else(|_| "daemoneye".to_string());
    let exe = shell_escape_arg(&exe_raw);
    let notify = format!(
        "{exe} notify complete {pane_id} $__de_ec {session}",
        pane_id = pane_id,
        session = shell_escape_arg(session),
    );
    // P5: inject a locale-independent sentinel as the sudo password prompt so
    // credential detection below does not rely on translated "password" strings.
    // Only applied when a credential will actually be injected.
    let sentineled_cmd;
    let cmd: &str = if command_has_sudo(cmd) && credential.is_some() {
        sentineled_cmd = format!("SUDO_PROMPT='[de-sudo-prompt]' {cmd}");
        &sentineled_cmd
    } else {
        cmd
    };

    let wrapped = if exit_var == "$status" {
        // fish: use set to capture status before running notify
        format!("{cmd}; set __de_ec $status; {notify}")
    } else {
        // bash / zsh / sh / ksh / dash / ...
        format!("{cmd}; __de_ec=$?; {notify}")
    };

    // Fix A: subscribe to completion channels BEFORE send_keys so a fast-completing
    // command cannot fire its signal before the monitor has subscribed.
    let mut complete_rx = complete_subscribe();
    let mut died_rx = bg_done_subscribe();

    // Fix B: start pipe-pane BEFORE the command fires to capture all output without
    // any scrollback cap.  Falls back silently if pipe-pane isn't available.
    let pipe_log = tmux::start_pipe_pane(&pane_id)
        .map_err(|e| log::warn!("Failed to start pipe-pane for {}: {}", pane_id, e))
        .ok();

    if let Err(e) = tmux::send_keys(&pane_id, &wrapped) {
        if pipe_log.is_some() {
            tmux::stop_pipe_pane(&pane_id);
        }
        let _ = tmux::kill_job_window(session, &win_name);
        return format!("Failed to send command to window: {}", e);
    }

    // Inject sudo credential synchronously (≤10 s); must happen before we return.
    // P3: detect auth failure and log a warning — the failed exit code propagates
    // through the completion monitor and will be visible to the AI.
    if let Some(cred) = credential {
        if wait_for_sudo_prompt_and_inject(&pane_id, cred).await {
            if sudo_auth_failed(&pane_id).await {
                log::warn!(
                    "sudo authentication failed for background command in {}: {}",
                    pane_id,
                    cmd
                );
            }
        } else {
            // Distinguish fingerprint-reader failures from plain timeouts so the
            // AI receives an actionable error rather than just a non-zero exit code.
            let snap = crate::tmux::capture_pane(&pane_id, 10).unwrap_or_default();
            if is_fingerprint_prompt(&snap) {
                log::warn!(
                    "sudo fingerprint auth not supported in background panes ({}): {}",
                    pane_id,
                    cmd
                );
                let _ = crate::tmux::kill_job_window(session, &win_name);
                return "sudo failed: fingerprint authentication is not supported in background \
                     panes — the pane has no TTY for a reader interaction. \
                     Use `daemoneye install-sudoers <script-name>` to create a NOPASSWD \
                     sudoers rule for this command, or run it in a foreground pane."
                    .to_string();
            }
            log::warn!(
                "sudo prompt not detected for background command in {}: {}",
                pane_id,
                cmd
            );
        }
    }

    // Register in the session's bg_windows list (cap enforcement runs in executor).
    if let Some(ref sid) = session_id
        && let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(sid)
    {
        entry.bg_windows.push(BgWindowInfo {
            pane_id: pane_id.clone(),
            window_name: win_name.clone(),
            tmux_session: session.to_string(),
            exit_code: None,
        });
    }

    log_event(
        "job_start",
        serde_json::json!({
            "session": session_id.as_deref().unwrap_or("-"),
            "job_name": win_name,
            "pane": pane_id,
        }),
    );

    // Inline completion wait (3 s): the async block borrows complete_rx / died_rx
    // by &mut without moving them, so after .await the receivers are still owned
    // here and can be moved into the async monitor on the slow path.
    // Fast commands (like `df -h`) complete in ~0 ms because Fix A ensures the
    // broadcast message is already buffered by the time we poll.
    let inline = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            tokio::select! {
                result = complete_rx.recv() => {
                    if let Ok((pid, code)) = result
                        && pid == pane_id { return (code, true); }
                }
                result = died_rx.recv() => {
                    if let Ok(pid) = result
                        && pid == pane_id {
                            let code = tmux::pane_dead_status(&pane_id).unwrap_or(-1);
                            return (code, false);
                        }
                }
            }
        }
    })
    .await;

    match inline {
        Ok((exit_code, pane_persists)) => {
            // Fast path: command finished within 3 s — return output inline as the
            // tool result.  Do NOT call notify_session; the output is already here.
            if pipe_log.is_some() {
                let _ = std::process::Command::new("tmux")
                    .args(["pipe-pane", "-t", &pane_id])
                    .output();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let body = capture_and_archive(&pane_id, &win_name, pipe_log);

            log_event(
                "job_complete",
                serde_json::json!({
                    "session": session_id.as_deref().unwrap_or("-"),
                    "job_name": win_name,
                    "exit_code": exit_code,
                    "duration_ms": started_at.elapsed().as_millis() as u64,
                    "pane_persists": pane_persists,
                }),
            );

            // Update exit_code in bg_windows.
            if let Some(ref sid) = session_id
                && let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(sid)
                && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
            {
                w.exit_code = Some(exit_code);
            }

            if !pane_persists {
                let reason = if exit_code == 124 {
                    "timeout"
                } else {
                    "pane-died"
                };
                log_event(
                    "gc_window",
                    serde_json::json!({
                        "session": session_id.as_deref().unwrap_or("-"),
                        "win_name": win_name,
                        "reason": reason,
                    }),
                );
                if let Err(e) = tmux::kill_job_window(session, &win_name) {
                    log::error!("Failed to GC dead bg window {}: {}", win_name, e);
                }
                if let Some(ref sid) = session_id
                    && let Ok(mut store) = sessions.lock()
                    && let Some(entry) = store.get_mut(sid)
                {
                    entry.bg_windows.retain(|w| w.pane_id != pane_id);
                }
            }

            let persist_note = if pane_persists {
                format!(
                    "The window is still open (pane {pane_id}). \
                     Use target=\"{pane_id}\" to run follow-up commands in the same shell."
                )
            } else {
                format!(
                    "The window was closed. Full log: ~/.daemoneye/var/log/panes/{win_name}.log"
                )
            };
            format!(
                "Background command completed (exit {exit_code}).\n{persist_note}\n<output>\n{body}\n</output>"
            )
        }
        Err(_elapsed) => {
            // Slow path: command still running after 3 s.
            // Borrows on complete_rx / died_rx ended when the timeout future was dropped;
            // move them into the async monitor.
            let pane_id_bg = pane_id.clone();
            let win_name_bg = win_name.clone();
            let cmd_bg = cmd.to_string();
            let session_bg = session.to_string();
            let session_id_bg = session_id.clone();
            let sessions_bg = sessions.clone();

            tokio::spawn(async move {
                let mut complete_rx = complete_rx;
                let mut died_rx = died_rx;

                let (exit_code, pane_persists) = tokio::time::timeout(
                    Duration::from_secs(3600),
                    async {
                        loop {
                            tokio::select! {
                                result = complete_rx.recv() => {
                                    if let Ok((pid, code)) = result
                                        && pid == pane_id_bg { return (code, true); }
                                }
                                result = died_rx.recv() => {
                                    if let Ok(pid) = result
                                        && pid == pane_id_bg {
                                            let code = tmux::pane_dead_status(&pane_id_bg).unwrap_or(-1);
                                            return (code, false);
                                        }
                                }
                            }
                        }
                    }
                ).await.unwrap_or((124, false));

                if pipe_log.is_some() {
                    let _ = std::process::Command::new("tmux")
                        .args(["pipe-pane", "-t", &pane_id_bg])
                        .output();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }

                let duration_ms = started_at.elapsed().as_millis() as u64;
                let body = capture_and_archive(&pane_id_bg, &win_name_bg, pipe_log);

                log_event(
                    "job_complete",
                    serde_json::json!({
                        "session": session_id_bg.as_deref().unwrap_or("-"),
                        "job_name": win_name_bg,
                        "exit_code": exit_code,
                        "duration_ms": duration_ms,
                        "pane_persists": pane_persists,
                    }),
                );

                if let Some(ref sid) = session_id_bg {
                    notify_session(
                        &sessions_bg,
                        sid,
                        BgJobInfo {
                            pane_id: &pane_id_bg,
                            cmd: &cmd_bg,
                            win_name: &win_name_bg,
                            exit_code,
                            body: &body,
                            pane_persists,
                        },
                    );
                }

                if !pane_persists {
                    let reason = if exit_code == 124 {
                        "timeout"
                    } else {
                        "pane-died"
                    };
                    log_event(
                        "gc_window",
                        serde_json::json!({
                            "session": session_id_bg.as_deref().unwrap_or("-"),
                            "win_name": win_name_bg,
                            "reason": reason,
                        }),
                    );
                    if let Err(e) = tmux::kill_job_window(&session_bg, &win_name_bg) {
                        log::error!("Failed to GC dead bg window {}: {}", win_name_bg, e);
                    }
                    if let Some(ref sid) = session_id_bg
                        && let Ok(mut store) = sessions_bg.lock()
                        && let Some(entry) = store.get_mut(sid)
                    {
                        entry.bg_windows.retain(|w| w.pane_id != pane_id_bg);
                    }
                }
            });

            format!(
                "Background command sent to pane {pane_id} (window {win_name}). \
                 You will receive a [Background Task Completed] context message when it finishes. \
                 Use target=\"{pane_id}\" to run follow-up commands in the same shell."
            )
        }
    }
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
    cmd_id: usize,
    cmd: &str,
    session: &str,
    session_id: Option<String>,
    sessions: SessionStore,
) -> String {
    if let Ok(mut map) = BG_COMMAND_MAP
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
    {
        map.insert(pane_id.to_string(), cmd_id);
    }
    // Respawn: start a fresh shell in the pane, killing anything running.
    let respawn_status = std::process::Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", pane_id])
        .status();
    if !respawn_status.map(|s| s.success()).unwrap_or(false) {
        return format!(
            "Error: failed to respawn pane {} (pane may no longer exist)",
            pane_id
        );
    }

    // Brief yield so tmux can start the shell before we query it.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started_at = tokio::time::Instant::now();

    // Detect shell for exit-code variable selection.
    let shell_name = tmux::pane_current_command(pane_id).unwrap_or_default();
    let exit_var = shell_exit_var(&shell_name);

    let exe_raw = std::env::current_exe()
        .map(|p| {
            p.to_string_lossy()
                .trim_end_matches(" (deleted)")
                .to_string()
        })
        .unwrap_or_else(|_| "daemoneye".to_string());
    let exe = shell_escape_arg(&exe_raw);
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

    // Fix A: subscribe before send_keys.
    let mut complete_rx = complete_subscribe();
    let mut died_rx = bg_done_subscribe();

    // Fix B: clean up any leftover pipe log from the previous run of this pane,
    // then start a fresh pipe before the command fires.
    let _ = std::fs::remove_file(tmux::pipe_log_path(pane_id));
    let pipe_log = tmux::start_pipe_pane(pane_id)
        .map_err(|e| log::warn!("Failed to start pipe-pane for retry on {}: {}", pane_id, e))
        .ok();

    if let Err(e) = tmux::send_keys(pane_id, &wrapped) {
        if pipe_log.is_some() {
            tmux::stop_pipe_pane(pane_id);
        }
        return format!(
            "Error: failed to send retry command to pane {}: {}",
            pane_id, e
        );
    }

    // Reset exit_code in bg_windows so the session knows it's running again.
    if let Some(ref sid) = session_id
        && let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(sid)
        && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
    {
        w.exit_code = None;
    }

    log_event(
        "job_retry",
        serde_json::json!({
            "session": session_id.as_deref().unwrap_or("-"),
            "pane": pane_id,
            "win_name": win_name,
        }),
    );

    // Inline completion wait (3 s): same borrow-not-move pattern as run_background_in_window.
    let pane_id_str = pane_id.to_string();
    let inline = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            tokio::select! {
                result = complete_rx.recv() => {
                    if let Ok((pid, code)) = result
                        && pid == pane_id_str { return (code, true); }
                }
                result = died_rx.recv() => {
                    if let Ok(pid) = result
                        && pid == pane_id_str {
                            let code = tmux::pane_dead_status(&pane_id_str).unwrap_or(-1);
                            return (code, false);
                        }
                }
            }
        }
    })
    .await;

    match inline {
        Ok((exit_code, pane_persists)) => {
            // Fast path: retry finished within 3 s — return output inline.
            if pipe_log.is_some() {
                let _ = std::process::Command::new("tmux")
                    .args(["pipe-pane", "-t", pane_id])
                    .output();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let body = capture_and_archive(pane_id, win_name, pipe_log);

            log_event(
                "job_complete",
                serde_json::json!({
                    "session": session_id.as_deref().unwrap_or("-"),
                    "job_name": win_name,
                    "exit_code": exit_code,
                    "duration_ms": started_at.elapsed().as_millis() as u64,
                    "pane_persists": pane_persists,
                    "retry": true,
                }),
            );

            if let Some(ref sid) = session_id
                && let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(sid)
                && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
            {
                w.exit_code = Some(exit_code);
            }

            if !pane_persists {
                let reason = if exit_code == 124 {
                    "timeout"
                } else {
                    "pane-died"
                };
                log_event(
                    "gc_window",
                    serde_json::json!({
                        "session": session_id.as_deref().unwrap_or("-"),
                        "win_name": win_name,
                        "reason": reason,
                    }),
                );
                if let Err(e) = tmux::kill_job_window(session, win_name) {
                    log::error!("Failed to GC retried bg window {}: {}", win_name, e);
                }
                if let Some(ref sid) = session_id
                    && let Ok(mut store) = sessions.lock()
                    && let Some(entry) = store.get_mut(sid)
                {
                    entry.bg_windows.retain(|w| w.pane_id != pane_id);
                }
            }

            let persist_note = if pane_persists {
                format!(
                    "The window is still open (pane {pane_id}). \
                     Use target=\"{pane_id}\" to run follow-up commands in the same shell."
                )
            } else {
                format!(
                    "The window was closed. Full log: ~/.daemoneye/var/log/panes/{win_name}.log"
                )
            };
            format!(
                "Retry command completed (exit {exit_code}).\n{persist_note}\n<output>\n{body}\n</output>"
            )
        }
        Err(_elapsed) => {
            // Slow path: retry still running after 3 s — move receivers to async monitor.
            let pane_id_bg = pane_id.to_string();
            let win_name_bg = win_name.to_string();
            let cmd_bg = cmd.to_string();
            let session_bg = session.to_string();
            let session_id_bg = session_id.clone();
            let sessions_bg = sessions.clone();

            tokio::spawn(async move {
                let mut complete_rx = complete_rx;
                let mut died_rx = died_rx;

                let (exit_code, pane_persists) = tokio::time::timeout(
                    Duration::from_secs(3600),
                    async {
                        loop {
                            tokio::select! {
                                result = complete_rx.recv() => {
                                    if let Ok((pid, code)) = result
                                        && pid == pane_id_bg { return (code, true); }
                                }
                                result = died_rx.recv() => {
                                    if let Ok(pid) = result
                                        && pid == pane_id_bg {
                                            let code = tmux::pane_dead_status(&pane_id_bg).unwrap_or(-1);
                                            return (code, false);
                                        }
                                }
                            }
                        }
                    }
                ).await.unwrap_or((124, false));

                if pipe_log.is_some() {
                    let _ = std::process::Command::new("tmux")
                        .args(["pipe-pane", "-t", &pane_id_bg])
                        .output();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }

                let body = capture_and_archive(&pane_id_bg, &win_name_bg, pipe_log);

                log_event(
                    "job_complete",
                    serde_json::json!({
                        "session": session_id_bg.as_deref().unwrap_or("-"),
                        "job_name": win_name_bg,
                        "exit_code": exit_code,
                        "duration_ms": started_at.elapsed().as_millis() as u64,
                        "pane_persists": pane_persists,
                        "retry": true,
                    }),
                );

                if let Some(ref sid) = session_id_bg {
                    notify_session(
                        &sessions_bg,
                        sid,
                        BgJobInfo {
                            pane_id: &pane_id_bg,
                            cmd: &cmd_bg,
                            win_name: &win_name_bg,
                            exit_code,
                            body: &body,
                            pane_persists,
                        },
                    );
                }

                if !pane_persists {
                    if let Err(e) = tmux::kill_job_window(&session_bg, &win_name_bg) {
                        log::error!("Failed to GC retried bg window {}: {}", win_name_bg, e);
                    }
                    if let Some(ref sid) = session_id_bg
                        && let Ok(mut store) = sessions_bg.lock()
                        && let Some(entry) = store.get_mut(sid)
                    {
                        entry.bg_windows.retain(|w| w.pane_id != pane_id_bg);
                    }
                }
            });

            format!(
                "Retry command sent to existing pane {pane_id} (window {win_name}). \
                 The previous output remains visible in scrollback above the new run. \
                 You will receive a [Background Task Completed] message when the retry finishes."
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduled / watchdog job completion handler
// ---------------------------------------------------------------------------

pub struct OwnedJobInfo {
    pub pane_id: String,
    pub cmd: String,
    pub win_name: String,
}

/// Completion handler for scheduled and watchdog jobs (called from `server.rs`).
///
/// - Captures and archives pane output.
/// - Sends a `SystemMsg` notification to any listening chat client.
/// - **GC**: destroys the window on success (FR-1.2.10); leaves it open on
///   failure so the user can inspect it via `daemoneye schedule windows`.
pub async fn notify_job_completion(
    job: OwnedJobInfo,
    session: String,
    exit_code: i32,
    session_id: Option<String>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
    started_at: tokio::time::Instant,
) {
    let OwnedJobInfo {
        pane_id,
        cmd,
        win_name,
    } = job;
    let duration_ms = started_at.elapsed().as_millis() as u64;

    log_event(
        "job_complete",
        serde_json::json!({
            "session": session_id.as_deref().unwrap_or("-"),
            "job_id": win_name.split('-').next_back().unwrap_or(""),
            "job_name": win_name,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
        }),
    );

    // Archive logs.
    let logs_dir = crate::config::pane_logs_dir();
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        log::error!("Failed to create pane_logs directory: {}", e);
    } else if let Err(e) =
        tmux::pane::capture_pane_to_file(&pane_id, &logs_dir.join(format!("{}.log", win_name)))
    {
        log::error!("Failed to archive pane logs for {}: {}", win_name, e);
    }

    let status_word = if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
    let alert_msg = format!("`{}` {} in pane {}", cmd, status_word, pane_id);

    if let Some(ref tx) = notify_tx {
        let _ = tx.send(Response::SystemMsg(alert_msg));
    }

    // FR-1.2.10: destroy on success, leave open on failure for inspection.
    if exit_code == 0 {
        log_event(
            "gc_window",
            serde_json::json!({
                "session": session_id.as_deref().unwrap_or("-"),
                "win_name": win_name,
                "reason": "done",
            }),
        );
        if let Err(e) = tmux::kill_job_window(&session, &win_name) {
            log::error!("Failed to GC scheduled job window {}: {}", win_name, e);
        }
    }
    // On failure: leave open indefinitely. User closes manually or via
    // `daemoneye schedule windows`.
}

// ---------------------------------------------------------------------------
// Periodic background-window garbage collection
// ---------------------------------------------------------------------------

/// Pane state snapshot used by the GC decision logic.
pub(crate) struct PaneGcInfo {
    /// True when the pane's foreground process has exited (remain-on-exit mode).
    pub dead: bool,
    /// True when the pane is sitting at an idle shell prompt.
    pub idle_shell: bool,
    /// Unix timestamp of the last time the pane produced output.
    pub last_activity: u64,
}

/// How long a dead pane must have been idle before the GC kills its window.
const DEAD_THRESHOLD_SECS: u64 = 30;

/// How long a completed-command pane must sit idle at the shell prompt before
/// the GC reclaims its window.
const IDLE_THRESHOLD_SECS: u64 = 120;

const IDLE_SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "fish", "ksh", "tcsh", "csh"];

/// Pure decision function: given the current tracked windows and live pane
/// state, returns the pane IDs whose windows should be killed.
///
/// Separated from the tmux-calling executor so it can be unit-tested without
/// a running tmux server.
pub(crate) fn plan_gc_actions(
    windows: &[crate::daemon::session::BgWindowInfo],
    pane_info: &std::collections::HashMap<String, PaneGcInfo>,
    now_unix: u64,
) -> Vec<String> {
    let mut to_kill = Vec::new();
    for win in windows {
        match pane_info.get(&win.pane_id) {
            None => {
                // Pane no longer exists in tmux.
                to_kill.push(win.pane_id.clone());
            }
            Some(info) => {
                let elapsed = now_unix.saturating_sub(info.last_activity);
                let should_kill = (info.dead && elapsed > DEAD_THRESHOLD_SECS)
                    || (win.exit_code.is_some()
                        && info.idle_shell
                        && elapsed > IDLE_THRESHOLD_SECS);
                if should_kill {
                    to_kill.push(win.pane_id.clone());
                }
            }
        }
    }
    to_kill
}

/// Prefixes that identify daemon-managed windows, used for orphan detection.
const DAEMON_BG_PREFIXES: &[&str] = &[
    crate::daemon::BG_WINDOW_PREFIX,
    crate::daemon::SCHED_WINDOW_PREFIX,
    crate::daemon::GS_BG_WINDOW_PREFIX,
    crate::daemon::GS_SCHED_WINDOW_PREFIX,
];

/// Periodic garbage collector for background windows.
///
/// Called every 60 seconds by the `bg-window-gc` supervised task.
///
/// For each session's tracked `bg_windows`:
/// - Kills windows whose pane is gone, dead, or has been idle since completing.
///
/// Also scans all tmux panes for daemon-prefixed windows not tracked by any
/// session (orphans from a daemon restart or missed completion signal) and
/// kills those too.
pub fn gc_bg_windows(sessions: &crate::daemon::session::SessionStore) {
    use std::collections::{HashMap, HashSet};

    // One tmux call to get all live panes.
    let all_panes = match crate::tmux::pane::list_panes_detailed() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("gc_bg_windows: failed to list panes: {}", e);
            return;
        }
    };

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Build lookup map and collect daemon-prefixed window names.
    let mut pane_map: HashMap<String, PaneGcInfo> = HashMap::new();
    let mut daemon_windows: HashMap<String, (String, String)> = HashMap::new(); // pane_id -> (session, window_name)

    for p in &all_panes {
        let idle_shell = IDLE_SHELLS.contains(&p.current_cmd.as_str());
        pane_map.insert(
            p.pane_id.clone(),
            PaneGcInfo {
                dead: p.dead,
                idle_shell,
                last_activity: p.last_activity,
            },
        );
        let is_daemon_window = DAEMON_BG_PREFIXES
            .iter()
            .any(|prefix| p.window_name.starts_with(prefix));
        if is_daemon_window {
            daemon_windows.insert(
                p.pane_id.clone(),
                (p.session_name.clone(), p.window_name.clone()),
            );
        }
    }

    let Ok(mut store) = sessions.lock() else {
        return;
    };

    // Track all pane IDs referenced by any session (for orphan detection).
    let mut tracked_pane_ids: HashSet<String> = HashSet::new();

    for (session_id, entry) in store.iter_mut() {
        let to_kill = plan_gc_actions(&entry.bg_windows, &pane_map, now_unix);
        if to_kill.is_empty() {
            for w in &entry.bg_windows {
                tracked_pane_ids.insert(w.pane_id.clone());
            }
            continue;
        }

        for pane_id in &to_kill {
            // Look up window info before removing.
            if let Some(win) = entry.bg_windows.iter().find(|w| &w.pane_id == pane_id) {
                let reason = if pane_map.contains_key(pane_id) {
                    if pane_map[pane_id].dead {
                        "pane_dead"
                    } else {
                        "idle_completed"
                    }
                } else {
                    "pane_gone"
                };
                log_event(
                    "gc_window",
                    serde_json::json!({
                        "session": session_id,
                        "win_name": win.window_name,
                        "pane_id": pane_id,
                        "reason": reason,
                    }),
                );
                if let Err(e) = tmux::kill_job_window(&win.tmux_session, &win.window_name) {
                    log::warn!("gc_bg_windows: failed to kill {}: {}", win.window_name, e);
                }
            }
        }

        entry.bg_windows.retain(|w| {
            let keep = !to_kill.contains(&w.pane_id);
            if keep {
                tracked_pane_ids.insert(w.pane_id.clone());
            }
            keep
        });
    }

    // Orphan sweep: kill daemon-prefixed windows not tracked by any session.
    for (pane_id, (tmux_session, window_name)) in &daemon_windows {
        if !tracked_pane_ids.contains(pane_id) {
            log_event(
                "gc_window",
                serde_json::json!({
                    "session": "-",
                    "win_name": window_name,
                    "pane_id": pane_id,
                    "reason": "orphan",
                }),
            );
            if let Err(e) = tmux::kill_job_window(tmux_session, window_name) {
                log::warn!(
                    "gc_bg_windows: failed to kill orphan {}: {}",
                    window_name,
                    e
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_small_output_unchanged() {
        let s = "line1\nline2\nline3\n";
        assert_eq!(trim_large_output(s, 50_000, "win"), s);
    }

    #[test]
    fn trim_large_output_has_head_and_tail() {
        // Build a string well over the limit.
        let raw: String = (0..1000)
            .map(|i| format!("{:03}: {}\n", i, "x".repeat(94)))
            .collect();
        let limit = 10_000;
        let result = trim_large_output(&raw, limit, "myjob");

        // Head and tail preserved, omission marker present.
        assert!(result.contains("... ("), "expected omission marker");
        assert!(result.contains("myjob.log"), "expected archive path");

        // Result must be smaller than raw.
        assert!(result.len() < raw.len());
    }

    #[test]
    fn trim_output_respects_newline_boundaries() {
        // Each line is exactly 10 bytes including newline.
        let raw: String = (0..200).map(|i| format!("{:09}\n", i)).collect();
        let limit = 500; // 50 lines total budget
        let result = trim_large_output(&raw, limit, "w");

        // No line should be cut mid-stream.
        for line in result.lines() {
            if line.starts_with("...") {
                continue;
            }
            assert!(line.len() == 9, "line cut: {:?}", line);
        }
    }

    #[test]
    fn trim_output_omission_count_is_positive() {
        let raw: String = (0..1000).map(|i| format!("line {}\n", i)).collect();
        let result = trim_large_output(&raw, 2_000, "w");
        // Extract the omission count from the marker line.
        let marker = result.lines().find(|l| l.starts_with("...")).unwrap();
        let count: usize = marker
            .split('(')
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(count > 0);
    }

    // ── plan_gc_actions ───────────────────────────────────────────────────────

    fn make_win(pane_id: &str, exit_code: Option<i32>) -> crate::daemon::session::BgWindowInfo {
        crate::daemon::session::BgWindowInfo {
            pane_id: pane_id.to_string(),
            window_name: format!("de-bg-{}-0-cmd", pane_id.trim_start_matches('%')),
            tmux_session: "test".to_string(),
            exit_code,
        }
    }

    fn alive(idle: bool, last_activity: u64) -> PaneGcInfo {
        PaneGcInfo {
            dead: false,
            idle_shell: idle,
            last_activity,
        }
    }

    fn dead(last_activity: u64) -> PaneGcInfo {
        PaneGcInfo {
            dead: true,
            idle_shell: true,
            last_activity,
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn gc_pane_gone_kills() {
        let wins = vec![make_win("%1", None)];
        let panes = std::collections::HashMap::new(); // pane not in map
        let result = plan_gc_actions(&wins, &panes, now());
        assert_eq!(result, vec!["%1".to_string()]);
    }

    #[test]
    fn gc_running_pane_no_kill() {
        let wins = vec![make_win("%2", None)];
        let mut panes = std::collections::HashMap::new();
        panes.insert("%2".to_string(), alive(false, now()));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_dead_pane_fresh_no_kill() {
        let wins = vec![make_win("%3", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // died just now — under threshold
        panes.insert("%3".to_string(), dead(now()));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_dead_pane_stale_kills() {
        let wins = vec![make_win("%4", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // died 60 seconds ago — over DEAD_THRESHOLD_SECS (30)
        panes.insert("%4".to_string(), dead(now().saturating_sub(60)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert_eq!(result, vec!["%4".to_string()]);
    }

    #[test]
    fn gc_completed_idle_fresh_no_kill() {
        let wins = vec![make_win("%5", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // completed but became idle just now — under IDLE_THRESHOLD_SECS (120)
        panes.insert("%5".to_string(), alive(true, now()));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_completed_idle_stale_kills() {
        let wins = vec![make_win("%6", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // idle for 180 seconds — over IDLE_THRESHOLD_SECS (120)
        panes.insert("%6".to_string(), alive(true, now().saturating_sub(180)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert_eq!(result, vec!["%6".to_string()]);
    }

    #[test]
    fn gc_completed_not_idle_no_kill() {
        // exit_code is set but pane is still running another command
        let wins = vec![make_win("%7", Some(0))];
        let mut panes = std::collections::HashMap::new();
        panes.insert("%7".to_string(), alive(false, now().saturating_sub(180)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_no_exit_code_idle_no_kill() {
        // Still "running" (no exit_code recorded yet) — must not GC even if idle
        let wins = vec![make_win("%8", None)];
        let mut panes = std::collections::HashMap::new();
        panes.insert("%8".to_string(), alive(true, now().saturating_sub(180)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }
}
