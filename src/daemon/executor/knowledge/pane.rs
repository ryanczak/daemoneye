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
// Read pane (M12 D3)
// ---------------------------------------------------------------------------

/// Default scrollback depth when `lines` is omitted.
const READ_PANE_DEFAULT_LINES: usize = 200;
/// Hard ceiling on a single `read_pane` capture.
const READ_PANE_MAX_LINES: usize = 2000;

/// Pure helper: compute the actual capture depth from the user request and the
/// pane's known history size. Extracted so it can be tested without tmux.
fn read_pane_depth(requested: Option<u64>, history_size: usize) -> usize {
    let requested = match requested {
        Some(n) if n > 0 => (n as usize).min(READ_PANE_MAX_LINES),
        _ => READ_PANE_DEFAULT_LINES,
    };
    if history_size > 0 {
        requested.min(history_size)
    } else {
        requested
    }
}

pub async fn read_pane(
    cache: &crate::tmux::cache::SessionCache,
    chat_pane: Option<&str>,
    pane_id: &str,
    lines: Option<u64>,
    grep: Option<&str>,
) -> String {
    if chat_pane == Some(pane_id) {
        return format!(
            "Error: {} is the chat pane — its content is this conversation. \
             Use get_terminal_context for the user's active pane.",
            pane_id
        );
    }

    let (known, history_size, window_name, session_name, status) = {
        let panes = cache.panes.read().unwrap_or_log();
        match panes.get(pane_id) {
            Some(p) => (
                true,
                p.history_size,
                p.window_name.clone(),
                p.session_name.clone(),
                p.status,
            ),
            None => (
                false,
                0usize,
                String::new(),
                String::new(),
                crate::tmux::status::PaneStatus::Idle(0),
            ),
        }
    };
    if !known {
        return format!(
            "Error: pane {} not found. Call list_panes to see available panes.",
            pane_id
        );
    }

    // Validate grep regex before the capture call so the error is deterministic
    // and hermetic (no tmux call needed).
    let grep_re = if let Some(pat) = grep {
        match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
            Ok(re) => Some(re),
            Err(e) => return format!("Error: invalid grep regex: {}", e),
        }
    } else {
        None
    };

    let depth = read_pane_depth(lines, history_size);

    let pid = pane_id.to_string();
    let raw = match crate::tmux::off_runtime("capture-pane-annotated", move || {
        crate::tmux::capture_pane_annotated(&pid, depth)
    })
    .await
    {
        Some(Ok(s)) => s,
        Some(Err(e)) => return format!("Error capturing pane {}: {}", pane_id, e),
        None => return format!("Error: timed out capturing pane {}.", pane_id),
    };

    let all: Vec<&str> = raw.lines().collect();
    let filtered: Vec<&str> = if let Some(re) = &grep_re {
        all.iter().filter(|l| re.is_match(l)).copied().collect()
    } else {
        all
    };

    let home = cache.session_name.read().unwrap_or_log().clone();
    let sess_part = if session_name != home {
        format!(" session:{}", session_name)
    } else {
        String::new()
    };

    if filtered.is_empty() {
        return match grep {
            Some(p) => format!(
                "{} (window '{}'{} status:{}): no lines matched /{}/ in the last {} lines.",
                pane_id, window_name, sess_part, status, p, depth
            ),
            None => format!(
                "{} (window '{}'{} status:{}): pane is empty.",
                pane_id, window_name, sess_part, status
            ),
        };
    }

    let body = mask_sensitive(filtered.join("\n").trim_end());
    let head = match grep {
        Some(p) => format!(
            "{} (window '{}'{} status:{}) — {} lines matching /{}/ in the last {}:",
            pane_id,
            window_name,
            sess_part,
            status,
            filtered.len(),
            p,
            depth
        ),
        None => format!(
            "{} (window '{}'{} status:{}) — last {} lines:",
            pane_id,
            window_name,
            sess_part,
            status,
            filtered.len()
        ),
    };
    format!("{}\n{}", head, body)
}

// ---------------------------------------------------------------------------
// Find in panes (M12 D4)
// ---------------------------------------------------------------------------

/// Hard ceiling on matches returned by a single `find_in_panes` call.
const FIND_MAX_MATCHES: usize = 50;
/// Maximum foreign-session panes captured live in one `scope: "all"` pass.
const FIND_FOREIGN_MAX_PANES: usize = 20;
/// Scrollback depth of each live foreign-pane capture.
const FIND_FOREIGN_CAPTURE_LINES: usize = 200;

/// One matching line plus its ±1 line of context. 1-indexed `line_no`.
struct BufferMatch {
    line_no: usize,
    before: Option<String>,
    line: String,
    after: Option<String>,
}

