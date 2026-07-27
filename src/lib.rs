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
pub fn test_home_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
