use crate::tmux;
use crate::util::UnpoisonExt;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
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
    /// Name of the tmux window containing this pane (`#{window_name}`).
    pub window_name: String,
    /// Name of the tmux session that owns this pane (`#{session_name}`).
    /// Equal to the cache's adopted session for "home" panes; other values
    /// mark foreign-session panes, which carry metadata but no captured buffer.
    pub session_name: String,
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
    /// PID of the foreground process in this pane (`#{pane_pid}`).
    /// Captured on every 2 s refresh.  Used as the baseline shell PID for
    /// foreground completion detection: command finished when `pane_pid`
    /// returns to this value.  Zero when unknown.
    pub shell_pid: u32,
    /// Live status classification, re-derived on every 2 s refresh (D2).
    pub status: crate::shell::status::PaneStatus,
}

/// Breadth of a `get_labeled_context` snapshot (M12 D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextScope {
    /// Only panes sharing the chat pane's window.
    Window,
    /// The user's own tmux session — today's behavior, and the default.
    Session,
    /// The home session plus a metadata-only listing of foreign-session panes.
    All,
}

/// True when a pane's window is inside `scope`. Pure, so it is testable
/// without a cache.
pub(crate) fn window_in_scope(
    scope: ContextScope,
    pane_window: &str,
    chat_window: Option<&str>,
) -> bool {
    match scope {
        ContextScope::Window => chat_window.is_none_or(|w| w == pane_window), // window scope keeps only the chat window
        _ => true,
    }
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
    /// Window IDs currently showing a bell (`!` flag) in non-daemon windows.
    ///
    /// Compared on each 2 s refresh to detect bells that the `alert-bell` hook
    /// may have missed (e.g. daemon restart while a bell was pending).
    bell_windows: RwLock<HashSet<String>>,
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
            bell_windows: RwLock::new(HashSet::new()),
        }
    }

    /// Update the session this cache monitors. Called when the first client
    /// connects and tells the daemon which tmux session to observe.
    pub fn set_session(&self, name: &str) {
        *self.session_name.write().unwrap_or_log() = name.to_string();
        // Clear stale state from any previous (empty) session.
        self.panes.write().unwrap_or_log().clear();
        self.windows.write().unwrap_or_log().clear();
        self.bell_windows.write().unwrap_or_log().clear();
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

    /// Remove cached panes not present in the latest `list-panes -a` snapshot.
    ///
    /// `live` is the full set of pane IDs on the server. An EMPTY set is
    /// treated as "snapshot unavailable" and evicts nothing — a transient
    /// `list-panes` failure must not wipe the cache.
    pub fn evict_missing(&self, live: &std::collections::HashSet<String>) {
        if live.is_empty() {
            return;
        }
        self.panes
            .write()
            .unwrap_or_log()
            .retain(|id, _| live.contains(id));
    }

    /// True when `pane_id` is cached AND belongs to the adopted session.
    /// Foreground execution and target hints must only accept home panes.
    pub fn is_home_pane(&self, pane_id: &str) -> bool {
        let home = self.session_name.read().unwrap_or_log().clone();
        self.panes
            .read()
            .unwrap_or_log()
            .get(pane_id)
            .is_some_and(|p| p.session_name == home)
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

        // Collect live pane IDs for eviction (before consuming rich_panes).
        let live_ids: HashSet<String> = rich_panes.iter().map(|p| p.pane_id.clone()).collect();

        // Collect captures outside the lock, then write all results in one acquisition.
        let mut captures: Vec<(crate::tmux::RichPaneInfo, Option<String>)> = Vec::new();
        for info in rich_panes {
            let content = if info.session_name == session {
                tmux::capture_pane(&info.pane_id, 100).ok()
            } else {
                None // foreign pane: metadata only, no capture (D1)
            };
            captures.push((info, content));
        }

        self.apply_captured_panes(captures);

        // Evict panes that no longer exist on the server.
        self.evict_missing(&live_ids);

        // Session environment (P5) — best-effort; ignore errors.
        if let Ok(env) = tmux::session_environment(&session)
            && !env.is_empty()
        {
            let mut env_lock = self.environment.write().unwrap_or_log();
            *env_lock = env;
        }

        self.refresh_topology_and_bells(&session);
        Ok(())
    }

    fn apply_captured_panes(&self, captures: Vec<(crate::tmux::RichPaneInfo, Option<String>)>) {
        // Compute clock and belled-window set before taking the panes lock.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Windows are refreshed later in this same cycle, so bell state lags one
        // 2 s poll — acceptable: every cache field is at most one poll stale.
        // Home-session windows only, so foreign panes never classify as Bell.
        let belled: HashSet<String> = self
            .windows
            .read()
            .unwrap_or_log()
            .iter()
            .filter(|w| w.has_bell())
            .map(|w| w.window_name.clone())
            .collect();

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
                    session_name: String::new(),
                    dead: false,
                    dead_status: None,
                    last_activity: 0,
                    start_cmd: String::new(),
                    pane_index: 0,
                    shell_pid: 0,
                    status: crate::shell::status::PaneStatus::Idle(0),
                });

            entry.current_cmd = info.current_cmd;
            entry.current_path = info.current_path;
            entry.pane_title = info.title;
            entry.scroll_position = info.scroll_position;
            entry.history_size = info.history_size;
            entry.in_copy_mode = info.in_copy_mode;
            entry.synchronized = info.synchronized;
            entry.window_name = info.window_name.clone();
            entry.session_name = info.session_name.clone();
            entry.dead = info.dead;
            entry.dead_status = info.dead_status;
            entry.last_activity = info.last_activity;
            entry.start_cmd = info.start_cmd;
            entry.pane_index = info.pane_index;
            entry.shell_pid = info.pane_pid;

            entry.status = crate::shell::status::classify(
                entry.dead,
                entry.dead_status,
                belled.contains(&entry.window_name),
                &entry.current_cmd,
                entry.last_activity,
                now_secs,
            );

            if let Some(c) = content
                && entry.buffer != c
            {
                entry.buffer = c;
                entry.last_updated = std::time::Instant::now();
            }
            entry.summary = crate::shell::status::summarize(entry.status, &entry.buffer);
        }
    }

    fn refresh_topology_and_bells(&self, session: &str) {
        let Ok(wins) = tmux::list_windows(session) else {
            return;
        };
        // Bell recovery: detect `!` flags on non-daemon windows that the
        // `alert-bell` hook may have missed (e.g. daemon was restarted
        // while a bell was pending).  The `!` flag persists in tmux until
        // the user visits the window, making it reliable for this purpose.
        let newly_belled: Vec<String> = {
            let prev = self.bell_windows.read().unwrap_or_log();
            wins.iter()
                .filter(|w| w.has_bell())
                .filter(|w| !crate::daemon::is_daemon_window(&w.window_name))
                .filter(|w| !prev.contains(&w.window_id))
                .map(|w| w.window_id.clone())
                .collect()
        };
        for window_id in &newly_belled {
            let name = wins
                .iter()
                .find(|w| &w.window_id == window_id)
                .map(|w| w.window_name.as_str())
                .unwrap_or("?");
            log::debug!(
                "Bell recovery: bell detected on window {} ('{}') via window_flags",
                window_id,
                name
            );
            crate::daemon::utils::log_event(
                "bell_recovery",
                serde_json::json!({
                    "window_id": window_id,
                    "window_name": name,
                    "session": session,
                }),
            );
        }
        {
            let mut prev = self.bell_windows.write().unwrap_or_log();
            *prev = wins
                .iter()
                .filter(|w| w.has_bell())
                .map(|w| w.window_id.clone())
                .collect();
        }
        let mut win_lock = self.windows.write().unwrap_or_log();
        *win_lock = wins;
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
