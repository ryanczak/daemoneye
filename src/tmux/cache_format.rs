use super::ansi::annotate_ansi;
use super::cache::{ContextScope, PaneState, SessionCache, window_in_scope};
use crate::ai::filter::mask_sensitive;
use crate::tmux;
use crate::util::UnpoisonExt;
use chrono::Local;

/// Fresh content for the active pane.
///
/// R2: use ANSI-escape-aware capture so colour codes are converted to
/// semantic markers ([ERROR:], [WARN:], [OK:]).  The `-e` variant asks tmux
/// to preserve escape sequences.
fn active_pane_content(pane_id: &str, scroll_pos: usize) -> String {
    // Unit tests must be hermetic: a live tmux server on the host must not
    // leak real pane content into fake-cache tests (the pane ids the tests
    // invent — `%1`, `%5` — can collide with real panes on the operator's
    // server). Behave exactly like a host with no tmux server running.
    // `cfg!` (not `#[cfg]`) so the live path below stays compiled in test
    // builds and its callees don't trip the dead_code lint.
    if cfg!(test) {
        return "(pane unavailable)".to_string();
    }
    if scroll_pos > 0 {
        crate::tmux::capture_pane_at_scroll_with_escapes(pane_id, scroll_pos, 200)
            .map(|s| annotate_ansi(&s))
            .or_else(|_| {
                crate::tmux::capture_pane_with_escapes(pane_id, 200).map(|s| annotate_ansi(&s))
            })
            .unwrap_or_else(|_| "(pane unavailable)".to_string())
    } else {
        crate::tmux::capture_pane_with_escapes(pane_id, 200)
            .map(|s| annotate_ansi(&s))
            .unwrap_or_else(|_| "(pane unavailable)".to_string())
    }
}

impl SessionCache {
    /// Returns a compact `[PANE MAP]` line mapping window-relative pane indices to
    /// absolute pane IDs.  Excludes daemon-owned background windows and the chat
    /// pane itself; returns an empty string when no targetable panes are known.
    pub fn pane_map_summary(&self, chat_pane: Option<&str>) -> String {
        // Read session_name before the panes guard (M12 lock ordering).
        let home = self.session_name.read().unwrap_or_log().clone();
        let panes = self.panes.read().unwrap_or_log();
        let mut entries: Vec<_> = panes
            .iter()
            .filter(|(_, state)| state.session_name == home)
            .filter(|(id, state)| {
                crate::daemon::is_targetable_pane(&state.window_name, id, chat_pane)
            })
            .collect();
        if entries.is_empty() {
            return String::new();
        }
        entries.sort_by_key(|(_, s)| (s.window_name.as_str(), s.pane_index));
        let active_id: Option<String> = self.active_pane.read().unwrap_or_log().clone();
        let parts: Vec<String> = entries
            .iter()
            .map(|(id, state)| {
                let active_mark = if active_id.as_deref() == Some(id.as_str()) {
                    "*"
                } else {
                    ""
                };
                format!(
                    "idx:{}={}{}  ({}, '{}')",
                    state.pane_index, id, active_mark, state.current_cmd, state.window_name
                )
            })
            .collect();
        format!("[PANE MAP] {}\n", parts.join(" | "))
    }

    pub fn get_labeled_context(&self, chat_pane: Option<&str>) -> String {
        self.get_labeled_context_scoped(chat_pane, ContextScope::Session)
    }

    /// Build a labeled terminal context block for the AI.
    ///
    /// The pane wearing `[ACTIVE PANE]` is the user's real tmux focus — the
    /// cache's `active_pane` — captured at full depth, whenever the cache has
    /// seen that focus, it is not the chat pane itself, and it is a cached
    /// pane of the home session. Otherwise no pane wears the label, so a
    /// snapshot never holds more than one `[ACTIVE PANE` block. Callers pass
    /// only the chat pane; whoever calls, the label follows the user's focus
    /// (M29 06a F1 — `get_shell_context` passes the chat pane, and the turn
    /// prompt passes the pane the chat chose at startup, so under the old
    /// argument-based rule a focus on any other pane was never labelled).
    ///
    /// All other cached panes — including a focused pane that did not get the
    /// active block — are included as brief summaries tagged `[VISIBLE PANE]`,
    /// `[BACKGROUND PANE]` or `[SESSION PANE]` with CWD and terminal title.
    /// A `[SESSION ENVIRONMENT]` block is prepended when any high-signal env vars are set.
    ///
    /// `chat_pane` is the pane running `daemoneye chat` — it is excluded from the
    /// pane listing so the AI cannot accidentally target it with foreground
    /// commands.
    pub fn get_labeled_context_scoped(
        &self,
        chat_pane: Option<&str>,
        scope: ContextScope,
    ) -> String {
        let mut out = String::new();
        self.append_topology_section(&mut out);
        self.append_environment_section(&mut out);
        self.append_viewport_section(&mut out);

        let active_pane = self.active_pane.read().unwrap_or_log().clone();
        // Read session_name before the panes guard (M12 lock ordering).
        let home = self.session_name.read().unwrap_or_log().clone();
        let active_label_pane = active_pane
            .as_deref()
            .filter(|focus| Some(*focus) != chat_pane)
            .filter(|focus| {
                self.panes
                    .read()
                    .unwrap_or_log()
                    .get(*focus)
                    .is_some_and(|p| p.session_name == home)
            });

        if let Some(pane_id) = active_label_pane {
            self.append_active_pane_section(&mut out, pane_id);
        }

        self.append_peer_panes_section(&mut out, chat_pane, active_label_pane, scope);
        self.append_foreign_panes_section(&mut out, chat_pane, active_label_pane, scope);
        self.append_footer_sections(&mut out);
        out
    }

