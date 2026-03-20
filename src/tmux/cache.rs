use crate::util::UnpoisonExt;
use crate::ai::filter::mask_sensitive;
use crate::tmux;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::RwLock;
use super::ansi::annotate_ansi;

/// Maximum bytes to read from the tail of a pipe log when using it as the
/// active-pane content source (R1).
const PIPE_LOG_MAX_BYTES: usize = 51_200; // 50 KB

/// When a pipe log grows beyond this size, truncate it to the last
/// `PIPE_LOG_MAX_BYTES` bytes after each read (A7). tmux's `cat` process
/// holds the file open with `O_APPEND`, so subsequent writes go to the
/// new end — no data is corrupted, and at most ~2 s of output is lost.
const PIPE_LOG_ROTATE_THRESHOLD: usize = 10 * 1024 * 1024; // 10 MB

/// Read the last [`PIPE_LOG_MAX_BYTES`] of the pipe log for `pane_id`,
/// annotating ANSI colour escapes as semantic markers.  Returns an empty
/// string if the log does not exist, is empty, or cannot be read.
fn read_pipe_log(pane_id: &str) -> String {
    use std::io::{Read, Seek, SeekFrom, Write};
    let path = tmux::pipe_log_path(pane_id);
    let Ok(mut file) = std::fs::File::open(&path) else { return String::new() };
    let Ok(meta) = file.metadata() else { return String::new() };
    let file_size = meta.len() as usize;
    if file_size == 0 { return String::new(); }
    let offset = file_size.saturating_sub(PIPE_LOG_MAX_BYTES);
    if offset > 0 && file.seek(SeekFrom::Start(offset as u64)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() { return String::new(); }

    // A7: rotate — if the file has grown beyond the threshold, truncate it to
    // just the tail we already hold. The tmux `cat` process keeps the file
    // open with O_APPEND, so future writes land at the new end cleanly.
    if file_size > PIPE_LOG_ROTATE_THRESHOLD {
        drop(file);
        if let Ok(mut wfile) = std::fs::OpenOptions::new()
            .write(true).truncate(true).open(&path)
        {
            if let Err(e) = wfile.write_all(&buf) {
                log::warn!("Failed to rotate pipe log {}: {}", path.display(), e);
            } else {
                log::debug!("Rotated pipe log {} ({} MB → {} KB)",
                    path.display(),
                    file_size / (1024 * 1024),
                    buf.len() / 1024);
            }
        }
    }

    let raw = String::from_utf8_lossy(&buf).into_owned();
    annotate_ansi(&raw)
}

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
    /// Name of the tmux window containing this pane (`#{window_name}`).
    pub window_name: String,
    /// True when the pane's foreground process has exited (remain-on-exit mode).
    pub dead: bool,
    /// Exit code of the foreground process if `dead` is true.
    pub dead_status: Option<i32>,
    /// Unix timestamp of the last time this pane produced output (`#{pane_activity}`, N4).
    /// Zero when unknown or never active.
    pub last_activity: u64,
    /// The command the pane was originally created with (`#{pane_start_command}`, N5).
    /// Empty when not recorded by tmux.
    pub start_cmd: String,
    /// Window-relative pane index (0-based) as shown by `ctrl+a q` / `tmux display-panes`.
    pub pane_index: usize,
}

/// Shared, periodically-refreshed view of all panes in a tmux session.
///
/// The daemon spawns a background task that calls [`SessionCache::refresh`]
/// every 2 seconds.  Request handlers read from the cache without blocking on
/// live tmux calls, keeping response latency low.
///
/// `session_name` is a `RwLock<String>` so it can be adopted from the first
/// connecting client when the daemon starts without an active tmux session
/// (e.g. launched via systemd before the user logs in).
pub struct SessionCache {
    pub session_name: RwLock<String>,
    /// Pane ID → state map; access via the `RwLock`.
    pub panes: RwLock<HashMap<String, PaneState>>,
    /// The currently-active pane ID as reported by `tmux display-message`.
    pub active_pane: RwLock<Option<String>>,
    /// High-signal tmux session environment variables (allowlisted subset).
    pub environment: RwLock<HashMap<String, String>>,
    /// Window-level topology for the session (P4).
    pub windows: RwLock<Vec<tmux::WindowState>>,
    /// Attached terminal client dimensions in columns × rows (N7). (0, 0) = unknown.
    pub client_size: RwLock<(u16, u16)>,
}

