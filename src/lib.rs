//! DaemonEye library crate.
//!
//! Core types and modules for the daemon, CLI, and integration tests.

pub mod agents;
pub mod ai;
pub mod cli;
pub mod config;
pub mod cost;
pub mod daemon;
pub mod ipc;
pub mod scheduler;
pub mod scripts;
pub mod session_store;
pub mod shell;
pub mod webhook;

pub(crate) mod header;
pub(crate) mod manifest;
pub(crate) mod memory;
pub(crate) mod pane_prefs;
pub(crate) mod runbook;
pub(crate) mod search;
pub(crate) mod sys_context;
pub(crate) mod tmux;
pub(crate) mod util;

/// Single global lock used by tests that mutate the HOME environment variable.
/// All test modules that call `env::set_var("HOME", ...)` must hold this lock.
///
/// This is unconditionally `pub` so integration tests (which are a separate
/// crate and do not get `#[cfg(test)]` items from the library) can access it.
pub static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`TEST_HOME_LOCK`], recovering if a previous holder panicked.
///
/// A test that panics while holding the lock poisons it. Every later
/// `.lock().unwrap()` on a poisoned mutex then panics too, so one real failure
/// becomes a failure in every HOME-dependent test in the same binary — 48
/// instead of 1, measured. Recovering keeps the count honest: the test that
/// actually broke is the only one that fails.
///
/// Unconditionally `pub`, not `#[cfg(test)]`, for the same reason the lock is:
/// integration tests are a separate crate and do not receive `#[cfg(test)]`
/// items from the library.
pub fn test_home_guard() -> TestHomeGuard {
    let lock = TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    TestHomeGuard {
        home: std::env::var("HOME").ok(),
        _lock: lock,
    }
}

/// RAII guard returned by [`test_home_guard`].
///
/// Holds [`TEST_HOME_LOCK`] *and* snapshots `HOME` on acquisition, restoring it
/// when dropped. Restoring here rather than by hand in each test is what keeps
/// the process-global honest: the suite had ~109 `set_var("HOME", …)` sites and
/// only a handful of matching restores, so a test that read `HOME` ambiently
/// could see a path left behind by an unrelated test — or a deleted `TempDir`.
/// That produced a real, measured flake twice (M6 phases 09 and 11), each time
/// surfacing in a *different* victim, because the bug belongs to the writers and
/// the symptom lands on whoever reads next.
///
/// `Drop` restores before the mutex is released, so the next guard holder never
/// observes this test's `HOME`.
pub struct TestHomeGuard {
    home: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestHomeGuard {
    fn drop(&mut self) {
        // SAFETY: `set_var`/`remove_var` are unsound only under concurrent env
        // access; we still hold TEST_HOME_LOCK here, which is the lock every
        // HOME-touching test takes.
        unsafe {
            match &self.home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

#[cfg(test)]
mod test_home_guard_tests {
    use super::test_home_guard;

    /// The guard must restore `HOME` even when the test never restores it.
    ///
    /// This is the regression guard for the fix itself: before it, ~109
    /// `set_var("HOME", …)` sites had only a handful of matching restores, and
    /// the leak surfaced as a flake in whichever test happened to read `HOME`
    /// ambiently next.
    ///
    /// Both observations are made *while holding the guard*. An earlier version
    /// of this test read the pre-state unguarded and was itself flaky — it
    /// captured whatever `HOME` a concurrently-running test had installed, which
    /// is precisely the ambient-read bug this whole change exists to remove.
    #[test]
    fn guard_restores_home_on_drop() {
        const PROBE: &str = "/tmp/de-guard-restore-probe";

        {
            let _g = test_home_guard();
            // Deliberately do NOT restore — the guard is what must.
            unsafe {
                std::env::set_var("HOME", PROBE);
            }
        }

        // Re-acquire so the observation is serialised against every other
        // HOME-touching test. If Drop failed to restore, the probe value is
        // still installed here.
        let _g = test_home_guard();
        assert_ne!(
            std::env::var("HOME").ok().as_deref(),
            Some(PROBE),
            "test_home_guard must restore HOME on drop — the probe value leaked"
        );
    }
}