    fn append_topology_section(&self, out: &mut String) {
        let wins = self.windows.read().unwrap_or_log();
        if wins.len() < 2 {
            return;
        }
        let count = wins.len();
        let parts: Vec<String> = wins
            .iter()
            .map(|w| {
                let mut desc = w.window_name.clone();
                desc.push_str(&format!(
                    " (ID: {}, {} pane{}",
                    w.window_id,
                    w.pane_count,
                    if w.pane_count == 1 { "" } else { "s" }
                ));
                if w.active {
                    desc.push_str(", active");
                }
                if w.zoomed {
                    desc.push_str(", zoomed");
                }
                if w.last_active {
                    desc.push_str(", last active");
                }
                if w.has_bell() {
                    desc.push_str(", bell!");
                }
                if w.has_activity() {
                    desc.push_str(", activity");
                }
                desc.push(')');
                desc
            })
            .collect();
        out.push_str(&format!(
            "[SESSION TOPOLOGY] {} windows — {}\n",
            count,
            parts.join(", ")
        ));
    }

    fn append_environment_section(&self, out: &mut String) {
        let env = self.environment.read().unwrap_or_log();
        if env.is_empty() {
            return;
        }
        let mut pairs: Vec<_> = env.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        let line = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, mask_sensitive(v)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("[SESSION ENVIRONMENT] {}\n", line));
    }

    fn append_viewport_section(&self, out: &mut String) {
        let (w, h) = *self.client_size.read().unwrap_or_log();
        if w > 0 && h > 0 {
            out.push_str(&format!("[CLIENT VIEWPORT] {}x{}\n", w, h));
        }
    }

    fn append_active_pane_section(&self, out: &mut String, pane_id: &str) {
        let (cwd, cmd, title, scroll_pos, in_copy_mode) = {
            let panes = self.panes.read().unwrap_or_log();
            if let Some(p) = panes.get(pane_id) {
                (
                    p.current_path.clone(),
                    p.current_cmd.clone(),
                    p.pane_title.clone(),
                    p.scroll_position,
                    p.in_copy_mode,
                )
            } else {
                (String::new(), String::new(), String::new(), 0usize, false)
            }
        };

        let content = active_pane_content(pane_id, scroll_pos);
        let cwd_label = if cwd.is_empty() {
            String::new()
        } else {
            format!(" | cwd: {}", cwd)
        };
        let title_label = if title.is_empty() || title == cmd {
            String::new()
        } else {
            format!(" | title: {}", mask_sensitive(&title))
        };
        let scroll_note = if scroll_pos > 0 {
            format!(" | scrolled {} lines up", scroll_pos)
        } else {
            String::new()
        };
        let copy_note = if in_copy_mode {
            " | copy mode".to_string()
        } else {
            String::new()
        };

        out.push_str(&format!(
            "[ACTIVE PANE {}{}{}{}{}]\n{}\n",
            pane_id,
            cwd_label,
            title_label,
            scroll_note,
            copy_note,
            mask_sensitive(&content),
        ));
    }

    fn append_peer_panes_section(
        &self,
        out: &mut String,
        chat_pane: Option<&str>,
        active_label_pane: Option<&str>,
        scope: ContextScope,
    ) {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Read session_name before the panes guard (M12 lock ordering).
        let home = self.session_name.read().unwrap_or_log().clone();
        let panes = self.panes.read().unwrap_or_log();

        let chat_window: Option<&str> = chat_pane
            .and_then(|cp| panes.get(cp))
            .map(|p| p.window_name.as_str())
            .filter(|w| !w.is_empty());

        let mut others: Vec<_> = panes
            .iter()
            .filter(|(_, state)| state.session_name == home)
            .filter(|(id, _)| active_label_pane != Some(id.as_str()))
            .filter(|(id, _)| chat_pane != Some(id.as_str()))
            .filter(|(_, state)| window_in_scope(scope, &state.window_name, chat_window))
            .collect();
        others.sort_by_key(|(id, _)| id.as_str());
        for (id, state) in others {
            out.push_str(&format_peer_pane_line(id, state, chat_window, now_secs));
        }
    }

    fn append_foreign_panes_section(
        &self,
        out: &mut String,
        chat_pane: Option<&str>,
        active_label_pane: Option<&str>,
        scope: ContextScope,
    ) {
        if scope != ContextScope::All {
            return;
        }
        // Read session_name before the panes guard (M12 lock ordering).
        let home = self.session_name.read().unwrap_or_log().clone();
        let panes = self.panes.read().unwrap_or_log();
        let mut foreign: Vec<_> = panes
            .iter()
            .filter(|(_, state)| state.session_name != home)
            .filter(|(id, _)| active_label_pane != Some(id.as_str()))
            .filter(|(id, _)| chat_pane != Some(id.as_str()))
            .collect();
        foreign.sort_by_key(|(id, _)| id.as_str());
        for (id, state) in foreign {
            let cwd_part = if !state.current_path.is_empty() {
                format!(" — {}", mask_sensitive(&state.current_path))
            } else {
                String::new()
            };
            out.push_str(&format!(
                "[FOREIGN SESSION PANE {} (idx:{} in '{}' | session:{}) — {}{} status:{}]\n",
                id,
                state.pane_index,
                state.window_name,
                state.session_name,
                state.current_cmd,
                cwd_part,
                state.status,
            ));
        }
    }

    fn append_footer_sections(&self, out: &mut String) {
        if out.is_empty() {
            out.push_str("(no terminal context available)");
            return;
        }
        let now_local = Local::now();
        let now_utc = now_local.to_utc();
        let tz_name = now_local.format("%Z").to_string();
        out.push_str(&format!(
            "[CURRENT TIME] {} UTC ({}: {})\n",
            now_utc.format("%Y-%m-%d %H:%M:%S"),
            tz_name,
            now_local.format("%Y-%m-%d %H:%M:%S"),
        ));

        let session_name = self.session_name.read().unwrap_or_log().clone();
        let other_ctx = tmux::other_sessions_context(&session_name);
        if !other_ctx.is_empty() {
            out.push_str(&other_ctx);
        }
    }
}