impl SessionCache {
    pub fn new(session_name: &str) -> Self {
        Self {
            session_name: RwLock::new(session_name.to_string()),
            panes: RwLock::new(HashMap::new()),
            active_pane: RwLock::new(None),
            environment: RwLock::new(HashMap::new()),
            windows: RwLock::new(Vec::new()),
            client_size: RwLock::new((0, 0)),
        }
    }

    /// Update the session this cache monitors. Called when the first client
    /// connects and tells the daemon which tmux session to observe.
    pub fn set_session(&self, name: &str) {
        *self.session_name.write().unwrap_or_log() = name.to_string();
        // Clear stale pane state from any previous (empty) session.
        self.panes.write().unwrap_or_log().clear();
        self.windows.write().unwrap_or_log().clear();
    }

    /// Instantly update the active pane without waiting for the next 2 s poll.
    ///
    /// Called by the `NotifyFocus` handler when a `pane-focus-in` hook fires (N1).
    pub fn set_active_pane(&self, pane_id: &str) {
        *self.active_pane.write().unwrap_or_log() = Some(pane_id.to_string());
    }

    /// Update the cached client viewport dimensions.
    ///
    /// Called by the `NotifyResize` handler when a `client-resized` hook fires (N8),
    /// and at session-hook install time to seed the initial value (N7).
    /// A width or height of 0 means "unknown" and suppresses the context block.
    pub fn set_client_size(&self, width: u16, height: u16) {
        *self.client_size.write().unwrap_or_log() = (width, height);
    }

    /// Refresh only the window topology list.
    ///
    /// Called by the `NotifyWindowChanged` handler when a `session-window-changed`
    /// hook fires (N2).  Faster than a full `refresh()` since it skips all pane
    /// captures and environment re-reads.
    pub fn refresh_windows(&self) {
        let session = self.session_name.read().unwrap_or_log().clone();
        if session.is_empty() {
            return;
        }
        if let Ok(wins) = tmux::list_windows(&session) {
            *self.windows.write().unwrap_or_log() = wins;
        }
    }


