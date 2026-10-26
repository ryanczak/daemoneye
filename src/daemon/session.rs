use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::ai::Message;
use crate::util::UnpoisonExt;

/// Metadata for a background tmux window spawned during a chat session.
pub struct BgWindowInfo {
    /// tmux pane ID (e.g. `%7`) — the background window's foreground target.
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
    /// True if this session is autonomous (no attached human user).
    pub is_ghost: bool,
    /// Settings for autonomous execution (inherited from the triggering runbook).
    pub ghost_config: Option<crate::ipc::GhostConfig>,
    /// Name of the runbook this ghost shell follows. `None` for chat
    /// sessions. Names the run in `RunKind::Ghost { runbook, .. }`.
    pub ghost_runbook_name: Option<String>,
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
    /// a `get_shell_context` tool call.
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

/// Shared registry of live sessions.
///
/// A newtype rather than an alias so the inner mutex cannot be locked from
/// outside this module: `with_sessions` is the only way in, which is what keeps
/// blocking work out of the critical section. See
/// `docs/design/daemon-stalls.md` § 1 mechanism A.
#[derive(Clone, Default)]
pub struct SessionStore(Arc<Mutex<HashMap<String, SessionEntry>>>);

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only escape hatch for asserting the guard was released.
    ///
    /// Deliberately `#[cfg(test)]`: production code must go through
    /// `with_sessions`, and a non-blocking peek is only ever needed to prove a
    /// lock is *not* held.
    #[cfg(test)]
    pub fn try_lock(
        &self,
    ) -> std::sync::TryLockResult<std::sync::MutexGuard<'_, HashMap<String, SessionEntry>>> {
        self.0.try_lock()
    }
}

/// Live chat-presence connections per session id (M27 phase 03).
///
/// One session may have several connections; detach state is stamped only
/// when the last one closes. Counted by connection rather than on
/// `SessionEntry`, because a fresh chat attaches before its first `Ask`
/// creates the entry.
pub static CHAT_PRESENCE: std::sync::OnceLock<Mutex<HashMap<String, Vec<PresenceConnection>>>> =
    std::sync::OnceLock::new();

/// One live presence connection's push channel (M27 phase 03b).
///
/// The attach handler clones the sender into the registry and keeps the
/// receiver, so the broadcast path queues and never awaits the handler: a
/// full or closed per-connection queue drops that push for that client while
/// the others still get the message.
#[derive(Clone)]
pub struct PresenceConnection {
    pub push: tokio::sync::mpsc::UnboundedSender<String>,
}

fn presence_map() -> &'static Mutex<HashMap<String, Vec<PresenceConnection>>> {
    CHAT_PRESENCE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register one live presence connection for `session_id`, returning the new
/// count for that session.
pub fn presence_register(session_id: &str, conn: PresenceConnection) -> usize {
    let mut map = presence_map().lock().unwrap_or_log();
    let conns = map.entry(session_id.to_string()).or_default();
    conns.push(conn);
    conns.len()
}

/// Unregister one presence connection for `session_id` — the one whose push
/// sender names. Returns `true` when the count dropped to zero (this was the
/// session's last connection). A close with no recorded presence simply
/// reports "not last".
pub fn presence_unregister(session_id: &str, conn: &PresenceConnection) -> bool {
    let mut map = presence_map().lock().unwrap_or_log();
    match map.get_mut(session_id) {
        Some(conns) => {
            if let Some(pos) = conns.iter().position(|c| c.push.same_channel(&conn.push)) {
                conns.swap_remove(pos);
            }
            if conns.is_empty() {
                map.remove(session_id);
                true
            } else {
                false
            }
        }
        None => false,
    }
}

/// Live presence-connection count for the session.
pub fn presence_count(session_id: &str) -> usize {
    presence_map()
        .lock()
        .unwrap_or_log()
        .get(session_id)
        .map(Vec::len)
        .unwrap_or(0)
}

/// The session ids that have at least one live presence connection.
///
/// A chat that re-attaches after a daemon restart is present here before its
/// first `Ask` creates its in-memory `SessionEntry`, so injection must reach
/// it even though the store does not know it yet.
pub fn presence_session_ids() -> Vec<String> {
    presence_map()
        .lock()
        .unwrap_or_log()
        .keys()
        .cloned()
        .collect()
}

/// True when the session has at least one live chat-presence connection.
pub fn presence_live(session_id: &str) -> bool {
    presence_count(session_id) > 0
}

