//! Session counters (M37 phase 02b): what the status bar's background, ghost
//! and shell slots show, pushed to a session's presence connections whenever
//! the values change and once at attach.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::ipc::Response;
use crate::shell::{Owner, ShellRegistry, with_shells};
use crate::util::UnpoisonExt;

/// The counters one chat session's bar shows. `background` is the session's
/// running `wait: false` commands, `ghosts` the daemon-wide active ghost
/// runs, `shells` the session's open PTY shells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionCounters {
    pub background: u32,
    pub ghosts: u32,
    pub shells: u32,
}

/// Compute a session's counters over one shell-registry snapshot.
///
/// `shells` counts shells owned by `Owner::Chat(session_id)` that have not
/// exited; `background` counts, of those, the `dedicated` shells with a
/// command in flight — the session's running `wait: false` commands.
/// `ghosts` is `stats::get_ghosts_active()`, daemon-wide, because ghost runs
/// belong to no chat session. Another session's shells, ghost-owned shells
/// and scheduler shells count toward nobody's `shells` or `background`.
pub fn counters_for_session(
    reg: &ShellRegistry,
    session_id: &str,
    ghosts_active: u32,
) -> SessionCounters {
    let (shells, background) = with_shells(reg, |store| {
        let mut shells = 0u32;
        let mut background = 0u32;
        for record in store.values() {
            if record.owner != Owner::Chat(session_id.to_string()) {
                continue;
            }
            if record.exited_unix.is_some() {
                continue;
            }
            shells += 1;
            if record.dedicated && record.in_flight.is_some() {
                background += 1;
            }
        }
        (shells, background)
    });
    SessionCounters {
        background,
        ghosts: ghosts_active,
        shells,
    }
}

/// Per-session last-pushed counter values, so the 1 s task pushes only on
/// change and an attach's first value starts the session's record fresh.
#[derive(Default)]
pub struct CountersTracker {
    last: HashMap<String, SessionCounters>,
}

impl CountersTracker {
    /// Whether `session_id` should see `value` pushed: the first value for a
    /// session, or one that differs from its last-pushed value.
    pub fn wants_push(&mut self, session_id: &str, value: SessionCounters) -> bool {
        match self.last.get(session_id) {
            Some(prev) => *prev != value,
            None => true,
        }
    }

    /// Record `value` as the session's last-pushed value.
    pub fn record_push(&mut self, session_id: &str, value: SessionCounters) {
        self.last.insert(session_id.to_string(), value);
    }

    /// Forget a session's last-pushed value: called when its last presence
    /// connection closes, so a re-attach starts fresh.
    pub fn forget(&mut self, session_id: &str) {
        self.last.remove(session_id);
    }
}