    /// Refresh the cache.
    ///
    /// Uses a single `list-panes` call to fetch all pane metadata (P3), then
    /// issues one `capture-pane` per pane for buffer content.  Session
    /// environment is refreshed on each cycle (P5).
    pub fn refresh(&self) -> Result<()> {
        let session = self.session_name.read().unwrap_or_log().clone();
        if session.is_empty() {
            return Ok(()); // No session adopted yet — nothing to refresh.
        }

        // Active pane.
        let active = tmux::get_active_pane(&session)?;
        {
            let mut active_lock = self.active_pane.write().unwrap_or_log();
            *active_lock = Some(active);
        }

        // All pane metadata in one tmux call (P1 + P2 + P3).
        let rich_panes = tmux::list_panes_detailed().unwrap_or_default();

        // Collect captures outside the lock, then write all results in one acquisition.
        let mut captures: Vec<(crate::tmux::RichPaneInfo, String)> = Vec::new();
        for info in rich_panes {
            if info.session_name != session {
                continue;
            }
            if let Ok(content) = tmux::capture_pane(&info.pane_id, 100) {
                captures.push((info, content));
            }
        }

        {
            let mut panes = self.panes.write().unwrap_or_log();
            for (info, content) in captures {
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
                        window_name: String::new(),
                        dead: false,
                        dead_status: None,
                        last_activity: 0,
                        start_cmd: String::new(),
                        pane_index: 0,
                    });

                entry.current_cmd = info.current_cmd;
                entry.current_path = info.current_path;
                entry.pane_title = info.title;
                entry.scroll_position = info.scroll_position;
                entry.history_size = info.history_size;
                entry.in_copy_mode = info.in_copy_mode;
                entry.synchronized = info.synchronized;
                entry.window_name = info.window_name.clone();
                entry.dead = info.dead;
                entry.dead_status = info.dead_status;
                entry.last_activity = info.last_activity;
                entry.start_cmd = info.start_cmd;
                entry.pane_index = info.pane_index;

                if entry.buffer != content {
                    entry.buffer = content;
                    entry.summary = self.summarize(&entry.buffer);
                    entry.last_updated = std::time::Instant::now();
                }
            }
        }

        // Session environment (P5) — best-effort; ignore errors.
        if let Ok(env) = tmux::session_environment(&session) {
            if !env.is_empty() {
                let mut env_lock = self.environment.write().unwrap_or_log();
                *env_lock = env;
            }
        }

        // Window topology (P4) — best-effort; ignore errors.
        if let Ok(wins) = tmux::list_windows(&session) {
            let mut win_lock = self.windows.write().unwrap_or_log();
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
        let Some(last_line) = buffer.lines().filter(|l| !l.trim().is_empty()).last() else {
            return "Empty pane".to_string();
        };
        let last_line = last_line.trim();

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
            let wins = self.windows.read().unwrap_or_log();
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
            let env = self.environment.read().unwrap_or_log();
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

        // Client viewport (N7) — prepend when dimensions are known.
        {
            let (w, h) = *self.client_size.read().unwrap_or_log();
            if w > 0 && h > 0 {
                out.push_str(&format!("[CLIENT VIEWPORT] {}x{}\n", w, h));
            }
        }

        // Active pane — full capture, explicitly labelled.
        if let Some(pane_id) = source_pane {
            // Pull CWD, command, title, scroll position, mode flags, and pane index from cache in one lock.
            let (cwd, cmd, title, scroll_pos, in_copy_mode, pane_idx, window_name_for_active) = {
                let panes = self.panes.read().unwrap_or_log();
                if let Some(p) = panes.get(pane_id) {
                    (p.current_path.clone(), p.current_cmd.clone(), p.pane_title.clone(),
                     p.scroll_position, p.in_copy_mode, p.pane_index, p.window_name.clone())
                } else {
                    (String::new(), String::new(), String::new(), 0usize, false, 0usize, String::new())
                }
            };

            // R1: prefer the pipe log over capture-pane when the pane is not
            // scrolled and the log has content.  The pipe log covers all output
            // since the chat session started, including content that has scrolled
            // past the tmux scrollback buffer.  When the user is actively looking
            // at a different scroll position (R3) we still use capture-pane since
            // the user's viewport is the meaningful reference point.
            // R2: use ANSI-escape-aware capture so colour codes are converted to
            // semantic markers ([ERROR:], [WARN:], [OK:]).  The pipe log already
            // contains raw ANSI (R1) and goes through annotate_ansi() in
            // read_pipe_log().  For capture-pane paths we use the `-e` variant
            // which asks tmux to preserve escape sequences.
            let content = if scroll_pos > 0 {
                crate::tmux::capture_pane_at_scroll_with_escapes(pane_id, scroll_pos, 200)
                    .map(|s| annotate_ansi(&s))
                    .or_else(|_| {
                        crate::tmux::capture_pane_with_escapes(pane_id, 200)
                            .map(|s| annotate_ansi(&s))
                    })
                    .unwrap_or_else(|_| "(pane unavailable)".to_string())
            } else {
                let pipe_content = read_pipe_log(pane_id);
                if pipe_content.is_empty() {
                    crate::tmux::capture_pane_with_escapes(pane_id, 200)
                        .map(|s| annotate_ansi(&s))
                        .unwrap_or_else(|_| "(pane unavailable)".to_string())
                } else {
                    pipe_content
                }
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

            let idx_label = if window_name_for_active.is_empty() {
                format!(" | idx:{}", pane_idx)
            } else {
                format!(" | idx:{} in '{}'", pane_idx, window_name_for_active)
            };
            out.push_str(&format!(
                "[ACTIVE PANE {}{}{}{}{}{}]\n{}\n",
                pane_id,
                idx_label,
                cwd_label,
                title_label,
                scroll_note,
                copy_note,
                mask_sensitive(&content),
            ));
        }

        // Non-active panes — classified by their relationship to the chat window.
        //
        // - VISIBLE PANE:    same window as the chat pane (user can see it in the split)
        // - BACKGROUND PANE: daemon-launched window (de-bg-* / de-sched-*)
        // - SESSION PANE:    any other user window (SSH sessions, editors, etc.)
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let panes = self.panes.read().unwrap_or_log();

        // Determine which window contains the chat pane so we can identify visible peers.
        let chat_window: Option<&str> = chat_pane
            .and_then(|cp| panes.get(cp))
            .map(|p| p.window_name.as_str())
            .filter(|w| !w.is_empty());

        let mut others: Vec<_> = panes
            .iter()
            .filter(|(id, _)| source_pane.map_or(true, |s| s != id.as_str()))
            .filter(|(id, _)| chat_pane.map_or(true, |c| c != id.as_str()))
            .collect();
        others.sort_by_key(|(id, _)| id.as_str());
        for (id, state) in others {
            let pane_label = if chat_window.map_or(false, |cw| cw == state.window_name) {
                "VISIBLE PANE"
            } else if state.window_name.starts_with("de-bg-")
                || state.window_name.starts_with("de-sched-")
            {
                "BACKGROUND PANE"
            } else {
                "SESSION PANE"
            };
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
            let sync_part = if state.synchronized {
                " [synchronized]".to_string()
            } else {
                String::new()
            };
            // For dead panes, fold the idle-since-completion time into the [dead:] tag
            // so the agent sees a single clear signal: [dead: 0, idle 8m].
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
            // N4: annotate how recently the pane produced output.
            // Dead panes already show elapsed time in dead_part above.
            let activity_part = if !state.dead && state.last_activity > 0 && now_secs >= state.last_activity {
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
            // N5: show start_cmd when it differs from current_cmd (e.g. "ssh -t host bash" vs "bash").
            let start_part = if !state.start_cmd.is_empty() && state.start_cmd != state.current_cmd {
                format!(" (started: {})", state.start_cmd)
            } else {
                String::new()
            };
            let idx_part = format!(" (idx:{} in '{}')", state.pane_index, state.window_name);
            out.push_str(&format!(
                "[{} {}{}{}{}{}{}{}{}{}]: {}\n",
                pane_label,
                id,
                idx_part,
                format!(" — {}", state.current_cmd),
                start_part,
                cwd_part,
                title_part,
                sync_part,
                dead_part,
                activity_part,
                mask_sensitive(&state.summary),
            ));
        }

        if out.is_empty() {
            out.push_str("(no terminal context available)");
        } else {
            // Other sessions (N16) — append when there is already meaningful context.
            // Skipped when out is empty so it doesn't interfere with the fallback sentinel.
            let session_name = self.session_name.read().unwrap_or_log().clone();
            let other_ctx = tmux::other_sessions_context(&session_name);
            if !other_ctx.is_empty() {
                out.push_str(&other_ctx);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SessionCache tests ---

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
    fn get_labeled_context_client_viewport_shown_when_known() {
        let c = cache();
        c.set_client_size(220, 50);
        // Need at least one pane so output is non-empty.
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert("%1".to_string(), PaneState {
                buffer: String::new(), summary: "shell".to_string(),
                current_cmd: "bash".to_string(), current_path: "/home/user".to_string(),
                pane_title: String::new(), last_updated: std::time::Instant::now(),
                scroll_position: 0, history_size: 0, in_copy_mode: false,
                synchronized: false, window_name: "main".to_string(),
                dead: false, dead_status: None, last_activity: 0, start_cmd: String::new(), pane_index: 0,
            });
        }
        let ctx = c.get_labeled_context(None, None);
        assert!(ctx.contains("[CLIENT VIEWPORT] 220x50"), "expected viewport block, got: {ctx}");
    }

    #[test]
    fn get_labeled_context_client_viewport_absent_when_zero() {
        let c = cache();
        // Default is (0, 0) — no viewport block should appear.
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert("%1".to_string(), PaneState {
                buffer: String::new(), summary: "shell".to_string(),
                current_cmd: "bash".to_string(), current_path: "/home/user".to_string(),
                pane_title: String::new(), last_updated: std::time::Instant::now(),
                scroll_position: 0, history_size: 0, in_copy_mode: false,
                synchronized: false, window_name: "main".to_string(),
                dead: false, dead_status: None, last_activity: 0, start_cmd: String::new(), pane_index: 0,
            });
        }
        let ctx = c.get_labeled_context(None, None);
        assert!(!ctx.contains("[CLIENT VIEWPORT]"), "viewport block should be absent when (0,0)");
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
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
    fn get_labeled_context_dead_pane_noted() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            panes.insert(
                "%11".to_string(),
                PaneState {
                    buffer: "some output".to_string(),
                    summary: "Active: job finished".to_string(),
                    current_cmd: "bash".to_string(),
                    current_path: "/tmp".to_string(),
                    pane_title: String::new(),
                    last_updated: std::time::Instant::now(),
                    scroll_position: 0,
                    history_size: 100,
                    in_copy_mode: false,
                    synchronized: false,
                    window_name: "de-bg-myjob".to_string(),
                    dead: true,
                    dead_status: Some(1),
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
                },
            );
        }
        let ctx = c.get_labeled_context(None, None);
        assert!(
            ctx.contains("[dead: 1]"),
            "dead pane should have [dead: 1] marker, got: {ctx}"
        );
        assert!(ctx.contains("%11"), "pane %11 should be listed");
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
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
                    window_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
                },
            );
        }
        // %1 is source, %2 is chat — chat pane must not appear in background listing.
        let ctx = c.get_labeled_context(Some("%1"), Some("%2"));
        assert!(!ctx.contains("[BACKGROUND PANE %2"), "chat pane should be excluded");
        // Source pane also shouldn't be in background listing (existing behaviour).
        assert!(!ctx.contains("[BACKGROUND PANE %1"), "source pane should be excluded too");
    }

    #[test]
    fn get_labeled_context_pane_classification() {
        let c = cache();
        {
            let mut panes = c.panes.write().unwrap();
            // Chat pane — window "work".
            panes.insert("%2".to_string(), PaneState {
                buffer: String::new(), summary: String::new(),
                current_cmd: "daemoneye".to_string(), current_path: String::new(),
                pane_title: String::new(), last_updated: std::time::Instant::now(),
                scroll_position: 0, history_size: 0, in_copy_mode: false,
                synchronized: false, window_name: "work".to_string(),
                dead: false, dead_status: None, last_activity: 0, start_cmd: String::new(), pane_index: 0,
            });
            // Visible peer — same window as chat.
            panes.insert("%3".to_string(), PaneState {
                buffer: String::new(), summary: "shell".to_string(),
                current_cmd: "bash".to_string(), current_path: "/home/user".to_string(),
                pane_title: String::new(), last_updated: std::time::Instant::now(),
                scroll_position: 0, history_size: 0, in_copy_mode: false,
                synchronized: false, window_name: "work".to_string(),
                dead: false, dead_status: None, last_activity: 0, start_cmd: String::new(), pane_index: 0,
            });
            // Daemon-launched background window.
            panes.insert("%5".to_string(), PaneState {
                buffer: String::new(), summary: "running".to_string(),
                current_cmd: "bash".to_string(), current_path: "/tmp".to_string(),
                pane_title: String::new(), last_updated: std::time::Instant::now(),
                scroll_position: 0, history_size: 0, in_copy_mode: false,
                synchronized: false, window_name: "de-bg-myjob".to_string(),
                dead: false, dead_status: None, last_activity: 0, start_cmd: String::new(), pane_index: 0,
            });
            // User's session pane in a different window.
            panes.insert("%7".to_string(), PaneState {
                buffer: String::new(), summary: "ssh idle".to_string(),
                current_cmd: "ssh".to_string(), current_path: "~".to_string(),
                pane_title: "web01".to_string(), last_updated: std::time::Instant::now(),
                scroll_position: 0, history_size: 0, in_copy_mode: false,
                synchronized: false, window_name: "servers".to_string(),
                dead: false, dead_status: None, last_activity: 0, start_cmd: String::new(), pane_index: 0,
            });
        }
        // No source pane; chat pane is %2.
        let ctx = c.get_labeled_context(None, Some("%2"));
        assert!(!ctx.contains("%2"), "chat pane should be excluded entirely");
        assert!(ctx.contains("[VISIBLE PANE %3"),   "peer in same window should be VISIBLE PANE");
        assert!(ctx.contains("[BACKGROUND PANE %5"), "de-bg-* window should be BACKGROUND PANE");
        assert!(ctx.contains("[SESSION PANE %7"),   "other user window should be SESSION PANE");
    }
}
