//! DaemonEye library crate.
//!
//! Core types and modules for the daemon, CLI, and integration tests.

pub mod agents;
pub mod ai;
pub mod cli;
pub mod config;
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
