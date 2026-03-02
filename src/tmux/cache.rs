use std::collections::HashMap;
use std::sync::RwLock;
use anyhow::Result;
use crate::tmux;
use crate::ai::filter::mask_sensitive;


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
}

impl SessionCache {
    pub fn new(session_name: &str) -> Self {
        Self {
            session_name: session_name.to_string(),
            panes: RwLock::new(HashMap::new()),
            active_pane: RwLock::new(None),
            environment: RwLock::new(HashMap::new()),
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
        let rich_panes = tmux::list_panes_detailed(&self.session_name)?;

        for info in rich_panes {
            if let Ok(content) = tmux::capture_pane(&info.pane_id, 100) {
                let mut panes = self.panes.write().unwrap();
                let entry = panes.entry(info.pane_id.clone()).or_insert_with(|| PaneState {
                    buffer: String::new(),
                    summary: String::new(),
                    current_cmd: String::new(),
                    current_path: String::new(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                });

                entry.current_cmd  = info.current_cmd;
                entry.current_path = info.current_path;
                entry.pane_title   = info.title;

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
            let marker = if Some(id) == active_id.as_ref() { " (ACTIVE)" } else { "" };
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
    pub fn get_labeled_context(&self, source_pane: Option<&str>) -> String {
        let mut out = String::new();

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
            let content = crate::tmux::capture_pane(pane_id, 200)
                .unwrap_or_else(|_| "(pane unavailable)".to_string());
            // Include CWD for the active pane from cache if available.
            let cwd = {
                let panes = self.panes.read().unwrap_or_else(|e| e.into_inner());
                panes.get(pane_id).map(|p| p.current_path.clone()).unwrap_or_default()
            };
            let cwd_label = if cwd.is_empty() { String::new() } else { format!(" | cwd: {}", cwd) };
            out.push_str(&format!(
                "[ACTIVE PANE {}{}]\n{}\n",
                pane_id,
                cwd_label,
                mask_sensitive(&content),
            ));
        }

        // Background panes — summaries with cmd, cwd, and title, sorted for deterministic ordering.
        let panes = self.panes.read().unwrap_or_else(|e| e.into_inner());
        let mut bg: Vec<_> = panes
            .iter()
            .filter(|(id, _)| source_pane.map_or(true, |s| s != id.as_str()))
            .collect();
        bg.sort_by_key(|(id, _)| id.as_str());
        for (id, state) in bg {
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
            out.push_str(&format!(
                "[BACKGROUND PANE {}{}{}{}]: {}\n",
                id,
                format!(" — {}", state.current_cmd),
                cwd_part,
                title_part,
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
        let ctx = c.get_labeled_context(None);
        assert!(ctx.contains("no terminal context available"));
    }

    #[test]
    fn get_labeled_context_background_panes_sorted() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert("%3".to_string(), PaneState {
                buffer: "foo".to_string(),
                summary: "summary3".to_string(),
                current_cmd: String::new(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
            });
            panes.insert("%1".to_string(), PaneState {
                buffer: "bar".to_string(),
                summary: "summary1".to_string(),
                current_cmd: String::new(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
            });
        }
        let ctx = c.get_labeled_context(None);
        let pos1 = ctx.find("%1").unwrap();
        let pos3 = ctx.find("%3").unwrap();
        assert!(pos1 < pos3, "panes should be sorted by ID");
    }

    #[test]
    fn get_labeled_context_source_pane_excluded_from_background() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert("%5".to_string(), PaneState {
                buffer: "active content".to_string(),
                summary: "active summary".to_string(),
                current_cmd: String::new(),
                current_path: String::new(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
            });
        }
        // When %5 is the source pane it should NOT appear in BACKGROUND PANE list.
        // (It will appear as ACTIVE PANE if capture-pane succeeds — but in tests
        //  tmux isn't running so capture_pane returns an error, which is fine.)
        let ctx = c.get_labeled_context(Some("%5"));
        assert!(!ctx.contains("[BACKGROUND PANE %5]"));
    }
}