fn format_peer_pane_line(
    id: &str,
    state: &PaneState,
    chat_window: Option<&str>,
    now_secs: u64,
) -> String {
    let pane_label = if chat_window.is_some_and(|cw| cw == state.window_name) {
        "VISIBLE PANE"
    } else if crate::daemon::is_daemon_window(&state.window_name) {
        "BACKGROUND PANE"
    } else {
        "SESSION PANE"
    };
    let cwd_part = if state.current_path.is_empty() {
        String::new()
    } else {
        format!(" — {}", state.current_path)
    };
    let title_part = if state.pane_title.is_empty() || state.pane_title == state.current_cmd {
        String::new()
    } else {
        format!(" ({})", mask_sensitive(&state.pane_title))
    };
    let ghost_part = if crate::daemon::is_ghost_window(&state.window_name) {
        " [ghost]".to_string()
    } else {
        String::new()
    };
    let sync_part = if state.synchronized {
        " [synchronized]".to_string()
    } else {
        String::new()
    };
    let dead_part = if state.dead {
        let idle_sfx = if state.last_activity > 0 && now_secs > state.last_activity {
            let age = now_secs - state.last_activity;
            if age < 60 {
                format!(", idle {}s", age)
            } else if age < 3600 {
                format!(", idle {}m", age / 60)
            } else {
                format!(", idle {}h{}m", age / 3600, (age % 3600) / 60)
            }
        } else {
            String::new()
        };
        format!(" [dead: {}{}]", state.dead_status.unwrap_or(0), idle_sfx)
    } else {
        String::new()
    };
    let activity_part = if !state.dead && state.last_activity > 0 && now_secs >= state.last_activity
    {
        let age = now_secs - state.last_activity;
        if age < 30 {
            format!(" [active {}s ago]", age)
        } else if age < 3600 {
            format!(" [idle {}m]", age / 60)
        } else {
            format!(" [idle {}h{}m]", age / 3600, (age % 3600) / 60)
        }
    } else {
        String::new()
    };
    let start_part = if !state.start_cmd.is_empty() && state.start_cmd != state.current_cmd {
        format!(" (started: {})", state.start_cmd)
    } else {
        String::new()
    };
    format!(
        "[{} {} — {}{}{}{}{}{}{}{}]: {}\n",
        pane_label,
        id,
        state.current_cmd,
        start_part,
        cwd_part,
        title_part,
        ghost_part,
        sync_part,
        dead_part,
        activity_part,
        mask_sensitive(&state.summary),
    )
}
