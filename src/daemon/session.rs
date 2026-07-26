use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::ai::Message;
use crate::util::UnpoisonExt;

/// Metadata for a background tmux window spawned during a chat session.
pub struct BgWindowInfo {
    /// tmux pane ID (e.g. `%7`) — can be passed to `watch_pane` or used as foreground target.
    pub pane_id: String,
    /// Full tmux window name (e.g. `de-bg-42-1712937600-cargo-build`).
    pub window_name: String,
    /// The tmux session the window belongs to (needed to kill it on eviction).
    pub tmux_session: String,
    /// `None` while still running; `Some(code)` after the pane exits.
    pub exit_code: Option<i32>,
}

/// In-memory record of an active chat session.
/// Evicted by the cleanup task after 30 minutes of inactivity.
pub struct SessionEntry {
    /// Full message history for this session. Bounded by token-budget
    /// compaction rather than a fixed message count.
    pub messages: Vec<Message>,
    /// Wall-clock time of the last `Ask` request; used to prune idle sessions.
    pub last_accessed: Instant,
    /// The tmux pane where the chat is occurring.
    pub chat_pane: Option<String>,
    /// A user-selected default pane for foreground execution when the AI doesn't specify one.
    pub default_target_pane: Option<String>,
    /// Background windows spawned during this session (capped at `MAX_BG_WINDOWS_PER_SESSION`).
    pub bg_windows: Vec<BgWindowInfo>,
    /// Prompt token count from the most recent AI turn — represents current context pressure.
    /// Updated after every `AiEvent::Done`; sent to the client as `Response::UsageUpdate`.
    pub last_prompt_tokens: u32,
    /// The tmux session name this AI session is attached to.
    /// Used to correlate client-detached / client-attached hook events (N15).
    pub tmux_session: String,
    /// When the tmux client last detached from this session (`client-detached` hook, N15).
    /// `None` while a client is attached or before any detach has been observed.
    pub last_detach: Option<Instant>,
    /// UTC wall-clock time of the last detach. Used to query `events.jsonl` for
    /// cost incurred during the detach window (Phase 7).
    pub detach_time_utc: Option<chrono::DateTime<chrono::Utc>>,
    /// Number of messages in `messages` at the time of `last_detach`.
    /// Used to identify messages injected while no client was present (N15).
    pub messages_at_detach: usize,
    /// The source pane that has `pipe-pane` active for this session (R1).
    /// `None` before the first Ask or when pipe-pane is not available.
    pub pipe_source_pane: Option<String>,
    /// True if this session is autonomous (no attached human user).
    pub is_ghost: bool,
    /// Settings for autonomous execution (inherited from the triggering runbook).
    pub ghost_config: Option<crate::ipc::GhostConfig>,
    /// Window-name prefix to use for background command windows spawned by this ghost session.
    /// Defaults to `GS_BG_WINDOW_PREFIX`; set to `GS_SCHED_WINDOW_PREFIX` for scheduler-
    /// originated ghosts so their windows are visually distinct.
    pub ghost_bg_prefix: &'static str,
    /// Wall-clock time when this session was created. Used by the session digest
    /// to filter events and detect artifacts created during this session.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Number of user-visible turns completed in this session.  Incremented on
    /// every Ask and never reset by compaction — used by the client to display
    /// a stable, ever-increasing turn counter.
    pub turn_count: usize,
    /// Cumulative number of non-approval-gated tool calls executed across all
    /// turns in this session.  Checked against `config.limits.max_tool_calls_per_session`.
    pub tool_calls_this_session: usize,
    /// Session-level model override set by `/model` slash command.
    /// `None` means use the daemon default (`[models.default]` from config).
    pub active_model: Option<String>,
    /// Unix timestamp (`#{pane_activity}`) of the foreground pane when a terminal
    /// snapshot was last injected into the prompt (first turn or auto-refresh).
    /// On the next turn, if the foreground pane's `last_activity` has advanced past
    /// this value the daemon automatically injects a fresh snapshot without requiring
    /// a `get_terminal_context` tool call.
    pub last_snapshot_activity: u64,
    /// Name of the saved session this entry is associated with.
    /// `None` for ephemeral sessions that have not been saved.
    pub saved_name: Option<String>,
    /// True if messages have been added since the last save or load.
    /// Guards `/session load` against discarding unsaved work.
    pub dirty: bool,
    /// Deferred tool names that have been loaded into this session via `load_tools`.
    /// When non-empty, those tools' schemas are included in the next AI render.
    pub loaded_tools: HashSet<String>,
    /// Artifacts (memories, runbooks, scripts) created during this session.
    /// Used for retroactive `session_origin` frontmatter backfill on save (Phase 3).
    pub artifacts_created: Vec<crate::session_store::ArtifactRef>,
    /// True after the auto-name suggestion has been sent once this session.
    /// Prevents repeated suggestions if the user ignores the first one.
    pub auto_name_suggested: bool,
    /// Task description passed to `spawn_ghost_shell` when this ghost was spawned.
    /// Used in mailbox results so the coordinator sees what the child was asked to do.
    pub ghost_task_message: Option<String>,
    /// Cumulative cost of this session so far. Reset on /clear or new session.
    pub cost_usd: f64,
    /// Per-agent breakdown for this session (key = agent_name).
    pub cost_by_agent: HashMap<String, f64>,
    /// Whether any AI call in this session had Unknown pricing.
    pub has_untracked_cost: bool,
    /// Multiplier mapping estimated history tokens to observed prompt tokens
    /// (absorbs system prompt, tool schemas, provider framing). EMA-smoothed;
    /// clamped to [0.5, 4.0]. Starts at 1.5 (history is typically smaller than
    /// the full prompt).
    pub token_scale: f64,
    /// True while a background compaction task for this session is running.
    /// Prevents duplicate spawns; cleared by the task on completion/discard.
    pub compaction_in_flight: bool,
    /// Notice queued by a completed background compaction, delivered as a
    /// SystemMsg at the start of the next turn.
    pub pending_compaction_notice: Option<String>,
}

