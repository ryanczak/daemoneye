use crate::daemon::session::{
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe, with_sessions,
};
use crate::daemon::utils::log_event;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Background window management
// ---------------------------------------------------------------------------

pub fn close_bg_window(pane_id: &str, session_id: Option<&str>, sessions: &SessionStore) -> String {
    let Some(sid) = session_id else {
        return "No active session — cannot close background window.".to_string();
    };
    let looked_up: Result<(String, String, bool), String> = with_sessions(sessions, |store| {
        let Some(entry) = store.get(sid) else {
            return Err(format!("Session '{}' not found.", sid));
        };
        let Some(win) = entry.bg_windows.iter().find(|w| w.pane_id == pane_id) else {
            return Err(format!(
                "No background window with pane ID {} found in this session.",
                pane_id
            ));
        };
        Ok((
            win.window_name.clone(),
            win.tmux_session.clone(),
            win.exit_code.is_none(),
        ))
    });
    let (win_name, tmux_session, still_running) = match looked_up {
        Ok(v) => v,
        Err(msg) => return msg,
    };

    if still_running {
        log::warn!(
            "Agent closing still-running bg window {} (pane {})",
            win_name,
            pane_id
        );
    }

    if let Err(e) = crate::tmux::kill_job_window(&tmux_session, &win_name) {
        log::warn!(
            "close_background_window: failed to kill {}: {}",
            win_name,
            e
        );
    }

    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(sid) {
            entry.bg_windows.retain(|w| w.pane_id != pane_id);
        }
    });

    log_event(
        "close_bg_window",
        serde_json::json!({
            "session": sid, "pane_id": pane_id,
            "win_name": win_name, "was_running": still_running,
        }),
    );

    format!("Background window {} (pane {}) closed.", win_name, pane_id)
}

/// True when `window_name` belongs to a daemon-managed window.
pub(crate) fn is_daemon_window(window_name: &str) -> bool {
    crate::daemon::is_daemon_window(window_name)
}

/// Why `tmux_control(action="kill_window")` must refuse this window, or `None`
/// when it may proceed (M12 D5).
///
/// Pure so the two refusals can be tested without tmux or a cache.
pub(crate) fn kill_window_refusal(window_name: &str, chat_window: Option<&str>) -> Option<String> {
    if is_daemon_window(window_name) {
        return Some(format!(
            "Refusing to kill window '{}': it is daemon-managed. Use close_background_window to \
             close a background job's window.",
            window_name
        )); // daemon windows belong to close_background_window
    }
    if chat_window == Some(window_name) {
        return Some(format!(
            "Refusing to kill window '{}': it contains the chat pane, which would end this \
             conversation.",
            window_name
        )); // never kill the window we are talking through
    }
    None
}

// ---------------------------------------------------------------------------
// Watch pane
// ---------------------------------------------------------------------------

/// Uninstalls a tmux hook on drop so an aborted or panicking `watch_pane` task
/// never leaves a stale `pane-title-changed` hook firing forever. Mirrors
/// `FgHookGuard` in `foreground.rs` (kept separate — see STANDARDS §2.2).
struct WatchHookGuard {
    pane_id: String,
    hook_name: String,
}

impl Drop for WatchHookGuard {
    fn drop(&mut self) {
        let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
            "set-hook",
            "-u",
            "-t",
            &self.pane_id,
            &self.hook_name,
        ]));
    }
}

