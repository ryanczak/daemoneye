//! Which run is currently in flight for a chat session.
//!
//! `Request::Cancel` names a *session*; the cancel registry is keyed by
//! *run*. This is the one mapping between them: it exists so the chat
//! client's Ctrl-C can find the run id to cancel. It is deliberately
//! separate from the cancel registry — that one holds every run kind, this
//! one answers only "which run is this chat session's turn?".
//!
//! The file also carries the run's `CancelDoor` mark. Both cancel doors fire
//! `CancelReason::UserStop` (phase-03 closed the reason set, and it must not
//! be widened), so the shared signal cannot say which door fired; the stage
//! recorded in `RunState::Cancelled` has to. Each door marks just before it
//! flips the signal, and the turn's cancel branch consumes the mark.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::util::UnpoisonExt;

use super::{CancelReason, RunId};

static ACTIVE: OnceLock<Mutex<HashMap<String, RunId>>> = OnceLock::new();
static DOORS: OnceLock<Mutex<HashMap<String, CancelDoor>>> = OnceLock::new();

fn active() -> &'static Mutex<HashMap<String, RunId>> {
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn doors() -> &'static Mutex<HashMap<String, CancelDoor>> {
    DOORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The cancel door a run's cancellation arrived through. Both wired doors
/// fire `CancelReason::UserStop`, so the reason alone cannot distinguish
/// them; the `stage` recorded in `RunState::Cancelled` names the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDoor {
    /// The chat client's `Request::Cancel`, resolved through the session map.
    Chat,
    /// The `var/run/stop-<run>` filesystem sentinel (`daemoneye stop <run>`).
    Sentinel,
}

impl CancelDoor {
    /// The `stage` string recorded in `RunState::Cancelled` when cancellation
    /// arrived through this door.
    pub fn stage(self) -> &'static str {
        match self {
            CancelDoor::Chat => "chat_cancel",
            CancelDoor::Sentinel => "stop_sentinel",
        }
    }
}

/// Mark which door is about to cancel `run_id`, so the turn can name it in
/// its recorded stage. Read back by [`take_cancel_door`].
pub fn mark_cancel_door(run_id: &RunId, door: CancelDoor) {
    doors()
        .lock()
        .unwrap_or_log()
        .insert(run_id.as_str().to_string(), door);
}

/// Consume the run's door mark. `None` when no door marked it — the signal
/// can also fire through a bare `cancel_run` that set no mark.
pub fn take_cancel_door(run_id: &RunId) -> Option<CancelDoor> {
    doors().lock().unwrap_or_log().remove(run_id.as_str())
}

/// Register `run_id` as the run currently in flight for `session_id`.
///
/// Registering a second run for the same session **replaces** the first: a
/// session has at most one turn in flight, and a stale entry would let a
/// cancel hit a finished run. Deregistration happens on guard drop, so every
/// exit path of the turn cleans up.
pub fn register_active(session_id: &str, run_id: &RunId) -> ActiveRunGuard {
    active()
        .lock()
        .unwrap_or_log()
        .insert(session_id.to_string(), run_id.clone());
    ActiveRunGuard {
        session_id: session_id.to_string(),
    }
}

/// The run currently in flight for `session_id`, if any.
pub fn run_for_session(session_id: &str) -> Option<RunId> {
    active().lock().unwrap_or_log().get(session_id).cloned()
}

/// Cancel whatever run `session_id` has in flight. Returns whether one was
/// found — the same contract `cancel_turn` had, so the IPC handler's log
/// line keeps its meaning. Marks the run's door `Chat` before flipping the
/// signal, so the turn records the chat door's stage.
pub fn cancel_session(session_id: &str, reason: CancelReason) -> bool {
    let run_id = match run_for_session(session_id) {
        Some(id) => id,
        None => return false,
    };
    mark_cancel_door(&run_id, CancelDoor::Chat);
    if super::cancel::cancel_run(&run_id, reason) {
        true
    } else {
        // The run deregistered between the lookup and the fire; nobody will
        // consume the mark, so remove it.
        take_cancel_door(&run_id);
        false
    }
}