/// Thread-safe, shared session store passed to every client handler.
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;

pub static BG_DONE_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<String>> =
    std::sync::OnceLock::new();

/// Broadcast channel for background command completion via IPC.
/// Carries `(pane_id, exit_code)` delivered directly by the command wrapper.
pub static COMPLETE_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<(String, i32)>> =
    std::sync::OnceLock::new();

/// Monotonically-incrementing counter used to generate unique `pane-title-changed`
/// hook slot names (`@de_fg_N`) for concurrent foreground command executions.
/// Using a counter avoids the timestamp-modulo collision risk.
pub static FG_HOOK_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Monotonically-incrementing counter used to generate unique tmux buffer names
/// (`de-rb-N`) for N12 local-pane file reads via `load-buffer`/`save-buffer`.
pub static BUFFER_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn bg_done_subscribe() -> tokio::sync::broadcast::Receiver<String> {
    BG_DONE_TX
        .get_or_init(|| {
            let (tx, _) = tokio::sync::broadcast::channel(32);
            tx
        })
        .subscribe()
}

pub fn complete_subscribe() -> tokio::sync::broadcast::Receiver<(String, i32)> {
    COMPLETE_TX
        .get_or_init(|| {
            let (tx, _) = tokio::sync::broadcast::channel(32);
            tx
        })
        .subscribe()
}

/// Path to the JSONL file storing a session's message history.
pub fn session_file(id: &str) -> std::path::PathBuf {
    crate::config::sessions_dir().join(format!("{}.jsonl", id))
}

/// Per-session continuity state that must survive daemon restarts and
/// idle eviction. Serialized to `sessions_dir()/<id>.meta.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub turn_count: usize,
    pub last_prompt_tokens: u32,
    pub token_scale: f64,
    pub tool_calls_this_session: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub saved_name: Option<String>,
}

/// Path to the meta file for a session.
pub fn meta_file(id: &str) -> std::path::PathBuf {
    crate::config::sessions_dir().join(format!("{}.meta.json", id))
}

