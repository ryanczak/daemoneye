//! Live chat-presence connections per session id (`PresenceRegistry`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use super::{SessionStore, with_sessions};
use crate::ipc::Response;
use crate::util::UnpoisonExt;

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

#[cfg(test)]
impl PresenceConnection {
    pub fn test_conn() -> Self {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        PresenceConnection { push: tx }
    }
}

/// Shared registry of live chat-presence connections per session id.
///
/// One session may have several connections; detach state is stamped only
/// when the last one closes. Counted by connection rather than on
/// `SessionEntry`, because a fresh chat attaches before its first `Ask`
/// creates the entry.
#[derive(Clone, Default)]
pub struct PresenceRegistry(Arc<Mutex<HashMap<String, Vec<PresenceConnection>>>>);

impl PresenceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn inner(&self) -> &Mutex<HashMap<String, Vec<PresenceConnection>>> {
        &self.0
    }

    /// Register one live presence connection for `session_id`, returning the new
    /// count for that session.
    pub fn register(&self, session_id: &str, conn: PresenceConnection) -> usize {
        let mut map = self.0.lock().unwrap_or_log();
        let conns = map.entry(session_id.to_string()).or_default();
        conns.push(conn);
        conns.len()
    }

    /// Unregister one presence connection for `session_id` — the one whose push
    /// sender names. Returns `true` when the count dropped to zero (this was the
    /// session's last connection).
    pub fn unregister(&self, session_id: &str, conn: &PresenceConnection) -> bool {
        let mut map = self.0.lock().unwrap_or_log();
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
    pub fn count(&self, session_id: &str) -> usize {
        self.0
            .lock()
            .unwrap_or_log()
            .get(session_id)
            .map(Vec::len)
            .unwrap_or(0)
    }

    /// The session ids that have at least one live presence connection.
    pub fn session_ids(&self) -> Vec<String> {
        self.0.lock().unwrap_or_log().keys().cloned().collect()
    }

    /// Total live presence connections across all sessions.
    pub fn total_connections(&self) -> usize {
        self.0.lock().unwrap_or_log().values().map(Vec::len).sum()
    }

    /// Push one `Response` frame to every live presence connection.
    pub fn broadcast_frame(&self, frame: &Response) -> usize {
        presence_broadcast_frame_in(&self.0, frame)
    }

    /// Push `msg` as a `SystemMsg` frame to every live presence connection.
    pub fn broadcast(&self, msg: &str) -> usize {
        presence_broadcast_in(&self.0, msg)
    }
}

/// Process-global chat-presence registry.
pub static CHAT_PRESENCE: OnceLock<PresenceRegistry> = OnceLock::new();

/// Access the process-global [`PresenceRegistry`].
pub fn global_presence() -> &'static PresenceRegistry {
    CHAT_PRESENCE.get_or_init(PresenceRegistry::new)
}

/// Register one live presence connection for `session_id`, returning the new
/// count for that session.
pub fn presence_register(session_id: &str, conn: PresenceConnection) -> usize {
    global_presence().register(session_id, conn)
}

/// Unregister one presence connection for `session_id`.
pub fn presence_unregister(session_id: &str, conn: &PresenceConnection) -> bool {
    global_presence().unregister(session_id, conn)
}

/// Live presence-connection count for the session.
pub fn presence_count(session_id: &str) -> usize {
    global_presence().count(session_id)
}

/// The session ids that have at least one live presence connection.
pub fn presence_session_ids() -> Vec<String> {
    global_presence().session_ids()
}

/// True when the session has at least one live chat-presence connection.
pub fn presence_live(session_id: &str) -> bool {
    presence_count(session_id) > 0
}

/// Push `msg` to every connection in an explicit presence registry as a
/// `SystemMsg` frame — the seam tests drive with a fresh map.
pub fn presence_broadcast_in(
    map: &Mutex<HashMap<String, Vec<PresenceConnection>>>,
    msg: &str,
) -> usize {
    presence_broadcast_frame_in(map, &Response::SystemMsg(msg.to_string()))
}

/// Push `msg` to every live presence connection, across all sessions.
pub fn presence_broadcast(msg: &str) -> usize {
    global_presence().broadcast(msg)
}

/// Push one `Response` frame to every live presence connection in `map`.
pub fn presence_broadcast_frame_in(
    map: &Mutex<HashMap<String, Vec<PresenceConnection>>>,
    frame: &Response,
) -> usize {
    let map = map.lock().unwrap_or_log();
    let mut pushed = 0;
    let Ok(line) = serde_json::to_string(frame) else {
        return 0;
    };
    for conns in map.values() {
        for conn in conns {
            if conn.push.send(line.clone()).is_ok() {
                pushed += 1;
            } else {
                log::debug!("chat presence push dropped: connection closed");
            }
        }
    }
    pushed
}

/// Push one `Response` frame to every live presence connection.
pub fn presence_broadcast_frame(frame: &Response) -> usize {
    global_presence().broadcast_frame(frame)
}

/// Stamp detach state on the session's entry, when it exists. Called by the
/// attach handler after `presence_unregister` reports the last close.
pub fn record_chat_detach(session_id: &str, sessions: &SessionStore) {
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            let msg_len = entry.history.messages.len();
            if let Some(i) = entry.as_interactive_mut() {
                i.last_detach = Some(now);
                i.detach_time_utc = Some(now_utc);
                i.messages_at_detach = msg_len;
            }
        }
    })
}
