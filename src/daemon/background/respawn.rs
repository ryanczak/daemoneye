use super::helpers::{
    BG_COMMAND_MAP, BgJobInfo, capture_and_archive, notify_session, shell_exit_var,
};
use crate::daemon::session::{SessionStore, bg_done_subscribe, complete_subscribe, with_sessions};
use crate::daemon::utils::{log_event, shell_escape_arg};
use crate::tmux;
use std::sync::Mutex;
use std::time::Duration;

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
    let p = pane_id.to_string();
    let respawn_ok = tmux::off_runtime("respawn-pane", move || {
        std::process::Command::new("tmux")
            .args(["respawn-pane", "-k", "-t", &p])
            .status()
    })
    .await
    .and_then(|r| r.ok())
    .map(|s| s.success())
    .unwrap_or(false);
    if !respawn_ok {
        return format!(
            "Error: failed to respawn pane {} (pane may no longer exist)",
            pane_id
        );
    }

    // Brief yield so tmux can start the shell before we query it.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started_at = tokio::time::Instant::now();

    // Detect shell for exit-code variable selection.
    let p2 = pane_id.to_string();
    let shell_name = tmux::off_runtime("pane-current-command", move || {
        tmux::pane_current_command(&p2)
    })
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();
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
    let p3 = pane_id.to_string();
    let pipe_log =
        match tmux::off_runtime("start-pipe-pane", move || tmux::start_pipe_pane(&p3)).await {
            Some(Ok(path)) => Some(path),
            Some(Err(e)) => {
                log::warn!("Failed to start pipe-pane for retry on {}: {}", pane_id, e);
                None
            }
            None => None, // already logged by off_runtime
        };

    let p4 = pane_id.to_string();
    let w = wrapped.clone();
    match tmux::off_runtime("send-keys", move || tmux::send_keys(&p4, &w)).await {
        Some(Err(e)) => {
            if pipe_log.is_some() {
                let p5 = pane_id.to_string();
                let _ =
                    tmux::off_runtime("stop-pipe-pane", move || tmux::stop_pipe_pane(&p5)).await;
            }
            return format!(
                "Error: failed to send retry command to pane {}: {}",
                pane_id, e
            );
        }
        None => {
            if pipe_log.is_some() {
                let p5 = pane_id.to_string();
                let _ =
                    tmux::off_runtime("stop-pipe-pane", move || tmux::stop_pipe_pane(&p5)).await;
            }
            return format!(
                "Error: failed to send retry command to pane {}: tmux timed out \
                 (server may be wedged)",
                pane_id
            );
        }
        Some(Ok(_)) => {}
    }

    // Reset exit_code in bg_windows so the session knows it's running again.
    if let Some(ref sid) = session_id {
        with_sessions(&sessions, |store| {
            if let Some(entry) = store.get_mut(sid)
                && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
            {
                w.exit_code = None;
            }
        });
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
                            let p_dead = pane_id_str.clone();
                            let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p_dead))
                                .await
                                .flatten()
                                .unwrap_or(-1);
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
                let p6 = pane_id.to_string();
                let _ = tmux::off_runtime("pipe-pane-stop", move || {
                    std::process::Command::new("tmux")
                        .args(["pipe-pane", "-t", &p6])
                        .output()
                })
                .await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let p = pane_id.to_string();
            let w = win_name.to_string();
            let body = tmux::off_runtime("capture-and-archive", move || {
                capture_and_archive(&p, &w, pipe_log)
            })
            .await
            .unwrap_or_default();

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

            if let Some(ref sid) = session_id {
                with_sessions(&sessions, |store| {
                    if let Some(entry) = store.get_mut(sid)
                        && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
                    {
                        w.exit_code = Some(exit_code);
                    }
                });
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
                let (s_gc, wn_gc) = (session.to_string(), win_name.to_string());
                match tmux::off_runtime("kill-job-window", move || {
                    tmux::kill_job_window(&s_gc, &wn_gc)
                })
                .await
                {
                    Some(Err(e)) => {
                        log::error!("Failed to GC retried bg window {}: {}", win_name, e);
                    }
                    None => {} // already logged by off_runtime
                    Some(Ok(_)) => {}
                }
                if let Some(ref sid) = session_id {
                    with_sessions(&sessions, |store| {
                        if let Some(entry) = store.get_mut(sid) {
                            entry.bg_windows.retain(|w| w.pane_id != pane_id);
                        }
                    });
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
                                            let p_dead = pane_id_bg.clone();
                                            let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p_dead))
                                                .await
                                                .flatten()
                                                .unwrap_or(-1);
                                            return (code, false);
                                        }
                                }
                            }
                        }
                    }
                ).await.unwrap_or((124, false));

                if pipe_log.is_some() {
                    let p7 = pane_id_bg.clone();
                    let _ = tmux::off_runtime("pipe-pane-stop", move || {
                        std::process::Command::new("tmux")
                            .args(["pipe-pane", "-t", &p7])
                            .output()
                    })
                    .await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }

                let p = pane_id_bg.to_string();
                let w = win_name_bg.to_string();
                let body = tmux::off_runtime("capture-and-archive", move || {
                    capture_and_archive(&p, &w, pipe_log)
                })
                .await
                .unwrap_or_default();

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
                    let (s_gc, wn_gc) = (session_bg.clone(), win_name_bg.clone());
                    match tmux::off_runtime("kill-job-window", move || {
                        tmux::kill_job_window(&s_gc, &wn_gc)
                    })
                    .await
                    {
                        Some(Err(e)) => {
                            log::error!("Failed to GC retried bg window {}: {}", win_name_bg, e);
                        }
                        None => {} // already logged by off_runtime
                        Some(Ok(_)) => {}
                    }
                    if let Some(ref sid) = session_id_bg {
                        with_sessions(&sessions_bg, |store| {
                            if let Some(entry) = store.get_mut(sid) {
                                entry.bg_windows.retain(|w| w.pane_id != pane_id_bg);
                            }
                        });
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