/// Push `frame` to every live presence connection of `session_id` only —
/// the per-session seam the 1 s task and the attach send drive, the way
/// [`crate::daemon::session::presence_broadcast_frame_in`] broadcasts. The
/// global map is `crate::daemon::session::presence_map()`; tests drive
/// explicit maps, since `CHAT_PRESENCE` is process-global and cargo runs
/// tests in parallel.
pub fn presence_push_to_session_in(
    map: &Mutex<HashMap<String, Vec<crate::daemon::session::PresenceConnection>>>,
    session_id: &str,
    frame: &Response,
) -> usize {
    let map = map.lock().unwrap_or_log();
    let mut pushed = 0;
    let Ok(line) = serde_json::to_string(frame) else {
        return 0;
    };
    if let Some(conns) = map.get(session_id) {
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

/// The daemon's last-pushed counters per session. Process-global like the
/// presence map itself; tests drive a fresh [`CountersTracker`], since cargo
/// runs tests in parallel.
static COUNTERS_TRACKER: std::sync::OnceLock<Mutex<CountersTracker>> = std::sync::OnceLock::new();

fn counters_tracker() -> &'static Mutex<CountersTracker> {
    COUNTERS_TRACKER.get_or_init(|| Mutex::new(CountersTracker::default()))
}

/// Push `value` to `session_id`'s presence connections and record it as the
/// session's last-pushed counters. The attach send uses this; the value it
/// records is the one the 1 s task diffs against.
pub fn record_session_counters(
    session_id: &str,
    value: SessionCounters,
    push: impl FnOnce(&Response),
) {
    {
        let mut tracker = counters_tracker().lock().unwrap_or_log();
        if !tracker.wants_push(session_id, value) {
            return;
        }
        tracker.record_push(session_id, value);
    }
    push(&Response::SessionCounters {
        background: value.background,
        ghosts: value.ghosts,
        shells: value.shells,
    });
}

/// Forget a session's last-pushed counters. Called by the attach handler when
/// `presence_unregister` reports the session's last connection closed, so a
/// re-attach starts fresh.
pub fn forget_session_counters(session_id: &str) {
    counters_tracker().lock().unwrap_or_log().forget(session_id);
}

/// One pass of the 1 s counters task: for each session with a live presence
/// connection, compute its counters and push a `SessionCounters` frame to
/// that session's connections only, and only when the value differs from the
/// last one pushed to it. The tracker records every value pushed here, so an
/// attach's first push starts from this task's last known value.
pub fn counters_tick(
    reg: &ShellRegistry,
    ghosts_active: u32,
    session_ids: &[String],
    tracker: &mut CountersTracker,
    mut push: impl FnMut(&str, &Response),
) {
    // Compute under one registry lock pass; the pushes run after, so no
    // registry lock is held across a client send.
    let values: Vec<(String, SessionCounters)> = session_ids
        .iter()
        .map(|sid| (sid.clone(), counters_for_session(reg, sid, ghosts_active)))
        .collect();
    for (sid, value) in values {
        if tracker.wants_push(&sid, value) {
            tracker.record_push(&sid, value);
            let frame = Response::SessionCounters {
                background: value.background,
                ghosts: value.ghosts,
                shells: value.shells,
            };
            push(&sid, &frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session::PresenceConnection;
    use crate::shell::{ShellId, ShellRecord, set_in_flight};
    use std::sync::Arc;

    fn record(index: u32, owner: Owner, dedicated: bool, exited: bool) -> ShellRecord {
        ShellRecord {
            id: ShellId::from_index(index),
            owner,
            host: crate::host::HostId::Local,
            sandbox: None,
            started_unix: 1,
            dedicated,
            exited_unix: if exited { Some(2) } else { None },
            last_foreground: None,
            in_flight: None,
            last_programs: Vec::new(),
        }
    }

    fn registry_with(records: Vec<ShellRecord>) -> ShellRegistry {
        let reg = ShellRegistry::default();
        with_shells(&reg, |store| {
            for r in records {
                store.insert(r.id.as_str().to_string(), r);
            }
        });
        reg
    }

    #[test]
    fn session_counters_count_this_sessions_live_shells_only() {
        let reg = registry_with(vec![
            record(1, Owner::Chat("sess".to_string()), false, false),
            record(2, Owner::Chat("sess".to_string()), false, false),
            // Another session's shell.
            record(3, Owner::Chat("other-sess".to_string()), false, false),
            // A ghost's shell.
            record(4, Owner::Ghost("g1".to_string()), false, false),
            // This session's shell, exited.
            record(5, Owner::Chat("sess".to_string()), false, true),
        ]);
        let c = counters_for_session(&reg, "sess", 3);
        assert_eq!(c.shells, 2, "only this session's live shells count");
        assert_eq!(c.ghosts, 3, "ghosts is the daemon-wide value");
        assert_eq!(c.background, 0);
    }

    #[test]
    fn session_counters_count_dedicated_shells_with_a_command_in_flight_as_background() {
        let reg = registry_with(vec![
            record(30, Owner::Chat("sess".to_string()), true, false),
            // Dedicated but idle: not background.
            record(31, Owner::Chat("sess".to_string()), true, false),
            // Busy but not dedicated: a foreground command, not background.
            record(32, Owner::Chat("sess".to_string()), false, false),
        ]);
        set_in_flight(
            &reg,
            &ShellId::from_index(30),
            "sleep 100",
            crate::shell::now_unix(),
        );
        set_in_flight(
            &reg,
            &ShellId::from_index(32),
            "sleep 100",
            crate::shell::now_unix(),
        );
        let c = counters_for_session(&reg, "sess", 0);
        assert_eq!(c.background, 1, "only the dedicated in-flight shell counts");
        assert_eq!(c.shells, 3, "all three live shells count");
    }

    #[test]
    fn counters_tracker_pushes_a_value_once_and_again_on_change() {
        let mut tracker = CountersTracker::default();
        let first = SessionCounters {
            background: 0,
            ghosts: 1,
            shells: 2,
        };
        assert!(tracker.wants_push("s", first), "the first value pushes");
        tracker.record_push("s", first);
        assert!(
            !tracker.wants_push("s", first),
            "the same value does not push again"
        );
        let changed = SessionCounters {
            background: 1,
            ghosts: 1,
            shells: 2,
        };
        assert!(tracker.wants_push("s", changed), "a new value pushes");
        tracker.record_push("s", changed);
        tracker.forget("s");
        assert!(
            tracker.wants_push("s", changed),
            "after forget, the value pushes again"
        );
    }

    #[test]
    fn presence_push_to_session_reaches_only_that_sessions_connections() {
        let map: Arc<Mutex<HashMap<String, Vec<PresenceConnection>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel();
        let (tx_c, mut rx_c) = tokio::sync::mpsc::unbounded_channel();
        map.lock().unwrap().insert(
            "sess-a".to_string(),
            vec![
                PresenceConnection { push: tx_a },
                PresenceConnection { push: tx_b },
            ],
        );
        map.lock().unwrap().insert(
            "sess-b".to_string(),
            vec![PresenceConnection { push: tx_c }],
        );
        let frame = Response::SessionCounters {
            background: 1,
            ghosts: 0,
            shells: 2,
        };
        let pushed = presence_push_to_session_in(&map, "sess-a", &frame);
        assert_eq!(pushed, 2, "both of session A's connections got the frame");
        assert!(rx_c.try_recv().is_err(), "session B got nothing");
        for (which, rx) in [("conn a", &mut rx_a), ("conn b", &mut rx_b)] {
            let got = rx
                .try_recv()
                .unwrap_or_else(|e| panic!("{which} received the frame: {e}"));
            assert_eq!(
                serde_json::from_str::<Response>(&got).unwrap(),
                frame,
                "{which} received the counters frame"
            );
        }
        assert!(rx_a.try_recv().is_err());
        assert!(rx_b.try_recv().is_err());
    }
}