/// Guard deregistering a session's active run on drop.
pub struct ActiveRunGuard {
    session_id: String,
}

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        let run_id = active().lock().unwrap_or_log().remove(&self.session_id);
        // A mark set by a door whose cancel lost the race with the turn's
        // exit would otherwise sit in the map forever.
        if let Some(run_id) = run_id {
            doors().lock().unwrap_or_log().remove(run_id.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::daemon::run::cancel::test_support::HomeGuard;
    use crate::daemon::run::register_run;

    /// A run id no other test in this binary will ever register. The
    /// registries are process-globals, and libtest runs these tests in
    /// parallel — every test that inserts into one must use its own id, or
    /// one test's registration is another test's flake. Same shape as
    /// `run::cancel`'s `fresh_run_id`.
    fn fresh_run_id() -> RunId {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0xa000_0000);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        RunId::parse(&format!("0badcafe{n:08x}")).expect("fixture id is valid hex")
    }

    /// A session id no other test will ever register, for the same reason
    /// the run ids are unique.
    fn fresh_session_id() -> String {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("active-session-{n}")
    }

    #[test]
    fn active_register_then_run_for_session_returns_that_run() {
        let _home = HomeGuard::new();
        let session = fresh_session_id();
        let run_id = fresh_run_id();
        let _active_guard = register_active(&session, &run_id);
        assert_eq!(
            run_for_session(&session),
            Some(run_id),
            "run_for_session must return the run just registered"
        );
    }

    #[test]
    fn active_run_for_an_unknown_session_is_none() {
        let _home = HomeGuard::new();
        let session = fresh_session_id();
        assert_eq!(
            run_for_session(&session),
            None,
            "an unknown session must have no in-flight run"
        );
    }

    #[test]
    fn active_guard_drop_deregisters() {
        let _home = HomeGuard::new();
        let session = fresh_session_id();
        let run_id = fresh_run_id();
        {
            let _active_guard = register_active(&session, &run_id);
            assert_eq!(
                run_for_session(&session),
                Some(run_id),
                "the entry must be live while the guard is"
            );
        }
        assert_eq!(
            run_for_session(&session),
            None,
            "the entry must be None after the guard drops"
        );
    }

    #[test]
    fn active_re_registering_a_session_replaces_the_previous_run() {
        let _home = HomeGuard::new();
        let session = fresh_session_id();
        let run_a = fresh_run_id();
        let run_b = fresh_run_id();
        let (_guard_a, signal_a) = register_run(&run_a);
        {
            let _active_a = register_active(&session, &run_a);
            let (_guard_b, signal_b) = register_run(&run_b);
            let _active_b = register_active(&session, &run_b);
            assert_eq!(
                run_for_session(&session),
                Some(run_b.clone()),
                "the newer run must be the one in flight"
            );
            assert!(
                cancel_session(&session, CancelReason::UserStop),
                "cancel_session must find the session's in-flight run"
            );
            assert_eq!(
                signal_b.reason(),
                Some(CancelReason::UserStop),
                "cancel_session must cancel run B, the run now in flight"
            );
            assert_eq!(
                signal_a.reason(),
                None,
                "run A must be untouched: a stale entry cancelling a finished \
                 run is exactly the bug last-wins prevents"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn active_cancel_session_cancels_the_registered_run() {
        let _home = HomeGuard::new();
        let session = fresh_session_id();
        let run_id = fresh_run_id();
        let (_run_guard, mut signal) = register_run(&run_id);
        let _active_guard = register_active(&session, &run_id);
        let found = cancel_session(&session, CancelReason::UserStop);
        assert!(found, "cancel_session must report the run it found");
        assert_eq!(
            signal.reason(),
            Some(CancelReason::UserStop),
            "the run registered for the session must be the one cancelled"
        );
        let resolved = signal.cancelled().await;
        assert_eq!(
            resolved,
            CancelReason::UserStop,
            "the signal must resolve to the reason cancel_session fired with"
        );
    }

    #[test]
    fn active_cancel_session_on_an_unknown_session_is_false() {
        let _home = HomeGuard::new();
        let session = fresh_session_id();
        let run_id = fresh_run_id();
        let (_run_guard, signal) = register_run(&run_id);
        assert!(
            !cancel_session(&session, CancelReason::UserStop),
            "an unknown session must report no in-flight run"
        );
        assert_eq!(
            signal.reason(),
            None,
            "an unknown session's cancel must not touch any registered run"
        );
    }
}
