use super::helpers::{
    BG_COMMAND_MAP, BgJobInfo, capture_and_archive, notify_session, shell_exit_var,
};
use crate::daemon::session::{
    BgWindowInfo, SessionStore, bg_done_subscribe, complete_subscribe, with_sessions,
};
use crate::daemon::utils::{
    command_has_sudo, is_fingerprint_prompt, log_event, shell_escape_arg, sudo_auth_failed,
    wait_for_sudo_prompt_and_inject,
};
use crate::tmux;
use std::sync::Mutex;
use std::time::Duration;

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
    _tool_id: &str,
    cmd_id: usize,
    cmd: &str,
    credential: Option<&str>,
    session_id: Option<String>,
    sessions: SessionStore,
) -> String {
    // Load the sandbox config and gate BEFORE any window is created: a
    // refused command must leave no `de-bg-*` window behind. The config is
    // reused below where `sandbox_window_command` builds its `job_id` from
    // the pane number.
    let config = crate::config::Config::load().unwrap_or_default();
    if config.sandbox.enabled
        && let Err(reason) = crate::daemon::executor::container::sandbox_preflight(&config.sandbox)
    {
        let message = crate::daemon::executor::container::describe_unavailable(&reason);
        log::warn!("refusing sandboxed background command: {message}");
        return message;
    }

    let prefix = if let Some(sid) = &session_id {
        if sid.starts_with("ghost-") {
            // Use the prefix registered on the session entry so webhook-triggered,
            // scheduler-triggered and interactive ghost shells get distinct prefixes.
            with_sessions(&sessions, |store| {
                store
                    .get(sid.as_str())
                    .map(|e| e.ghost_bg_prefix)
                    .unwrap_or(crate::daemon::GS_BG_WINDOW_PREFIX)
            })
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

    let (s, t) = (session.to_string(), temp_name.clone());
    let pane_id =
        match tmux::off_runtime("create-job-window", move || tmux::create_job_window(&s, &t)).await
        {
            Some(Ok(p)) => p,
            Some(Err(e)) => return format!("Failed to create background window: {}", e),
            None => {
                return "Failed to create background window: tmux timed out (server may be wedged)"
                    .to_string();
            }
        };

    // Build final name: prefix + pane-number + unix-ts + command-slug.
    let pane_num = pane_id.trim_start_matches('%');
    let cmd_slug = crate::daemon::utils::sanitize_cmd_for_window(cmd, 30);
    let final_name = format!("{}{}-{}-{}", prefix, pane_num, unix_ts, cmd_slug);
    let (s2, t2, f2) = (session.to_string(), temp_name.clone(), final_name.clone());
    let win_name = match tmux::off_runtime("rename-window", move || {
        tmux::rename_window(&s2, &t2, &f2)
    })
    .await
    {
        Some(Ok(())) => final_name,
        Some(Err(e)) => {
            log::warn!(
                "Failed to rename bg window {} -> {}: {}",
                temp_name,
                final_name,
                e
            );
            temp_name
        }
        None => {
            // already logged by off_runtime
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
    let p = pane_id.clone();
    match tmux::off_runtime("set-remain-on-exit", move || {
        tmux::set_remain_on_exit(&p, true)
    })
    .await
    {
        Some(Err(e)) => log::warn!("Failed to set remain-on-exit for {}: {}", win_name, e),
        None => {} // already logged by off_runtime
        Some(Ok(_)) => {}
    }

    // Detect the shell to select the right exit-code variable.
    let p2 = pane_id.clone();
    let shell_name = tmux::off_runtime("pane-current-command", move || {
        tmux::pane_current_command(&p2)
    })
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();
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

    let sandboxed_cmd;
    let cmd: &str = {
        if config.sandbox.enabled {
            let job_id = format!("{pane_num}-{unix_ts}");
            let spec = crate::daemon::executor::container::ExecSpec {
                job_id: &job_id,
                network: "none",
                is_ghost: false,
                command: cmd,
            };
            sandboxed_cmd = crate::daemon::executor::container::sandbox_window_command(
                &config.sandbox,
                &spec,
                cmd,
            );
            &sandboxed_cmd
        } else {
            cmd
        }
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
    let p3 = pane_id.clone();
    let pipe_log =
        match tmux::off_runtime("start-pipe-pane", move || tmux::start_pipe_pane(&p3)).await {
            Some(Ok(path)) => Some(path),
            Some(Err(e)) => {
                log::warn!("Failed to start pipe-pane for {}: {}", pane_id, e);
                None
            }
            None => None, // already logged by off_runtime
        };

    let p4 = pane_id.clone();
    let w4 = wrapped.clone();
    match tmux::off_runtime("send-keys", move || tmux::send_keys(&p4, &w4)).await {
        Some(Err(e)) => {
            if pipe_log.is_some() {
                let p5 = pane_id.clone();
                let _ =
                    tmux::off_runtime("stop-pipe-pane", move || tmux::stop_pipe_pane(&p5)).await;
            }
            let (s5, wn5) = (session.to_string(), win_name.clone());
            let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s5, &wn5))
                .await;
            return format!("Failed to send command to window: {}", e);
        }
        None => {
            if pipe_log.is_some() {
                let p5 = pane_id.clone();
                let _ =
                    tmux::off_runtime("stop-pipe-pane", move || tmux::stop_pipe_pane(&p5)).await;
            }
            let (s5, wn5) = (session.to_string(), win_name.clone());
            let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s5, &wn5))
                .await;
            return "Failed to send command to window: tmux timed out (server may be wedged)"
                .to_string();
        }
        Some(Ok(_)) => {}
    }

    // Inject sudo credential synchronously (≤10 s); must happen before we return.
    // P3: detect auth failure and log a warning — the failed exit code propagates
    // through the completion monitor and will be visible to the AI.
    if let Some(cred) = credential {
        if wait_for_sudo_prompt_and_inject(&pane_id, cred, "[de-sudo-prompt]").await {
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
            let p_snap = pane_id.clone();
            let snap = tmux::off_runtime("capture-pane", move || {
                crate::tmux::capture_pane(&p_snap, 10)
            })
            .await
            .and_then(|r| r.ok())
            .unwrap_or_default();
            if is_fingerprint_prompt(&snap) {
                log::warn!(
                    "sudo fingerprint auth not supported in background panes ({}): {}",
                    pane_id,
                    cmd
                );
                let (s_kill, wn_kill) = (session.to_string(), win_name.clone());
                let _ = tmux::off_runtime("kill-job-window", move || {
                    tmux::kill_job_window(&s_kill, &wn_kill)
                })
                .await;
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
    if let Some(ref sid) = session_id {
        with_sessions(&sessions, |store| {
            if let Some(entry) = store.get_mut(sid) {
                entry.bg_windows.push(BgWindowInfo {
                    pane_id: pane_id.clone(),
                    window_name: win_name.clone(),
                    tmux_session: session.to_string(),
                    exit_code: None,
                });
            }
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
                            let p_dead = pane_id.clone();
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
            // Fast path: command finished within 3 s — return output inline as the
            // tool result.  Do NOT call notify_session; the output is already here.
            if pipe_log.is_some() {
                let p_pipe = pane_id.clone();
                let _ = tmux::off_runtime("pipe-pane", move || {
                    std::process::Command::new("tmux")
                        .args(["pipe-pane", "-t", &p_pipe])
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
                }),
            );

            // Update exit_code in bg_windows.
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
                let (s_gc, wn_gc) = (session.to_string(), win_name.clone());
                match tmux::off_runtime("kill-job-window", move || {
                    tmux::kill_job_window(&s_gc, &wn_gc)
                })
                .await
                {
                    Some(Err(e)) => log::error!("Failed to GC dead bg window {}: {}", win_name, e),
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
                                            let p_dead_bg = pane_id_bg.clone();
                                            let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p_dead_bg))
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
                    let p_pipe_bg = pane_id_bg.clone();
                    let _ = tmux::off_runtime("pipe-pane", move || {
                        std::process::Command::new("tmux")
                            .args(["pipe-pane", "-t", &p_pipe_bg])
                            .output()
                    })
                    .await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }

                let duration_ms = started_at.elapsed().as_millis() as u64;
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
                        "duration_ms": duration_ms,
                        "pane_persists": pane_persists,
                    }),
                );

                if let Some(ref sid) = session_id_bg {
                    let s_ns = sessions_bg.clone();
                    let sid_ns = sid.clone();
                    let job = BgJobInfo {
                        pane_id: pane_id_bg.clone(),
                        cmd: cmd_bg.clone(),
                        win_name: win_name_bg.clone(),
                        exit_code,
                        body: body.clone(),
                        pane_persists,
                    };
                    let _ = tmux::off_runtime("notify-session", move || {
                        notify_session(&s_ns, &sid_ns, job)
                    })
                    .await;
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
                    let (s_kill_bg, wn_kill_bg) = (session_bg.to_string(), win_name_bg.clone());
                    match tmux::off_runtime("kill-job-window", move || {
                        tmux::kill_job_window(&s_kill_bg, &wn_kill_bg)
                    })
                    .await
                    {
                        Some(Err(e)) => {
                            log::error!("Failed to GC dead bg window {}: {}", win_name_bg, e)
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
                "Background command sent to pane {pane_id} (window {win_name}). \
                 You will receive a [Background Task Completed] context message when it finishes. \
                 Use target=\"{pane_id}\" to run follow-up commands in the same shell."
            )
        }
    }
}
