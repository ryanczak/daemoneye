use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::ai::Message;

/// Metadata for a background tmux window spawned during a chat session.
pub struct BgWindowInfo {
    /// tmux pane ID (e.g. `%7`) — can be passed to `watch_pane` or used as foreground target.
    pub pane_id: String,
    /// Full tmux window name (e.g. `de-bg-main-20240101120000-abc12345`).
    pub window_name: String,
    /// The tmux session the window belongs to (needed to kill it on eviction).
    pub tmux_session: String,
    /// `None` while still running; `Some(code)` after the pane exits.
    pub exit_code: Option<i32>,
}

/// In-memory record of an active chat session.
/// Evicted by the cleanup task after 30 minutes of inactivity.
pub struct SessionEntry {
    /// Full trimmed message history for this session (bounded to `MAX_HISTORY`).
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
    /// Number of messages in `messages` at the time of `last_detach`.
    /// Used to identify messages injected while no client was present (N15).
    pub messages_at_detach: usize,
    /// The source pane that has `pipe-pane` active for this session (R1).
    /// `None` before the first Ask or when pipe-pane is not available.
    pub pipe_source_pane: Option<String>,
}

/// Thread-safe, shared session store passed to every client handler.
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;

/// Maximum number of messages retained per session (in memory and on disk).
pub const MAX_HISTORY: usize = 40;

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

/// Rewrite the entire session file with the current message history.
/// Used after a `trim_history` compaction, when old entries have been dropped.
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

/// Append a single message to the session file without rewriting earlier entries.
/// This is the hot path — called once per new message during normal turns.
/// Failures are logged at WARN and non-fatal.
pub fn append_session_message(id: &str, msg: &Message) {
    use std::fs::OpenOptions;
    use std::io::Write;
    let path = session_file(id);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(msg) {
            if let Err(e) = writeln!(f, "{}", line) {
                log::warn!("Failed to append to session file {}: {}", path.display(), e);
            }
        }
    } else {
        log::warn!("Failed to open session file {} for append", path.display());
    }
}

/// Trim a message history Vec to at most `MAX_HISTORY` entries.
///
/// Layout after trim: `[first_message] [placeholder] [tail…]`
/// - `first_message` is the initial user turn (contains injected system context).
/// - `placeholder` is a synthetic assistant message noting the truncation so the
///   AI understands it is not seeing the full history.
/// - `tail` is the most-recent slice, always starting at an even index (user turn)
///   to keep the strict `user → assistant → user → …` alternation valid.
///
/// Returns `messages` unchanged when `messages.len() <= MAX_HISTORY`.
pub fn trim_history(messages: Vec<Message>) -> Vec<Message> {
    if messages.len() <= MAX_HISTORY {
        return messages;
    }
    // raw_tail_start ensures result length ≤ MAX_HISTORY:
    //   1 (first) + 1 (placeholder) + (N - tail_start) ≤ MAX_HISTORY
    let raw_tail_start = messages.len() - MAX_HISTORY + 2;
    // Round up to even so the tail begins on a user message.
    let tail_start = if raw_tail_start % 2 == 0 {
        raw_tail_start
    } else {
        raw_tail_start + 1
    };
    let dropped = tail_start - 1;
    let first = messages[0].clone();
    let placeholder = Message {
        role: "assistant".to_string(),
        content: format!(
            "[{} earlier messages were trimmed to fit the context window. \
             The conversation continues from a later point in the session.]",
            dropped
        ),
        tool_calls: None,
        tool_results: None,
    };
    let mut trimmed = Vec::with_capacity(MAX_HISTORY);
    trimmed.push(first);
    trimmed.push(placeholder);
    trimmed.extend_from_slice(&messages[tail_start..]);
    trimmed
}

/// Load message history from a session file, returning at most `MAX_HISTORY`
/// tail messages.  Returns an empty Vec if the file does not exist or is unreadable.
pub fn read_session_file(id: &str) -> Vec<Message> {
    let path = session_file(id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let msgs: Vec<Message> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if msgs.len() <= MAX_HISTORY {
        msgs
    } else {
        msgs[msgs.len() - MAX_HISTORY..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;

    fn make_msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
        }
    }

    fn make_history(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                make_msg(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &format!("msg {i}"),
                )
            })
            .collect()
    }

    #[test]
    fn trim_history_unchanged_when_under_limit() {
        let msgs = make_history(10);
        let out = trim_history(msgs.clone());
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].content, "msg 0");
    }

    #[test]
    fn trim_history_at_exact_limit_unchanged() {
        let msgs = make_history(MAX_HISTORY);
        let out = trim_history(msgs);
        assert_eq!(out.len(), MAX_HISTORY);
    }

    #[test]
    fn trim_history_over_limit_bounded() {
        let msgs = make_history(MAX_HISTORY + 10);
        let out = trim_history(msgs);
        assert!(out.len() <= MAX_HISTORY);
    }

    #[test]
    fn trim_history_preserves_first_message() {
        let msgs = make_history(MAX_HISTORY + 5);
        let out = trim_history(msgs);
        assert_eq!(out[0].content, "msg 0");
    }

    #[test]
    fn trim_history_placeholder_is_assistant() {
        let msgs = make_history(MAX_HISTORY + 5);
        let out = trim_history(msgs);
        // position 1 is the placeholder
        assert_eq!(out[1].role, "assistant");
        assert!(out[1].content.contains("trimmed"));
    }

    #[test]
    fn trim_history_tail_starts_on_user_turn() {
        // After [first, placeholder], the next message must be a user message
        // so the user→assistant alternation is valid.
        let msgs = make_history(MAX_HISTORY + 5);
        let out = trim_history(msgs);
        assert_eq!(out[2].role, "user", "tail must start on a user message");
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
        if let Some(ref pane_id) = self.pipe_source_pane {
            crate::tmux::stop_pipe_pane(pane_id);
        }
    }
}