/// Push `msg` to every connection in an explicit presence registry — the seam
/// tests drive with a fresh map, since `CHAT_PRESENCE` is process-global and
/// cargo runs tests in parallel. The global [`presence_broadcast`] calls this
/// with [`presence_map`].
///
/// Called from the webhook handler and the scheduler, so it must not await a
/// slow client: each push is queued on a per-connection unbounded channel and
/// never awaited. A closed queue drops that push for that client (logged at
/// `debug`) while the others still get the message. Delivering to nobody is
/// the normal no-chat-attached case, not an error. Returns how many live
/// connections the push reached.
pub fn presence_broadcast_in(
    map: &Mutex<HashMap<String, Vec<PresenceConnection>>>,
    msg: &str,
) -> usize {
    let map = map.lock().unwrap_or_log();
    let mut pushed = 0;
    for conns in map.values() {
        for conn in conns {
            if conn.push.send(msg.to_string()).is_ok() {
                pushed += 1;
            } else {
                log::debug!("chat presence push dropped: connection closed");
            }
        }
    }
    pushed
}

/// Push `msg` to every live presence connection, across all sessions.
pub fn presence_broadcast(msg: &str) -> usize {
    presence_broadcast_in(presence_map(), msg)
}

#[cfg(test)]
impl PresenceConnection {
    pub fn test_conn() -> Self {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        PresenceConnection { push: tx }
    }
}

/// Stamp detach state on the session's entry, when it exists. Called by the
/// attach handler after `presence_unregister` reports the last close.
pub fn record_chat_detach(session_id: &str, sessions: &SessionStore) {
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.last_detach = Some(now);
            entry.detach_time_utc = Some(now_utc);
            entry.messages_at_detach = entry.messages.len();
        }
    })
}

/// Broadcast channel for background command completion via IPC.
/// Carries `(pane_id, exit_code)` delivered directly by the command wrapper.
pub static COMPLETE_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<(String, i32)>> =
    std::sync::OnceLock::new();

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

    // Ensure the sessions directory exists.
    if let Some(parent) = archive_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Seed: if the archive doesn't exist but the working file does, copy it.
    if !archive_path.exists()
        && working_path.exists()
        && std::fs::copy(&working_path, &archive_path).is_ok()
    {
        // Index the entire seeded file so copied lines are searchable.
        if let Ok(conn) = crate::memory::index::open_index() {
            if let Err(e) = crate::memory::index::index_archive_file(&conn, id, &archive_path) {
                log::warn!("indexing seeded archive for session {}: {e:#}", id);
            }
        } else {
            log::warn!("index open failed for seed, skipping");
        }
    }

    // Compute the byte offset of the line about to be appended.
    // Must be computed after any seed (which writes the file) and before the append.
    let offset = std::fs::metadata(&archive_path)
        .ok()
        .map(|m| m.len())
        .or(Some(0));

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
        Ok(()) => {
            // Index the appended turn if it has a turn number.
            if let Some(turn) = msg.turn {
                let mut body = msg.content.clone();
                if let Some(ref tool_results) = msg.tool_results {
                    for tr in tool_results {
                        body.push(' ');
                        body.push_str(&tr.content);
                    }
                }
                if let Some(off) = offset
                    && let Err(e) = crate::memory::index::index_turn(id, turn, off, &body)
                {
                    log::warn!(
                        "memory index update failed for turn {} in session {}: {e:#}",
                        turn,
                        id
                    );
                }
            }
        }
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

    /// Build the resident entry for `id` from its disk state: the session
    /// file's history plus the persisted meta's continuity fields. A missing
    /// file or meta gives an empty history and the same defaults a fresh
    /// session starts with. This is the one constructor for a resident entry —
    /// `handle_ask`'s upsert and the presence attach both go through it, so
    /// the two creation sites cannot drift (M29 phase 05).
    pub fn from_disk(id: &str) -> SessionEntry {
        let meta = read_session_meta(id);
        let (
            started_at,
            turn_count,
            last_prompt_tokens,
            tool_calls_this_session,
            token_scale,
            saved_name,
        ) = match &meta {
            Some(m) => (
                m.started_at,
                m.turn_count,
                m.last_prompt_tokens,
                m.tool_calls_this_session,
                m.token_scale,
                m.saved_name.clone(),
            ),
            None => (chrono::Utc::now(), 0, 0, 0, 1.5, None),
        };
        SessionEntry {
            messages: read_session_file(id, None),
            last_accessed: Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: Vec::new(),
            last_prompt_tokens,
            tmux_session: String::new(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            is_ghost: false,
            ghost_config: None,
            ghost_runbook_name: None,
            ghost_bg_prefix: crate::daemon::GS_BG_WINDOW_PREFIX,
            started_at,
            turn_count,
            tool_calls_this_session,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name,
            dirty: false,
            artifacts_created: Vec::new(),
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale,
            compaction_in_flight: false,
            pending_compaction_notice: None,
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
    let mut store = sessions.0.lock().unwrap_or_log();
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
            .filter(|(k, v)| {
                now.duration_since(v.last_accessed()) >= idle_after && !presence_live(k)
            })
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
#[path = "session_tests.rs"]
mod tests;