/// Pure helper: find up to `limit` matches in `buffer`. Extracted so the match
/// and context arithmetic can be tested without tmux or a cache.
fn search_buffer(buffer: &str, re: &regex::Regex, limit: usize) -> Vec<BufferMatch> {
    let lines: Vec<&str> = buffer.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if out.len() >= limit {
            break;
        }
        if re.is_match(line) {
            out.push(BufferMatch {
                line_no: i + 1,
                before: i.checked_sub(1).map(|j| lines[j].to_string()),
                after: lines.get(i + 1).map(|s| s.to_string()),
                line: (*line).to_string(),
            });
        }
    }
    out
}

pub async fn find_in_panes(
    cache: &crate::tmux::cache::SessionCache,
    chat_pane: Option<&str>,
    pattern: &str,
    scope: Option<&str>,
) -> String {
    // 1. Build the regex first, before any lock or tmux call.
    let re = match regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
    {
        Ok(re) => re,
        Err(e) => return format!("Error: invalid search regex: {}", e),
    };

    // 2. Resolve the scope.
    let search_all = match scope {
        None | Some("session") => false,
        Some("all") => true,
        Some(s) => {
            return format!(
                "Error: invalid scope '{}' — expected \"session\" or \"all\".",
                s
            );
        }
    };

    // Read home session name before acquiring panes lock (M12 lock-ordering).
    let home = cache.session_name.read().unwrap_or_log().clone();

    // 3. Home pass — read the cache once, clone data, drop guard, then search.
    let mut home_rows: Vec<(
        String,
        String,
        String,
        crate::tmux::status::PaneStatus,
        String,
    )> = {
        let panes = cache.panes.read().unwrap_or_log();
        panes
            .iter()
            .filter(|(_, st)| st.session_name == home)
            .filter(|(id, _)| chat_pane != Some(id.as_str())) // never search the chat pane
            .map(|(id, st)| {
                (
                    id.clone(),
                    st.window_name.clone(),
                    st.session_name.clone(),
                    st.status,
                    st.buffer.clone(),
                )
            })
            .collect()
    };

    home_rows.sort_by(|a, b| a.0.cmp(&b.0));

    let mut results: Vec<(
        String,
        String,
        String,
        crate::tmux::status::PaneStatus,
        Vec<BufferMatch>,
    )> = Vec::new();
    let mut total_matches = 0usize;

    for (pane_id, window_name, session_name, status, buffer) in &home_rows {
        if total_matches >= FIND_MAX_MATCHES {
            break;
        }
        let limit = FIND_MAX_MATCHES - total_matches;
        let matches = search_buffer(buffer, &re, limit);
        if !matches.is_empty() {
            let n = matches.len();
            results.push((
                pane_id.clone(),
                window_name.clone(),
                session_name.clone(),
                *status,
                matches,
            ));
            total_matches += n;
        }
    }

    let home_count = home_rows.len();

    // 4. Foreign pass — only when scope is "all".
    let mut skipped = 0usize;
    let mut foreign_visited = 0usize;

    if search_all {
        let foreign_rows: Vec<(String, String, String, crate::tmux::status::PaneStatus)> = {
            let panes = cache.panes.read().unwrap_or_log();
            panes
                .iter()
                .filter(|(_, st)| st.session_name != home)
                .filter(|(id, _)| chat_pane != Some(id.as_str()))
                .map(|(id, st)| {
                    (
                        id.clone(),
                        st.window_name.clone(),
                        st.session_name.clone(),
                        st.status,
                    )
                })
                .collect()
        };

        let mut foreign_rows = foreign_rows;
        foreign_rows.sort_by(|a, b| a.0.cmp(&b.0));
        let foreign_rows: Vec<_> = foreign_rows
            .into_iter()
            .take(FIND_FOREIGN_MAX_PANES)
            .collect();

        for (pane_id, window_name, session_name, status) in foreign_rows {
            if total_matches >= FIND_MAX_MATCHES {
                break;
            }
            foreign_visited += 1;

            let pid = pane_id.clone();
            let raw = match crate::tmux::off_runtime("capture-pane-annotated", move || {
                crate::tmux::capture_pane_annotated(&pid, FIND_FOREIGN_CAPTURE_LINES)
            })
            .await
            {
                Some(Ok(s)) => s,
                Some(Err(_)) => {
                    skipped += 1;
                    continue;
                }
                None => {
                    skipped += 1;
                    continue;
                }
            };

            let limit = FIND_MAX_MATCHES - total_matches;
            let matches = search_buffer(&raw, &re, limit);
            if !matches.is_empty() {
                let n = matches.len();
                results.push((pane_id, window_name, session_name, status, matches));
                total_matches += n;
            }
        }
    }

    // 5. Render the result.
    if results.is_empty() {
        let foreign_part = if search_all && foreign_visited > 0 {
            format!(" plus {} foreign pane(s)", foreign_visited)
        } else {
            String::new()
        };
        return format!(
            "No pane matched /{}/ (searched {} pane(s) in session '{}'{}).",
            pattern, home_count, home, foreign_part
        );
    }

    let mut out = format!(
        "{} match(es) for /{}/ across {} pane(s):\n",
        total_matches,
        pattern,
        results.len()
    );

    for (pane_id, window_name, session_name, status, matches) in &results {
        let sess_part = if *session_name != home {
            format!(" session:{}", session_name)
        } else {
            String::new()
        };

        let mut body_parts = Vec::new();
        for m in matches {
            if let Some(ref before) = m.before {
                body_parts.push(format!("{:>5}- {}", m.line_no - 1, before));
            }
            body_parts.push(format!("{:>5}: {}", m.line_no, m.line));
            if let Some(ref after) = m.after {
                body_parts.push(format!("{:>5}- {}", m.line_no + 1, after));
            }
        }

        let body = mask_sensitive(&body_parts.join("\n"));
        out.push_str(&format!(
            "\n{} (window '{}'{} status:{}) — {} match(es):\n{}",
            pane_id,
            window_name,
            sess_part,
            status,
            matches.len(),
            body
        ));
    }

    if total_matches >= FIND_MAX_MATCHES {
        out.push_str(&format!(
            "\n[capped at {} matches — narrow the pattern]",
            FIND_MAX_MATCHES
        ));
    }
    if skipped > 0 {
        out.push_str(&format!(
            "\n[{} foreign pane(s) could not be captured]",
            skipped
        ));
    }

    out
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
            status: crate::tmux::status::PaneStatus::Idle(0),
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

    // ---------------------------------------------------------------------------
    // read_pane tests (M12 D3)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn read_pane_refuses_chat_pane() {
        let cache = SessionCache::new("sess");
        // Do NOT seed %999999 into the cache — the chat-pane guard runs before
        // the cache lookup, so the test is hermetic (no tmux call).
        let result = read_pane(&cache, Some("%999999"), "%999999", None, None).await;
        assert!(
            result.contains("chat pane"),
            "chat pane refusal message expected, got: {result}"
        );
    }

    #[tokio::test]
    async fn read_pane_unknown_pane_id_is_an_error() {
        let cache = SessionCache::new("sess");
        let result = read_pane(&cache, None, "%999999", None, None).await;
        assert!(
            result.contains("not found"),
            "unknown pane error expected, got: {result}"
        );
    }

    #[test]
    fn read_pane_caps_lines_at_history_size() {
        assert_eq!(read_pane_depth(Some(500), 50), 50);
    }

    #[test]
    fn read_pane_depth_defaults_and_ceiling() {
        // Default when None
        assert_eq!(read_pane_depth(None, 1000), 200);
        // Some(0) falls through to default
        assert_eq!(read_pane_depth(Some(0), 1000), 200);
        // Capped at READ_PANE_MAX_LINES
        assert_eq!(read_pane_depth(Some(5000), 10000), 2000);
        // Unknown history (0) must not clamp to zero
        assert_eq!(read_pane_depth(Some(10), 0), 10);
    }

    #[tokio::test]
    async fn read_pane_invalid_grep_regex_is_reported() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            p.insert("%999999".to_string(), pane("bash", "main", 0));
        }
        let result = read_pane(&cache, None, "%999999", None, Some("[")).await;
        assert!(
            result.contains("invalid grep regex"),
            "invalid regex error expected, got: {result}"
        );
    }

    // ---------------------------------------------------------------------------
    // find_in_panes tests (M12 D4)
    // ---------------------------------------------------------------------------

    #[test]
    fn search_buffer_includes_one_line_of_context() {
        let buffer = "line one\nline two MATCH\nline three";
        let re = regex::Regex::new("MATCH").unwrap();
        let matches = search_buffer(buffer, &re, 10);
        assert_eq!(matches.len(), 1);
        let m = &matches[0];
        assert_eq!(m.line_no, 2);
        assert_eq!(m.before.as_deref(), Some("line one"));
        assert_eq!(m.after.as_deref(), Some("line three"));

        // Match on first line — before is None
        let buffer2 = "MATCH here\nsecond line";
        let matches2 = search_buffer(buffer2, &re, 10);
        assert_eq!(matches2.len(), 1);
        assert_eq!(matches2[0].line_no, 1);
        assert!(matches2[0].before.is_none());
        assert_eq!(matches2[0].after.as_deref(), Some("second line"));
    }

    #[test]
    fn search_buffer_respects_limit() {
        let buffer: String = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let re = regex::Regex::new("line").unwrap();
        let matches = search_buffer(&buffer, &re, 3);
        assert_eq!(matches.len(), 3);
    }

    #[tokio::test]
    async fn find_in_panes_finds_match_in_cached_buffer() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            let mut p1 = pane("bash", "build", 0);
            p1.buffer = "compiling...\nerror[E0433]: failed to resolve\ndone".to_string();
            p1.session_name = "sess".to_string();
            p.insert("%1".to_string(), p1);
            let mut p2 = pane("vim", "edit", 1);
            p2.buffer = "all clear, nothing wrong".to_string();
            p2.session_name = "sess".to_string();
            p.insert("%2".to_string(), p2);
        }
        let result = find_in_panes(&cache, None, "error", None).await;
        assert!(result.contains("%1"), "matching pane id expected: {result}");
        assert!(result.contains("build"), "window name expected: {result}");
        assert!(
            result.contains("error[E0433]"),
            "matching line expected: {result}"
        );
        assert!(
            !result.contains("%2"),
            "non-matching pane id must not appear: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_excludes_chat_pane() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            let mut p1 = pane("bash", "main", 0);
            p1.buffer = "error found here".to_string();
            p1.session_name = "sess".to_string();
            p.insert("%1".to_string(), p1);
        }
        let result = find_in_panes(&cache, Some("%1"), "error", None).await;
        assert!(
            result.contains("No pane matched"),
            "chat pane must be excluded: {result}"
        );
        assert!(
            !result.contains("%1"),
            "chat pane id must not appear in output: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_no_match_is_not_an_error() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            let mut p1 = pane("bash", "main", 0);
            p1.buffer = "all is well".to_string();
            p1.session_name = "sess".to_string();
            p.insert("%1".to_string(), p1);
        }
        let result = find_in_panes(&cache, None, "error", None).await;
        assert!(
            result.contains("No pane matched"),
            "no-match message expected: {result}"
        );
        assert!(
            !result.starts_with("Error:"),
            "no-match must not be an error: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_invalid_regex_is_reported() {
        let cache = SessionCache::new("sess");
        let result = find_in_panes(&cache, None, "[", None).await;
        assert!(
            result.contains("invalid search regex"),
            "invalid regex error expected: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_invalid_scope_is_reported() {
        let cache = SessionCache::new("sess");
        let result = find_in_panes(&cache, None, "error", Some("everything")).await;
        assert!(
            result.contains("invalid scope"),
            "invalid scope error expected: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_caps_total_matches() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            let mut p1 = pane("bash", "build", 0);
            p1.buffer = (0..120)
                .map(|i| format!("error line {}", i))
                .collect::<Vec<_>>()
                .join("\n");
            p1.session_name = "sess".to_string();
            p.insert("%1".to_string(), p1);
        }
        let result = find_in_panes(&cache, None, "error", None).await;
        assert!(
            result.contains("capped at 50 matches"),
            "cap message expected: {result}"
        );
        assert!(
            result.contains("50 match(es)"),
            "head line must report 50 matches: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_default_scope_skips_foreign_panes() {
        let cache = SessionCache::new("home");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            // Home pane without the pattern
            let mut p1 = pane("bash", "main", 0);
            p1.buffer = "all clear".to_string();
            p1.session_name = "home".to_string();
            p.insert("%1".to_string(), p1);
            // Foreign pane WITH the pattern (but default scope won't capture it)
            let mut p2 = pane("vim", "editor", 1);
            p2.buffer = "error in foreign session".to_string();
            p2.session_name = "other".to_string();
            p.insert("%2".to_string(), p2);
        }
        let result = find_in_panes(&cache, None, "error", None).await;
        assert!(
            !result.contains("%2"),
            "foreign pane id must not appear with default scope: {result}"
        );
        assert!(
            result.contains("No pane matched"),
            "no match expected since home pane has no error: {result}"
        );
    }

    #[tokio::test]
    async fn find_in_panes_results_sorted_by_pane_id() {
        let cache = SessionCache::new("sess");
        {
            let mut p = cache.panes.write().unwrap_or_log();
            // Insert six panes in reverse id order so HashMap iteration is not sorted.
            for i in 1..=6 {
                let mut ps = pane("bash", "w", 0);
                ps.buffer = format!("found target {}", i);
                ps.session_name = "sess".to_string();
                p.insert(format!("%{}", i), ps);
            }
        }
        let result = find_in_panes(&cache, None, "target", None).await;
        // Every pane id must appear
        for i in 1..=6 {
            assert!(
                result.contains(&format!("%{}", i)),
                "pane id %{} must appear in output: {}",
                i,
                result
            );
        }
        // Collect byte offsets of each pane id — they must be strictly increasing
        // (i.e., %1 appears before %2 before %3 ... before %6).
        let offsets: Vec<_> = (1..=6)
            .map(|i| result.find(&format!("%{}", i)).unwrap())
            .collect();
        for w in offsets.windows(2) {
            assert!(
                w[0] < w[1],
                "pane ids must appear in ascending order; got offset {} before {} in: {}",
                w[0],
                w[1],
                result
            );
        }
    }
}