pub fn watch_pane(
    pane_id: &str,
    timeout_secs: u64,
    pattern: Option<&str>,
    session_id: Option<&str>,
    session_name: &str,
    sessions: &SessionStore,
) -> String {
    let initial_cmd = crate::tmux::pane_current_command(pane_id).unwrap_or_default();

    let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hook_name = format!("pane-title-changed[@de_wp_{}]", hook_idx);
    let current_exe =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
    let notify_cmd = format!(
        "run-shell -b '{} notify activity {} 0 \"{}\"'",
        current_exe.display(),
        pane_id,
        crate::daemon::utils::shell_escape_arg(session_name)
    );
    let hook_name_clone = hook_name.clone();
    let _ = std::process::Command::new("tmux")
        .args(["set-hook", "-t", pane_id, &hook_name_clone, &notify_cmd])
        .output();

    let mut wp_rx = bg_done_subscribe();

    let pane_id_owned = pane_id.to_string();
    let session_id_owned = session_id.unwrap_or("-").to_string();
    let sessions_clone = sessions.clone();
    let timeout = Duration::from_secs(timeout_secs);
    let pattern_owned = pattern.map(|s| s.to_string());

    log::info!(
        "watch_pane: monitoring {} (initial_cmd={:?}) for session {}",
        pane_id,
        initial_cmd,
        session_id_owned
    );
    log_event(
        "watch_pane",
        serde_json::json!({
            "session": session_id_owned, "pane_id": pane_id,
            "pattern": pattern, "status": "active"
        }),
    );

    tokio::spawn(async move {
        let _guard = WatchHookGuard {
            pane_id: pane_id_owned.clone(),
            hook_name: hook_name.clone(),
        };

        let slow_poll = Duration::from_millis(500);
        let start_wait = Duration::from_secs(5);

        let pattern_re = pattern_owned
            .as_deref()
            .and_then(|p| regex::RegexBuilder::new(p).size_limit(1 << 20).build().ok());

        let completed = tokio::time::timeout(timeout, async {
            if let Some(ref re) = pattern_re {
                loop {
                    tokio::select! {
                        result = wp_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == pane_id_owned {
                                    let p = pane_id_owned.clone();
                                    let snap = crate::tmux::off_runtime("capture-pane", move || crate::tmux::capture_pane(&p, 200))
                                        .await
                                        .and_then(|r| r.ok())
                                        .unwrap_or_default();
                                    if re.is_match(&snap) { break; }
                                }
                        }
                        _ = tokio::time::sleep(slow_poll) => {
                            let p = pane_id_owned.clone();
                            let snap = crate::tmux::off_runtime("capture-pane", move || crate::tmux::capture_pane(&p, 200))
                                .await
                                .and_then(|r| r.ok())
                                .unwrap_or_default();
                            if re.is_match(&snap) { break; }
                        }
                    }
                }
            } else {
                if super::super::foreground::is_shell_prompt(&initial_cmd) {
                    let _ = tokio::time::timeout(start_wait, async {
                        loop {
                            tokio::time::sleep(slow_poll).await;
                            let p = pane_id_owned.clone();
                            let cur = crate::tmux::off_runtime("pane-current-command", move || crate::tmux::pane_current_command(&p))
                                .await
                                .and_then(|r| r.ok())
                                .unwrap_or_default();
                            if !super::super::foreground::is_shell_prompt(&cur) { break; }
                        }
                    }).await;
                }

                loop {
                    tokio::select! {
                        result = wp_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == pane_id_owned {
                                    let p = pane_id_owned.clone();
                                    let cur = crate::tmux::off_runtime("pane-current-command", move || crate::tmux::pane_current_command(&p))
                                        .await
                                        .and_then(|r| r.ok())
                                        .unwrap_or_default();
                                    if super::super::foreground::is_shell_prompt(&cur) { break; }
                                }
                        }
                        _ = tokio::time::sleep(slow_poll) => {
                            let p = pane_id_owned.clone();
                            let cur = crate::tmux::off_runtime("pane-current-command", move || crate::tmux::pane_current_command(&p))
                                .await
                                .and_then(|r| r.ok())
                                .unwrap_or_default();
                            if super::super::foreground::is_shell_prompt(&cur) { break; }
                        }
                    }
                }
            }
        }).await.is_ok();

        let p = pane_id_owned.clone();
        let raw =
            crate::tmux::off_runtime("capture-pane", move || crate::tmux::capture_pane(&p, 200))
                .await
                .and_then(|r| r.ok())
                .unwrap_or_default();
        let mut body =
            crate::ai::filter::mask_sensitive(&crate::daemon::utils::normalize_output(&raw));
        let hints = crate::manifest::related_knowledge_hints(&body);
        if !hints.is_empty() {
            body.push('\n');
            body.push_str(&hints);
        }

        let content = if completed {
            if let Some(ref pat) = pattern_owned {
                format!(
                    "[Watch Pane Match] Pattern `{}` matched in pane {}.\n<output>\n{}\n</output>",
                    pat, pane_id_owned, body
                )
            } else {
                format!(
                    "[Watch Pane Complete] Command finished in pane {}.\n<output>\n{}\n</output>",
                    pane_id_owned, body
                )
            }
        } else {
            format!(
                "[Watch Pane Timeout] Timed out waiting in pane {}.\n<output>\n{}\n</output>",
                pane_id_owned, body
            )
        };

        let watch_msg = crate::ai::Message {
            role: "user".to_string(),
            content,
            tool_calls: None,
            tool_results: None,
            turn: None,
        };

        // Phase 1 (locked): confirm the entry exists and take what the rest needs.
        let Some(chat_pane) = with_sessions(&sessions_clone, |store| {
            store
                .get_mut(&session_id_owned)
                .map(|entry| entry.chat_pane.clone())
        }) else {
            log::info!(
                "watch_pane {}: {}",
                pane_id_owned,
                if completed { "completed" } else { "timed out" }
            );
            return;
        };

        // Phase 2 (unlocked): the file write.
        append_session_message(&session_id_owned, &watch_msg);

        // Phase 3 (locked): push the message into the in-memory history.
        with_sessions(&sessions_clone, |store| {
            if let Some(entry) = store.get_mut(&session_id_owned) {
                entry.messages.push(watch_msg);
            }
        });

        // Phase 4 (unlocked): the tmux notification.
        let alert = if completed {
            format!("Watched pane {} command completed", pane_id_owned)
        } else {
            format!("Watched pane {} timed out", pane_id_owned)
        };
        if let Some(ref cp) = chat_pane {
            let cp = cp.clone();
            let _ = crate::tmux::off_runtime("display-message", move || {
                std::process::Command::new("tmux")
                    .args(["display-message", "-d", "5000", "-t", &cp, &alert])
                    .output()
            })
            .await;
        }
        log::info!(
            "watch_pane {}: {}",
            pane_id_owned,
            if completed { "completed" } else { "timed out" }
        );
    });

    if let Some(pat) = pattern {
        format!(
            "Now watching pane {} for pattern `{}`. \
             You will receive [Watch Pane Match] when the pattern appears, \
             or [Watch Pane Timeout] after {} seconds.",
            pane_id, pat, timeout_secs
        )
    } else {
        format!(
            "Now watching pane {} for command completion. \
             You will receive [Watch Pane Complete] when the command finishes, \
             or [Watch Pane Timeout] after {} seconds.",
            pane_id, timeout_secs
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_bg_window_no_session() {
        let store: SessionStore = SessionStore::new();
        assert_eq!(
            close_bg_window("%1", None, &store),
            "No active session — cannot close background window."
        );
    }

    #[test]
    fn close_bg_window_unknown_session() {
        let store: SessionStore = SessionStore::new();
        assert_eq!(
            close_bg_window("%1", Some("missing-sid"), &store),
            "Session 'missing-sid' not found."
        );
    }

    #[test]
    fn kill_window_refusal_refuses_daemon_windows() {
        // A background-window name returns Some mentioning close_background_window.
        let refusal = kill_window_refusal("de-bg-42-1712937600-cargo", None);
        assert!(refusal.is_some(), "daemon bg window should be refused");
        let msg = refusal.unwrap();
        assert!(
            msg.contains("close_background_window"),
            "message should mention close_background_window, got: {msg}"
        );

        // A ghost-sched prefix also returns Some.
        let refusal = kill_window_refusal("de-gs-ir-42-1712937600-ir", None);
        assert!(refusal.is_some(), "ghost-sched window should be refused");
    }

    #[test]
    fn kill_window_refusal_refuses_the_chat_window() {
        let refusal = kill_window_refusal("my-window", Some("my-window"));
        assert!(refusal.is_some(), "chat window should be refused");
        let msg = refusal.unwrap();
        assert!(
            msg.contains("chat pane"),
            "message should mention chat pane, got: {msg}"
        );
    }

    #[test]
    fn kill_window_refusal_allows_a_plain_user_window() {
        // Neither daemon-owned nor the chat window → None.
        assert_eq!(
            kill_window_refusal("my-editor", None),
            None,
            "plain window with no chat_window should be allowed"
        );

        // chat_window names a different window → allowed.
        assert_eq!(
            kill_window_refusal("my-editor", Some("my-terminal")),
            None,
            "window different from chat_window should be allowed"
        );
    }
}
