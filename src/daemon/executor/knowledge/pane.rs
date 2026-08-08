use crate::ai::filter::mask_sensitive;
use crate::daemon::session::{
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe, with_sessions,
};
use crate::daemon::utils::{log_event, normalize_output};
use crate::util::UnpoisonExt;
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

// ---------------------------------------------------------------------------
// List panes
// ---------------------------------------------------------------------------

pub fn list_panes(cache: &crate::tmux::cache::SessionCache, chat_pane: Option<&str>) -> String {
    let session = cache.session_name.read().unwrap_or_log().clone();
    let panes = cache.panes.read().unwrap_or_log();

    let mut rows: Vec<_> = panes
        .iter()
        .filter(|(_, state)| state.session_name == session)
        .filter(|(id, _)| chat_pane != Some(id.as_str()))
        .collect();
    rows.sort_by_key(|(id, _)| id.as_str());

    if rows.is_empty() {
        return format!("No targetable panes found in session '{}'.", session);
    }

    let mut out = format!(
        "{} pane{} in session '{}' (chat pane excluded):\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        session
    );
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for (id, state) in &rows {
        let title_part = if !state.pane_title.is_empty() && state.pane_title != state.current_cmd {
            format!("  title:{}", mask_sensitive(&state.pane_title))
        } else {
            String::new()
        };
        let start_part = if !state.start_cmd.is_empty() && state.start_cmd != state.current_cmd {
            format!("  started:{}", state.start_cmd)
        } else {
            String::new()
        };
        let ghost_part = if state
            .window_name
            .starts_with(crate::daemon::INCIDENT_WINDOW_PREFIX)
            || state
                .window_name
                .starts_with(crate::daemon::GS_BG_WINDOW_PREFIX)
            || state
                .window_name
                .starts_with(crate::daemon::GS_SCHED_WINDOW_PREFIX)
        {
            "  [ghost]"
        } else {
            ""
        };
        let sync_part = if state.synchronized {
            "  [synchronized]"
        } else {
            ""
        };
        let dead_part = if state.dead {
            format!("  [dead: {}]", state.dead_status.unwrap_or(0))
        } else {
            String::new()
        };
        let activity_part = if state.last_activity > 0 && now_secs >= state.last_activity {
            let age = now_secs - state.last_activity;
            if age < 30 {
                format!("  [active {}s ago]", age)
            } else if age < 3600 {
                format!("  [idle {}m]", age / 60)
            } else {
                format!("  [idle {}h{}m]", age / 3600, (age % 3600) / 60)
            }
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {}  idx:{:<3}  window:{:<12}  cmd:{:<8}  cwd:{}{}{}{}{}{}{}\n",
            id,
            state.pane_index,
            state.window_name,
            state.current_cmd,
            state.current_path,
            start_part,
            title_part,
            ghost_part,
            sync_part,
            dead_part,
            activity_part,
        ));
    }
    out.push_str(
        "\nUse the pane ID as target_pane in run_terminal_command to execute a command there.",
    );
    out
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
        let mut body = mask_sensitive(&normalize_output(&raw));
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
    use crate::tmux::cache::{PaneState, SessionCache};
    use crate::util::UnpoisonExt;

    fn pane(cmd: &str, window: &str, idx: usize) -> PaneState {
        PaneState {
            buffer: String::new(),
            summary: String::new(),
            current_cmd: cmd.to_string(),
            current_path: "/home/user".to_string(),
            pane_title: String::new(),
            last_updated: std::time::Instant::now(),
            scroll_position: 0,
            history_size: 0,
            in_copy_mode: false,
            synchronized: false,
            window_name: window.to_string(),
            session_name: "sess".to_string(),
            dead: false,
            dead_status: None,
            last_activity: 0,
            start_cmd: String::new(),
            pane_index: idx,
            shell_pid: 0,
        }
    }

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
    fn list_panes_excludes_chat_pane() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            p.insert("%1".to_string(), pane("bash", "main", 0));
            p.insert("%2".to_string(), pane("vim", "edit", 1));
        }
        let out = list_panes(&cache, Some("%1"));
        assert!(!out.contains("%1"), "chat pane must be excluded: {out}");
        assert!(out.contains("%2"), "non-chat pane must be listed: {out}");
        assert!(out.contains("idx:1"));
    }

    #[test]
    fn list_panes_empty_when_only_chat_pane() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            p.insert("%1".to_string(), pane("bash", "main", 0));
        }
        let out = list_panes(&cache, Some("%1"));
        assert!(
            out.contains("No targetable panes found in session 'sess'"),
            "got: {out}"
        );
    }

    #[test]
    fn list_panes_excludes_foreign_session_panes() {
        let c = SessionCache::new("home");
        {
            let mut panes = c.panes.write().unwrap_or_log();
            panes.insert("%1".to_string(), pane("bash", "main", 0));
            // "pane" fixture uses session_name "sess", so for a cache created with "home"
            // the fixture pane is foreign — but we need a home pane too. Override session.
            let p1 = panes.get_mut("%1").unwrap();
            p1.session_name = "home".to_string();
            // Foreign pane: session_name stays "sess" (from fixture), window is non-daemon
            let foreign = pane("nvim", "editor", 1);
            panes.insert("%9".to_string(), foreign);
        }
        let output = list_panes(&c, None);
        assert!(
            output.contains("%1"),
            "home pane should appear in list_panes output, got: {output}"
        );
        assert!(
            !output.contains("%9"),
            "foreign pane must not appear in list_panes output, got: {output}"
        );
    }
}
