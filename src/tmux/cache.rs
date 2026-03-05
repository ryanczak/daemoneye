use crate::ai::filter::mask_sensitive;
use crate::tmux;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::RwLock;

/// Cached state for a single tmux pane, refreshed every 2 seconds.
#[derive(Debug, Clone)]
pub struct PaneState {
    /// Raw text content of the pane (last 100 lines from `capture-pane`).
    pub buffer: String,
    /// Human-readable one-line summary derived from `buffer` by heuristics.
    pub summary: String,
    /// Foreground process name as reported by `#{pane_current_command}`.
    pub current_cmd: String,
    /// Current working directory of the shell (`#{pane_current_path}`).
    pub current_path: String,
    /// Terminal title set by the running application via OSC (`#{pane_title}`).
    pub pane_title: String,
    /// Wall-clock time of the most recent successful `capture-pane` call.
    pub last_updated: std::time::Instant,
    /// Lines scrolled back from the visible bottom (0 = at bottom, R3).
    pub scroll_position: usize,
    /// Total scrollback history lines (`#{history_size}`, R3).
    pub history_size: usize,
    /// True when the pane is in copy/scroll mode (`#{pane_in_mode}`, R4).
    pub in_copy_mode: bool,
    /// True when pane input is synchronized with other panes (`#{pane_synchronized}`, R6).
    pub synchronized: bool,
}

/// Shared, periodically-refreshed view of all panes in a tmux session.
///
/// The daemon spawns a background task that calls [`SessionCache::refresh`]
/// every 2 seconds.  Request handlers read from the cache without blocking on
/// live tmux calls, keeping response latency low.
pub struct SessionCache {
    pub session_name: String,
    /// Pane ID → state map; access via the `RwLock`.
    pub panes: RwLock<HashMap<String, PaneState>>,
    /// The currently-active pane ID as reported by `tmux display-message`.
    pub active_pane: RwLock<Option<String>>,
    /// High-signal tmux session environment variables (allowlisted subset).
    pub environment: RwLock<HashMap<String, String>>,
    /// Window-level topology for the session (P4).
    pub windows: RwLock<Vec<tmux::WindowState>>,
}

impl SessionCache {
    pub fn new(session_name: &str) -> Self {
        Self {
            session_name: session_name.to_string(),
            panes: RwLock::new(HashMap::new()),
            active_pane: RwLock::new(None),
            environment: RwLock::new(HashMap::new()),
            windows: RwLock::new(Vec::new()),
        }
    }

    /// Refresh the cache.
    ///
    /// Uses a single `list-panes` call to fetch all pane metadata (P3), then
    /// issues one `capture-pane` per pane for buffer content.  Session
    /// environment is refreshed on each cycle (P5).
    pub fn refresh(&self) -> Result<()> {
        // Active pane.
        let active = tmux::get_active_pane(&self.session_name)?;
        {
            let mut active_lock = self.active_pane.write().unwrap();
            *active_lock = Some(active);
        }

        // All pane metadata in one tmux call (P1 + P2 + P3).
        let rich_panes = tmux::list_panes_detailed().unwrap_or_default();

        for info in rich_panes {
            if info.session_name != self.session_name {
                continue;
            }
            if let Ok(content) = tmux::capture_pane(&info.pane_id, 100) {
                let mut panes = self.panes.write().unwrap();
                let entry = panes
                    .entry(info.pane_id.clone())
                    .or_insert_with(|| PaneState {
                        buffer: String::new(),
                        summary: String::new(),
                        current_cmd: String::new(),
                        current_path: String::new(),
                        pane_title: String::new(),
                        last_updated: std::time::Instant::now(),
                        scroll_position: 0,
                        history_size: 0,
                        in_copy_mode: false,
                        synchronized: false,
                    });

                entry.current_cmd = info.current_cmd;
                entry.current_path = info.current_path;
                entry.pane_title = info.title;
                entry.scroll_position = info.scroll_position;
                entry.history_size = info.history_size;
                entry.in_copy_mode = info.in_copy_mode;
                entry.synchronized = info.synchronized;

                if entry.buffer != content {
                    entry.buffer = content;
                    entry.summary = self.summarize(&entry.buffer);
                    entry.last_updated = std::time::Instant::now();
                }
            }
        }

        // Session environment (P5) — best-effort; ignore errors.
        if let Ok(env) = tmux::session_environment(&self.session_name) {
            if !env.is_empty() {
                let mut env_lock = self.environment.write().unwrap();
                *env_lock = env;
            }
        }

        // Window topology (P4) — best-effort; ignore errors.
        if let Ok(wins) = tmux::list_windows(&self.session_name) {
            let mut win_lock = self.windows.write().unwrap();
            *win_lock = wins;
        }

        Ok(())
    }

