
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::ai::Message;

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
    /// Panes currently flagged for passive alert-activity monitoring.
    pub watched_panes: std::collections::HashSet<String>,
}

/// Thread-safe, shared session store passed to every client handler.
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;

pub const FALLBACK_SESSION: &str = "daemoneye";


/// Maximum number of messages retained per session (in memory and on disk).
pub const MAX_HISTORY: usize = 40;

pub static BG_DONE_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<String>> =
    std::sync::OnceLock::new();

pub fn bg_done_subscribe() -> tokio::sync::broadcast::Receiver<String> {
    BG_DONE_TX
        .get_or_init(|| { let (tx, _) = tokio::sync::broadcast::channel(32); tx })
        .subscribe()
}


/// Path to the JSONL file storing a session's message history.
pub fn session_file(id: &str) -> std::path::PathBuf {
    crate::config::sessions_dir().join(format!("{}.jsonl", id))
}

/// Write the current (already-trimmed) message history to disk, overwriting
/// the previous snapshot.  Failures are non-fatal — we just skip persistence.
pub fn write_session_file(id: &str, messages: &[Message]) {
    use std::io::Write;
    let path = session_file(id);
    if let Ok(mut f) = std::fs::File::create(&path) {
        for msg in messages {
            if let Ok(line) = serde_json::to_string(msg) {
                let _ = writeln!(f, "{}", line);
            }
        }
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
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
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
        Message { role: role.to_string(), content: content.to_string(), tool_calls: None, tool_results: None }
    }

    fn make_history(n: usize) -> Vec<Message> {
        (0..n).map(|i| make_msg(if i % 2 == 0 { "user" } else { "assistant" }, &format!("msg {i}"))).collect()
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
    fn session_file_roundtrip() {
        // Write messages to a temp session file and read them back.
        let id = format!("test_{}", std::process::id());
        // Temporarily point sessions_dir() at /tmp to avoid HOME dependency.
        // We call the helpers directly using /tmp as the base.
        let dir = std::path::PathBuf::from("/tmp");
        let path = dir.join(format!("{}.jsonl", id));

        let msgs = vec![
            make_msg("user", "hello"),
            make_msg("assistant", "hi there"),
        ];

        // Replicate write_session_file logic with a known path.
        use std::io::Write;
        let mut f = std::fs::File::create(&path).unwrap();
        for m in &msgs {
            writeln!(f, "{}", serde_json::to_string(m).unwrap()).unwrap();
        }

        // Replicate read_session_file logic with the same path.
        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Vec<Message> = text.lines()
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
}