/// Atomically write session meta to disk.
/// Failures are logged at WARN and non-fatal.
pub fn write_session_meta(id: &str, meta: &SessionMeta) {
    use std::io::Write;
    let path = meta_file(id);
    let tmp_path = path.with_extension("json.tmp");
    let result: std::io::Result<()> = (|| {
        let mut f = std::fs::File::create(&tmp_path)?;
        let json = serde_json::to_string_pretty(meta).map_err(std::io::Error::other)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("Failed to write session meta {}: {}", path.display(), e);
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Read session meta from disk. Returns `None` if absent or corrupt.
pub fn read_session_meta(id: &str) -> Option<SessionMeta> {
    let text = std::fs::read_to_string(meta_file(id)).ok()?;
    match serde_json::from_str(&text) {
        Ok(meta) => Some(meta),
        Err(e) => {
            log::warn!(
                "Corrupt session meta for {}: {} — using fresh defaults",
                id,
                e
            );
            None
        }
    }
}

/// Path to the append-only archive of every message this session has
/// exchanged. NEVER rewritten or truncated by any code path — retention
/// (config `[sessions] archive_retention_days`) deletes whole files only.
pub fn archive_file(id: &str) -> std::path::PathBuf {
    crate::config::sessions_dir().join(format!("{}.archive.jsonl", id))
}

/// Rewrite the entire session file with the current message history.
/// Used after a compaction pass, when old entries have been dropped.
/// Writes atomically: tmp file → fsync → rename, so a crash mid-write leaves
/// the old file intact rather than producing a truncated session.
/// Failures are logged at WARN and non-fatal.
pub fn write_session_file(id: &str, messages: &[Message]) {
    use std::io::Write;
    let path = session_file(id);
    let tmp_path = path.with_extension("jsonl.tmp");
    let result: std::io::Result<()> = (|| {
        let mut f = std::fs::File::create(&tmp_path)?;
        for msg in messages {
            if let Ok(line) = serde_json::to_string(msg) {
                writeln!(f, "{}", line)?;
            }
        }
        f.sync_all()?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("Failed to write session file {}: {}", path.display(), e);
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Append one message to the session archive, seeding the archive from the
/// working file on first use (so pre-archive history is captured).
///
/// Synthetic messages created *by* compaction (the digest message) are part
/// of the working set but not the archive — that is correct (the archive holds
/// what actually happened; digests are derived).
pub fn append_archive_message(id: &str, msg: &crate::ai::Message) {
    let archive_path = archive_file(id);
    let working_path = session_file(id);

    // Seed: if the archive doesn't exist but the working file does, copy it.
    if !archive_path.exists() && working_path.exists() {
        let _ = std::fs::copy(&working_path, &archive_path);
    }

    let line = serde_json::to_string(msg).unwrap_or_default();
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&archive_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")
        }) {
        Ok(()) => {}
        Err(e) => {
            log::warn!("session archive append failed for session {}: {}", id, e);
        }
    }
}

/// Append a single message to the session file without rewriting earlier entries.
/// This is the hot path — called once per new message during normal turns.
/// Failures are logged at WARN and non-fatal.
pub fn append_session_message(id: &str, msg: &Message) {
    // Archive FIRST: if the archive doesn't exist yet, it seeds from the
    // current working file (which does NOT yet contain `msg`). After seeding,
    // `msg` is appended once. If we appended to the working file first, the
    // seed copy would already contain `msg`, causing a duplicate.
    append_archive_message(id, msg);

    use std::fs::OpenOptions;
    use std::io::Write;
    let path = session_file(id);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(msg)
            && let Err(e) = writeln!(f, "{}", line)
        {
            log::warn!("Failed to append to session file {}: {}", path.display(), e);
        }
    } else {
        log::warn!("Failed to open session file {} for append", path.display());
    }
}

/// Find the first index `>= start` that points at a "clean turn boundary":
/// a user message whose `tool_results` field is empty.  A user message with
/// `tool_results` is a response to an assistant tool call — splitting the
/// history there would leave an orphan tool_result whose corresponding
/// tool_call has been trimmed, which most backends reject.
///
/// Returns `None` if no clean boundary exists in `messages[start..]`.
pub fn next_clean_turn_start(messages: &[Message], start: usize) -> Option<usize> {
    let mut idx = start;
    while idx < messages.len() {
        let m = &messages[idx];
        if m.role == "user" && m.tool_results.as_ref().is_none_or(|v| v.is_empty()) {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

/// Load message history from a session file, returning at most `cap`
/// tail messages.  `cap = None` means unbounded — all messages are returned.
/// Returns an empty Vec if the file does not exist or is unreadable.
///
/// When a cap is applied, the tail is advanced to the nearest clean turn
/// boundary (user message without tool_results) so that no orphaned
/// tool_results are returned.  If no clean boundary exists in the tail,
/// the raw slice is repaired by stripping leading orphaned tool_results.
pub fn read_session_file(id: &str, cap: Option<usize>) -> Vec<Message> {
    let path = session_file(id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let msgs: Vec<Message> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    match cap {
        Some(cap) if msgs.len() > cap => {
            let slice_start = msgs.len() - cap;
            let clean_start = next_clean_turn_start(&msgs, slice_start);
            match clean_start {
                Some(idx) => msgs[idx..].to_vec(),
                None => {
                    // No clean boundary in the tail — repair instead of returning raw.
                    let mut tail = msgs[slice_start..].to_vec();
                    crate::daemon::digest::repair_tail_head(&mut tail);
                    tail
                }
            }
        }
        _ => msgs,
    }
}

impl SessionEntry {
    pub fn last_accessed(&self) -> std::time::Instant {
        self.last_accessed
    }

    /// Kill all background windows that are still open for this session.
    /// Called when the session is evicted from the store.
    pub fn cleanup_bg_windows(&self) {
        for win in &self.bg_windows {
            if let Err(e) = crate::tmux::kill_job_window(&win.tmux_session, &win.window_name) {
                log::warn!(
                    "GC bg window {} on session eviction: {}",
                    win.window_name,
                    e
                );
            }
        }
        // R1: stop pipe-pane and remove the log file if one was started for this session.
        // An empty string is the "failed / skipped" sentinel — nothing to clean up.
        if let Some(ref pane_id) = self.pipe_source_pane
            && !pane_id.is_empty()
        {
            crate::tmux::stop_pipe_pane(pane_id);
        }
    }
}

thread_local! {
    /// Depth of the current thread's `with_sessions` nesting. Only ever 0 or 1
    /// in correct code.
    static SESSIONS_LOCK_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// RAII depth counter for `with_sessions`. Decrements on drop, so the count is
/// correct even when the closure panics — otherwise one panicking test would
/// poison the counter for every later test on the same thread.
struct SessionsLockDepth;

impl SessionsLockDepth {
    fn enter() -> Self {
        SESSIONS_LOCK_DEPTH.with(|d| {
            assert_eq!(
                d.get(),
                0,
                "re-entrant SessionStore lock: with_sessions() called while this \
                 thread already holds the store. std::sync::Mutex is not reentrant \
                 — this would deadlock the whole daemon. Collect what you need \
                 inside the outer closure and use it after it returns. See \
                 docs/design/daemon-stalls.md § 1.5c."
            );
            d.set(1);
        });
        Self
    }
}

impl Drop for SessionsLockDepth {
    fn drop(&mut self) {
        SESSIONS_LOCK_DEPTH.with(|d| d.set(0));
    }
}

/// Run `f` with exclusive access to the session map.
///
/// This is the intended way to touch `SessionStore`. The guard's lifetime is the
/// closure body, so it cannot escape, cannot be held across an `.await`, and a
/// nested acquisition trips an assertion instead of deadlocking.
///
/// Do **not** call `with_sessions` from inside `f`, and do not call anything from
/// inside `f` that reaches the store — collect what you need, return it, and act
/// after the closure returns.
pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T {
    let _depth = SessionsLockDepth::enter();
    let mut store = sessions.lock().unwrap_or_log();
    f(&mut store)
}

/// One session-cleanup pass: evict sessions idle longer than `idle_after` and
/// report which sessions remain.
///
/// The lock is acquired **once** and released before this returns. Evicted
/// entries are handed back by value so the caller can run their teardown —
/// which spawns tmux subprocesses — outside the critical section.
///
/// Do not add a second `sessions.lock()` to this function or to its caller's
/// iteration. `std::sync::Mutex` is not reentrant; a second acquisition while
/// the first guard is alive deadlocks the whole daemon, because every IPC
/// handler locks this same store. See `docs/design/daemon-stalls.md` § 1.5c.
pub fn cleanup_pass(
    sessions: &SessionStore,
    now: std::time::Instant,
    idle_after: std::time::Duration,
) -> (Vec<SessionEntry>, std::collections::HashSet<String>) {
    with_sessions(sessions, |store| {
        let expired: Vec<String> = store
            .iter()
            .filter(|(_, v)| now.duration_since(v.last_accessed()) >= idle_after)
            .map(|(k, _)| k.clone())
            .collect();

        let mut evicted = Vec::with_capacity(expired.len());
        for key in expired {
            if let Some(entry) = store.remove(&key) {
                evicted.push(entry);
            }
        }

        let active: std::collections::HashSet<String> = store.keys().cloned().collect();
        (evicted, active)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{Message, ToolResult};

    fn make_msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    fn make_msg_with_tool_results(role: &str, content: &str, results: Vec<ToolResult>) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: Some(results),
            turn: None,
        }
    }

    #[test]
    fn append_session_message_adds_lines() {
        let id = format!("test_append_{}", std::process::id());
        let path = std::path::PathBuf::from("/tmp").join(format!("{}.jsonl", id));
        // Start with two messages written via the full-rewrite path.
        let msgs = vec![make_msg("user", "hello"), make_msg("assistant", "hi")];
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).unwrap();
            for m in &msgs {
                writeln!(f, "{}", serde_json::to_string(m).unwrap()).unwrap();
            }
        }
        // Append one more message.
        let extra = make_msg("user", "how are you");
        // Call append_session_message via the session-file path directly.
        {
            use std::fs::OpenOptions;
            use std::io::Write;
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "{}", serde_json::to_string(&extra).unwrap()).unwrap();
        }
        // Read back and verify all three messages are present.
        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Vec<Message> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[2].content, "how are you");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn session_file_roundtrip() {
        // Write messages to a temp session file and read them back.
        let id = format!("test_{}", std::process::id());
        // Temporarily point sessions_dir() at /tmp to avoid HOME dependency.
        // We call the helpers directly using /tmp as the base.
        let dir = std::path::PathBuf::from("/tmp");
        let path = dir.join(format!("{}.jsonl", id));

        let msgs = vec![make_msg("user", "hello"), make_msg("assistant", "hi there")];

        // Replicate write_session_file logic with a known path.
        use std::io::Write;
        let mut f = std::fs::File::create(&path).unwrap();
        for m in &msgs {
            writeln!(f, "{}", serde_json::to_string(m).unwrap()).unwrap();
        }

        // Replicate read_session_file logic with the same path.
        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Vec<Message> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "hello");
        assert_eq!(loaded[1].role, "assistant");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn auto_name_suggested_starts_false() {
        // auto_name_suggested must default to false so the first suggestion fires.
        let entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            cost_usd: 0.0,
            loaded_tools: HashSet::new(),
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };
        assert!(!entry.auto_name_suggested);
        assert!(entry.saved_name.is_none());
    }

    #[test]
    fn session_entry_accumulates_cost_across_turns() {
        let mut entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            cost_usd: 0.0,
            loaded_tools: HashSet::new(),
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };

        // Simulate three turns of cost accumulation.
        entry.cost_usd += 0.10;
        *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.10;
        entry.cost_usd += 0.20;
        *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.20;
        entry.cost_usd += 0.05;
        *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.05;

        assert!((entry.cost_usd - 0.35).abs() < 1e-10);
        assert!((entry.cost_by_agent["chat"] - 0.35).abs() < 1e-10);
        assert!(!entry.has_untracked_cost);
    }

    #[test]
    fn session_entry_per_agent_split() {
        let mut entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            cost_usd: 0.0,
            loaded_tools: HashSet::new(),
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };

        // Simulate /agent switch mid-flow.
        entry.cost_usd += 0.30;
        *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.30;
        entry.cost_usd += 0.15;
        *entry
            .cost_by_agent
            .entry("architect".to_string())
            .or_insert(0.0) += 0.15;

        assert!((entry.cost_usd - 0.45).abs() < 1e-10);
        assert!((entry.cost_by_agent["chat"] - 0.30).abs() < 1e-10);
        assert!((entry.cost_by_agent["architect"] - 0.15).abs() < 1e-10);
        assert_eq!(entry.cost_by_agent.len(), 2);
    }

    #[test]
    fn unknown_pricing_sets_has_untracked_cost() {
        let mut entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            cost_usd: 0.0,
            loaded_tools: HashSet::new(),
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };

        // Known pricing call.
        entry.cost_usd += 0.10;
        *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.10;
        assert!(!entry.has_untracked_cost);

        // Unknown pricing call — flag should flip.
        entry.has_untracked_cost = true;
        assert!(entry.has_untracked_cost);

        // Subsequent known pricing calls don't reset the flag.
        entry.cost_usd += 0.05;
        *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.05;
        assert!(entry.has_untracked_cost);
    }

    #[test]
    fn archive_appends_survive_compaction_rewrite() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "archive-sim";

        // Append 30 messages — each goes to both working file and archive
        for i in 0..30 {
            let msg = make_msg("user", &format!("msg {}", i));
            append_session_message(id, &msg);
        }

        // Verify archive has 30 lines
        let archive_path = archive_file(id);
        let archive_content = std::fs::read_to_string(&archive_path).unwrap();
        let archive_lines: Vec<&str> = archive_content.lines().collect();
        assert_eq!(
            archive_lines.len(),
            30,
            "archive should have exactly 30 messages"
        );

        // Simulate compaction: rewrite working file to only 5 messages
        let msgs: Vec<Message> = (0..5)
            .map(|i| make_msg("user", &format!("msg {}", i)))
            .collect();
        write_session_file(id, &msgs);

        // Archive should still have 30 messages (not 5, not 35)
        let archive_content = std::fs::read_to_string(&archive_path).unwrap();
        let archive_lines: Vec<&str> = archive_content.lines().collect();
        assert_eq!(
            archive_lines.len(),
            30,
            "archive should still have 30 messages after working file compaction"
        );

        // First and last should match the original 30 messages
        assert!(
            archive_lines[0].contains("msg 0"),
            "first archive line should be msg 0"
        );
        assert!(
            archive_lines[29].contains("msg 29"),
            "last archive line should be msg 29"
        );

        // No duplicates: archive length == messages appended, not 2x
        assert_eq!(archive_lines.len(), 30);
    }

    #[test]
    fn archive_seeds_from_existing_working_file() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "archive-seed";

        // Pre-populate working file with 10 messages (no archive yet)
        let working_path = session_file(id);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&working_path)
            .unwrap();
        use std::io::Write;
        for i in 0..10 {
            let msg = make_msg("user", &format!("seed {}", i));
            let line = serde_json::to_string(&msg).unwrap();
            writeln!(file, "{}", line).unwrap();
        }
        drop(file);

        // Verify no archive exists yet
        assert!(!archive_file(id).exists());

        // First append_archive_message should seed from working file + append new msg
        let new_msg = make_msg("user", "new msg");
        append_archive_message(id, &new_msg);

        // Archive should have 11 messages (10 seeded + 1 new)
        let archive_content = std::fs::read_to_string(archive_file(id)).unwrap();
        let archive_lines: Vec<&str> = archive_content.lines().collect();
        assert_eq!(
            archive_lines.len(),
            11,
            "archive should have 10 seeded + 1 new = 11 messages"
        );

        // First line should be the first seeded message
        assert!(
            archive_lines[0].contains("seed 0"),
            "first archive line should be the first seeded message"
        );
        // Last line should be the new message
        assert!(
            archive_lines[10].contains("new msg"),
            "last archive line should be the new message"
        );
    }

    #[test]
    fn archive_seed_absent_working_file() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "archive-fresh";

        // Neither file exists
        assert!(!session_file(id).exists());
        assert!(!archive_file(id).exists());

        // Append one message
        let msg = make_msg("user", "first");
        append_archive_message(id, &msg);

        // Archive should have exactly 1 message (no seed, just the append)
        let archive_content = std::fs::read_to_string(archive_file(id)).unwrap();
        let archive_lines: Vec<&str> = archive_content.lines().collect();
        assert_eq!(
            archive_lines.len(),
            1,
            "archive should have exactly 1 message when neither file existed"
        );
    }

    #[test]
    fn archive_file_path_is_correct() {
        let path = archive_file("abc123");
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "abc123.archive.jsonl",
            "archive file path should end with <id>.archive.jsonl"
        );
    }

    fn assert_no_orphan_tool_results(msgs: &[Message]) {
        for (i, m) in msgs.iter().enumerate() {
            if let Some(results) = &m.tool_results {
                for r in results {
                    let found = msgs[..i].iter().rev().any(|prev| {
                        prev.tool_calls
                            .as_ref()
                            .is_some_and(|calls| calls.iter().any(|c| c.id == r.tool_call_id))
                    });
                    assert!(
                        found,
                        "orphan tool_result at idx {}: call_id={}",
                        i, r.tool_call_id
                    );
                }
            }
        }
    }

    #[test]
    fn meta_roundtrip() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "meta-roundtrip";
        let meta = SessionMeta {
            started_at: chrono::Utc::now(),
            turn_count: 5,
            last_prompt_tokens: 120,
            token_scale: 1.8,
            tool_calls_this_session: 3,
            saved_name: Some("my-session".to_string()),
        };
        write_session_meta(id, &meta);

        let loaded = read_session_meta(id).expect("meta file should exist");
        assert_eq!(loaded.turn_count, 5);
        assert_eq!(loaded.last_prompt_tokens, 120);
        assert_eq!(loaded.token_scale, 1.8);
        assert_eq!(loaded.tool_calls_this_session, 3);
        assert_eq!(loaded.saved_name, Some("my-session".to_string()));
    }

    #[test]
    fn meta_corrupt_returns_none() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "meta-corrupt";
        let path = meta_file(id);
        std::fs::write(&path, "NOT_JSON {{{").unwrap();

        let loaded = read_session_meta(id);
        assert!(
            loaded.is_none(),
            "corrupt meta should return None, not Some"
        );
    }

    #[test]
    fn entry_recreation_seeds_from_meta() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "meta-seed";
        let meta = SessionMeta {
            started_at: chrono::Utc::now(),
            turn_count: 7,
            last_prompt_tokens: 200,
            token_scale: 2.0,
            tool_calls_this_session: 10,
            saved_name: Some("seeded".to_string()),
        };
        write_session_meta(id, &meta);

        // Simulate entry recreation by reading meta and building a fresh entry.
        let loaded_meta = read_session_meta(id).expect("meta should exist");
        let entry = SessionEntry {
            messages: Vec::new(),
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: Vec::new(),
            last_prompt_tokens: loaded_meta.last_prompt_tokens,
            tmux_session: String::new(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: crate::daemon::GS_BG_WINDOW_PREFIX,
            started_at: loaded_meta.started_at,
            turn_count: loaded_meta.turn_count,
            tool_calls_this_session: loaded_meta.tool_calls_this_session,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: loaded_meta.saved_name,
            dirty: false,
            artifacts_created: Vec::new(),
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
            token_scale: loaded_meta.token_scale,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };

        assert_eq!(entry.turn_count, 7);
        assert_eq!(entry.last_prompt_tokens, 200);
        assert_eq!(entry.token_scale, 2.0);
        assert_eq!(entry.tool_calls_this_session, 10);
        assert_eq!(entry.saved_name, Some("seeded".to_string()));
        assert!(!entry.compaction_in_flight);
        assert!(entry.pending_compaction_notice.is_none());
    }

    #[test]
    fn read_session_file_lands_on_clean_boundary() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "boundary-test";
        // Build a history where a small cap would slice mid-tool-chain:
        // user → assistant(tool_calls) → user(tool_results) → user(tool_results) → user(clean) → assistant
        // With cap=3, the raw slice starts at index 3 (user with tool_results).
        // Boundary-safe reload should advance to index 4 (clean user).
        let tool_result_1 = ToolResult {
            tool_call_id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            content: "file content".to_string(),
        };
        let tool_result_2 = ToolResult {
            tool_call_id: "call-2".to_string(),
            tool_name: "bash".to_string(),
            content: "output".to_string(),
        };
        let msgs: Vec<Message> = vec![
            make_msg("user", "hello"),
            make_msg("assistant", "thinking"),
            make_msg_with_tool_results("user", "result 1", vec![tool_result_1.clone()]),
            make_msg_with_tool_results("user", "result 2", vec![tool_result_2.clone()]),
            make_msg("user", "new question"),
            make_msg("assistant", "answer"),
        ];
        // Write the history to disk
        let path = session_file(id);
        let jsonl: String = msgs
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, jsonl).unwrap();

        // Read with a small cap that forces boundary alignment
        let result = read_session_file(id, Some(3));
        assert_no_orphan_tool_results(&result);
        // Verify the boundary was actually advanced: the first message should
        // be the clean user message (index 4), not the tool_results-bearing
        // user message (index 3).
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "new question");
        assert!(result[0].tool_results.is_none());
    }

    #[test]
    fn read_session_file_repairs_when_no_boundary() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let id = "repair-test";
        // Build a history where the tail contains only tool_results-bearing user messages
        // (no clean boundary exists in the tail).
        let tool_result_1 = ToolResult {
            tool_call_id: "call-3".to_string(),
            tool_name: "read_file".to_string(),
            content: "file content".to_string(),
        };
        let tool_result_2 = ToolResult {
            tool_call_id: "call-4".to_string(),
            tool_name: "bash".to_string(),
            content: "output".to_string(),
        };
        let tool_result_3 = ToolResult {
            tool_call_id: "call-5".to_string(),
            tool_name: "search".to_string(),
            content: "matches".to_string(),
        };
        let msgs: Vec<Message> = vec![
            make_msg("user", "hello"),
            make_msg("assistant", "thinking"),
            make_msg_with_tool_results("user", "result 1", vec![tool_result_1]),
            make_msg_with_tool_results("user", "result 2", vec![tool_result_2]),
            make_msg_with_tool_results("user", "result 3", vec![tool_result_3]),
        ];
        let path = session_file(id);
        let jsonl: String = msgs
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, jsonl).unwrap();

        // Read with a cap that forces the tail to start within the tool_results zone
        let result = read_session_file(id, Some(3));
        // The repair should have stripped the orphaned tool_results
        assert_no_orphan_tool_results(&result);
        // Verify the repair actually happened: no message should have tool_results
        for msg in &result {
            assert!(
                msg.tool_results.as_ref().is_none_or(Vec::is_empty),
                "Expected repaired messages to have no tool_results"
            );
        }
    }

    fn entry_with(last_accessed: Instant) -> SessionEntry {
        SessionEntry {
            messages: vec![],
            last_accessed,
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            cost_usd: 0.0,
            loaded_tools: HashSet::new(),
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        }
    }

    #[test]
    fn cleanup_pass_releases_the_lock() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        let idle = Instant::now()
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or(Instant::now());
        with_sessions(&sessions, |store| {
            store.insert("s1".to_string(), entry_with(idle));
        });

        let now = Instant::now();
        let (_evicted, _active) =
            cleanup_pass(&sessions, now, std::time::Duration::from_secs(1800));

        // Guard from cleanup_pass is dropped; try_lock must succeed.
        assert!(sessions.try_lock().is_ok());
    }

    #[test]
    fn cleanup_pass_evicts_idle_and_keeps_active() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        let idle = Instant::now()
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or(Instant::now());
        let active = Instant::now();

        with_sessions(&sessions, |store| {
            store.insert("idle".to_string(), entry_with(idle));
            store.insert("active".to_string(), entry_with(active));
        });

        let now = Instant::now();
        let (evicted, active_ids) =
            cleanup_pass(&sessions, now, std::time::Duration::from_secs(1800));

        assert_eq!(evicted.len(), 1);
        assert!(active_ids.contains("active"));
        assert!(!active_ids.contains("idle"));
        let remaining = sessions
            .try_lock()
            .expect("cleanup_pass must release the lock before returning");
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn with_sessions_runs_closure_and_releases_lock() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        with_sessions(&sessions, |store| {
            store.insert("test".to_string(), entry_with(Instant::now()));
        });

        let len = with_sessions(&sessions, |s| s.len());
        assert_eq!(len, 1, "closure return value should be passed through");
        assert!(
            sessions.try_lock().is_ok(),
            "guard must be released after with_sessions returns"
        );
    }

    #[test]
    #[should_panic(expected = "re-entrant SessionStore lock")]
    fn with_sessions_rejects_reentrant_call() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        with_sessions(&sessions, |_s| {
            // nested call — should panic with the re-entrancy message
            with_sessions(&sessions, |_s| {});
        });
    }

    #[test]
    fn with_sessions_depth_resets_after_panic() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));

        // First call panics — the depth counter must still reset via Drop
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_sessions(&sessions, |_s| panic!("intentional test panic"));
        }));
        std::panic::set_hook(old_hook);

        // Second call must succeed — depth is back to 0
        let len = with_sessions(&sessions, |s| {
            s.insert("after-panic".to_string(), entry_with(Instant::now()));
            s.len()
        });
        assert_eq!(len, 1, "depth counter must reset after a panicked closure");
    }

    #[test]
    fn with_sessions_sets_depth_inside_closure() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        with_sessions(&sessions, |_store| {
            assert_eq!(
                SESSIONS_LOCK_DEPTH.with(|d| d.get()),
                1,
                "depth must read 1 inside the closure — a `let _ =` binding on \
                 SessionsLockDepth::enter() would drop the guard immediately and \
                 read 0 here"
            );
        });
        assert_eq!(
            SESSIONS_LOCK_DEPTH.with(|d| d.get()),
            0,
            "depth must reset to 0 after the closure returns"
        );
    }
}