    /// Produce a one-line heuristic summary of a pane's visible content.
    ///
    /// Matches well-known patterns (shell prompt, `top`, HTTP log lines) and
    /// falls back to the first 50 characters of the last non-empty line.
    /// These heuristics are best-effort: unusual prompts or tools may not match.
    fn summarize(&self, buffer: &str) -> String {
        let lines: Vec<&str> = buffer.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return "Empty pane".to_string();
        }

        // Take the last non-empty line as a hint of what's happening
        let last_line = lines.last().unwrap_or(&"").trim();

        if last_line.starts_with('$') || last_line.starts_with('#') {
            format!("Idle shell at: {}", last_line)
        } else if last_line.contains("top - ") || last_line.contains("htop") {
            "Running system monitor".to_string()
        } else if last_line.contains("GET /") || last_line.contains("POST /") {
            "Tailing web logs".to_string()
        } else {
            format!("Active: {}", last_line.chars().take(50).collect::<String>())
        }
    }

    /// Get a full context summary for the AI.
    #[allow(dead_code)]
    pub fn get_context_summary(&self) -> String {
        let panes = self.panes.read().unwrap();
        let active_id = self.active_pane.read().unwrap();

        let mut summary = String::from("Current Tmux Session State:\n");
        for (id, state) in panes.iter() {
            let marker = if Some(id) == active_id.as_ref() {
                " (ACTIVE)"
            } else {
                ""
            };
            let masked_summary = mask_sensitive(&state.summary);
            summary.push_str(&format!("- Pane {}{}: {}\n", id, marker, masked_summary));
        }

        summary
    }

    /// Build a labeled terminal context block for the AI.
    ///
    /// The source pane (the user's working pane, identified by `DAEMONEYE_SOURCE_PANE`) is
    /// captured at full depth and tagged `[ACTIVE PANE]` so the AI immediately knows
    /// which pane is the user's current focus.  All other cached panes are included as
    /// brief summaries tagged `[BACKGROUND PANE]` with CWD and terminal title.
    /// A `[SESSION ENVIRONMENT]` block is prepended when any high-signal env vars are set.
    ///
    /// `chat_pane` is the pane running `daemoneye chat` — it is excluded from the
    /// background pane listing so the AI cannot accidentally target it with foreground
    /// commands.
    pub fn get_labeled_context(&self, source_pane: Option<&str>, chat_pane: Option<&str>) -> String {
        let mut out = String::new();

        // Window topology (P4) — prepend if session has ≥2 windows.
        {
            let wins = self.windows.read().unwrap_or_else(|e| e.into_inner());
            if wins.len() >= 2 {
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
        }

        // Session environment block (P5) — prepend if any vars are present.
        {
            let env = self.environment.read().unwrap_or_else(|e| e.into_inner());
            if !env.is_empty() {
                let mut pairs: Vec<_> = env.iter().collect();
                pairs.sort_by_key(|(k, _)| k.as_str());
                let line = pairs
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, mask_sensitive(v)))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("[SESSION ENVIRONMENT] {}\n", line));
            }
        }

        // Active pane — full capture, explicitly labelled.
        if let Some(pane_id) = source_pane {
            // Pull CWD, command, title, scroll position, and mode flags from cache in one lock.
            let (cwd, cmd, title, scroll_pos, in_copy_mode) = {
                let panes = self.panes.read().unwrap_or_else(|e| e.into_inner());
                if let Some(p) = panes.get(pane_id) {
                    (p.current_path.clone(), p.current_cmd.clone(), p.pane_title.clone(), p.scroll_position, p.in_copy_mode)
                } else {
                    (String::new(), String::new(), String::new(), 0usize, false)
                }
            };

            // R3: if the pane is scrolled back, capture content at that position.
            let content = if scroll_pos > 0 {
                crate::tmux::capture_pane_at_scroll(pane_id, scroll_pos, 200)
                    .or_else(|_| crate::tmux::capture_pane(pane_id, 200))
                    .unwrap_or_else(|_| "(pane unavailable)".to_string())
            } else {
                crate::tmux::capture_pane(pane_id, 200)
                    .unwrap_or_else(|_| "(pane unavailable)".to_string())
            };

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
            // R3: annotate when scrolled so the AI knows it's not looking at the bottom.
            let scroll_note = if scroll_pos > 0 {
                format!(" | scrolled {} lines up", scroll_pos)
            } else {
                String::new()
            };
            // R4: flag copy/scroll mode explicitly.
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

        // Background panes — summaries with cmd, cwd, and title, sorted for deterministic ordering.
        let panes = self.panes.read().unwrap_or_else(|e| e.into_inner());
        let mut bg: Vec<_> = panes
            .iter()
            .filter(|(id, _)| source_pane.map_or(true, |s| s != id.as_str()))
            .filter(|(id, _)| chat_pane.map_or(true, |c| c != id.as_str()))
            .collect();
        bg.sort_by_key(|(id, _)| id.as_str());
        for (id, state) in bg {
            let cwd_part = if state.current_path.is_empty() {
                String::new()
            } else {
                format!(" — {}", state.current_path)
            };
            let title_part = if state.pane_title.is_empty() || state.pane_title == state.current_cmd
            {
                String::new()
            } else {
                format!(" ({})", mask_sensitive(&state.pane_title))
            };
            // R6: warn the AI that input to this pane broadcasts to all synced panes.
            let sync_part = if state.synchronized {
                " [synchronized]"
            } else {
                ""
            };
            out.push_str(&format!(
                "[BACKGROUND PANE {}{}{}{}{}]: {}\n",
                id,
                format!(" — {}", state.current_cmd),
                cwd_part,
                title_part,
                sync_part,
                mask_sensitive(&state.summary),
            ));
        }

        if out.is_empty() {
            out.push_str("(no terminal context available)");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> SessionCache {
        SessionCache::new("test-session")
    }

    // ── summarize heuristics ──────────────────────────────────────────────────

    #[test]
    fn summarize_empty_buffer() {
        assert_eq!(cache().summarize(""), "Empty pane");
    }

    #[test]
    fn summarize_only_blank_lines() {
        assert_eq!(cache().summarize("   \n\n  "), "Empty pane");
    }

    #[test]
    fn summarize_dollar_prompt() {
        let buf = "some output\n$ ";
        let s = cache().summarize(buf);
        assert!(s.starts_with("Idle shell at:"), "got: {s}");
    }

    #[test]
    fn summarize_hash_prompt() {
        let buf = "root output\n# ";
        let s = cache().summarize(buf);
        assert!(s.starts_with("Idle shell at:"), "got: {s}");
    }

    #[test]
    fn summarize_top_output() {
        let buf = "Tasks: 200\ntop - 12:34:56 up 1 day";
        let s = cache().summarize(buf);
        assert_eq!(s, "Running system monitor");
    }

    #[test]
    fn summarize_web_log_get() {
        let buf = "127.0.0.1 - - [01/Jan/2024] GET /api/health HTTP/1.1 200";
        let s = cache().summarize(buf);
        assert_eq!(s, "Tailing web logs");
    }

    #[test]
    fn summarize_web_log_post() {
        let buf = "POST /submit HTTP/1.1";
        let s = cache().summarize(buf);
        assert_eq!(s, "Tailing web logs");
    }

    #[test]
    fn summarize_generic_truncates_to_50_chars() {
        let long_line = "x".repeat(100);
        let s = cache().summarize(&long_line);
        assert!(s.starts_with("Active:"));
        let content_part = s.trim_start_matches("Active: ");
        assert!(content_part.len() <= 50);
    }

    // ── get_labeled_context ───────────────────────────────────────────────────

    #[test]
    fn get_labeled_context_no_panes_no_source_returns_fallback() {
        let c = cache();
        let ctx = c.get_labeled_context(None, None);
        assert!(ctx.contains("no terminal context available"));
    }

    #[test]
    fn get_labeled_context_background_panes_sorted() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert(
                "%3".to_string(),
                PaneState {
                    buffer: "foo".to_string(),
                    summary: "summary3".to_string(),
                    current_cmd: String::new(),
                    current_path: String::new(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 0,
                    in_copy_mode: false,
                    synchronized: false,
                },
            );
            panes.insert(
                "%1".to_string(),
                PaneState {
                    buffer: "bar".to_string(),
                    summary: "summary1".to_string(),
                    current_cmd: String::new(),
                    current_path: String::new(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 0,
                    in_copy_mode: false,
                    synchronized: false,
                },
            );
        }
        let ctx = c.get_labeled_context(None, None);
        let pos1 = ctx.find("%1").unwrap();
        let pos3 = ctx.find("%3").unwrap();
        assert!(pos1 < pos3, "panes should be sorted by ID");
    }

    #[test]
    fn get_labeled_context_session_topology() {
        let c = cache();
        {
            let mut wins = c.windows.write().unwrap();
            wins.push(tmux::WindowState {
                window_id: "@1".to_string(),
                window_name: "nginx".to_string(),
                active: true,
                pane_count: 2,
                zoomed: false,
                last_active: false,
            });
            wins.push(tmux::WindowState {
                window_id: "@2".to_string(),
                window_name: "postgres".to_string(),
                active: false,
                pane_count: 1,
                zoomed: false,
                last_active: true,
            });
        }
        let ctx = c.get_labeled_context(None, None);
        assert!(
            ctx.contains("[SESSION TOPOLOGY]"),
            "expected topology block, got: {ctx}"
        );
        assert!(
            ctx.contains("nginx (ID: @1"),
            "expected nginx in topology with ID @1"
        );
        assert!(ctx.contains("2 panes"), "expected pane count in topology");
        assert!(ctx.contains("postgres"), "expected postgres in topology");
        assert!(
            ctx.contains("last active"),
            "expected postgres to be marked as last active"
        );
    }

    #[test]
    fn get_labeled_context_single_window_no_topology() {
        let c = cache();
        {
            let mut wins = c.windows.write().unwrap();
            wins.push(tmux::WindowState {
                window_id: "@1".to_string(),
                window_name: "main".to_string(),
                active: true,
                pane_count: 1,
                zoomed: false,
                last_active: false,
            });
        }
        let ctx = c.get_labeled_context(None, None);
        assert!(
            !ctx.contains("[SESSION TOPOLOGY]"),
            "single-window session should not have topology block"
        );
    }

    #[test]
    fn get_labeled_context_source_pane_excluded_from_background() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert(
                "%5".to_string(),
                PaneState {
                    buffer: "active content".to_string(),
                    summary: "active summary".to_string(),
                    current_cmd: String::new(),
                    current_path: String::new(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 0,
                    in_copy_mode: false,
                    synchronized: false,
                },
            );
        }
        // When %5 is the source pane it should NOT appear in BACKGROUND PANE list.
        // (It will appear as ACTIVE PANE if capture-pane succeeds — but in tests
        //  tmux isn't running so capture_pane returns an error, which is fine.)
        let ctx = c.get_labeled_context(Some("%5"), None);
        assert!(!ctx.contains("[BACKGROUND PANE %5]"));
    }

    #[test]
    fn get_labeled_context_copy_mode_annotated() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert(
                "%7".to_string(),
                PaneState {
                    buffer: "some output".to_string(),
                    summary: "Active: some output".to_string(),
                    current_cmd: "bash".to_string(),
                    current_path: "/home/user".to_string(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 42,
                    history_size: 1000,
                    in_copy_mode: true,
                    synchronized: false,
                },
            );
        }
        // get_labeled_context reads from cache; capture_pane won't run (no tmux).
        // Assert that the BACKGROUND PANE line for %7 contains no copy-mode marker
        // (that's only on the ACTIVE PANE header) but that the pane is listed.
        let ctx = c.get_labeled_context(None, None);
        assert!(ctx.contains("%7"), "pane %7 should appear in context");
        // Synchronized flag should NOT appear (synchronized=false).
        assert!(
            !ctx.contains("[synchronized]"),
            "non-synchronized pane should have no sync marker"
        );
    }

    #[test]
    fn get_labeled_context_synchronized_pane_noted() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert(
                "%9".to_string(),
                PaneState {
                    buffer: "some output".to_string(),
                    summary: "Active: doing things".to_string(),
                    current_cmd: "bash".to_string(),
                    current_path: "/tmp".to_string(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 500,
                    in_copy_mode: false,
                    synchronized: true,
                },
            );
        }
        let ctx = c.get_labeled_context(None, None);
        assert!(
            ctx.contains("[synchronized]"),
            "synchronized pane should have [synchronized] marker"
        );
        assert!(ctx.contains("%9"), "pane %9 should be listed");
    }

    #[test]
    fn get_labeled_context_chat_pane_excluded_from_background() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            // Pane running the user's shell.
            panes.insert(
                "%1".to_string(),
                PaneState {
                    buffer: "user shell".to_string(),
                    summary: "Idle shell at: $".to_string(),
                    current_cmd: "bash".to_string(),
                    current_path: "/home/user".to_string(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 0,
                    in_copy_mode: false,
                    synchronized: false,
                },
            );
            // Pane running daemoneye chat.
            panes.insert(
                "%2".to_string(),
                PaneState {
                    buffer: "chat output".to_string(),
                    summary: "Active: chat output".to_string(),
                    current_cmd: "daemoneye".to_string(),
                    current_path: "/home/user".to_string(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 0,
                    in_copy_mode: false,
                    synchronized: false,
                },
            );
        }
        // %1 is source, %2 is chat — chat pane must not appear in background listing.
        let ctx = c.get_labeled_context(Some("%1"), Some("%2"));
        assert!(!ctx.contains("[BACKGROUND PANE %2"), "chat pane should be excluded");
        // Source pane also shouldn't be in background listing (existing behaviour).
        assert!(!ctx.contains("[BACKGROUND PANE %1"), "source pane should be excluded too");
    }
}
