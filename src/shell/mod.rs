//! Shell engine building blocks shared by the M20 phases.
//!
//! `pty.rs` holds the two primitives every later phase builds on: a PTY-backed
//! shell spawned through `portable-pty`, and the marker protocol that returns a
//! command's real exit code and its exact output bytes.

mod backend;
mod client;
mod host;
mod launch;
mod log;
mod proto;
mod pty;
mod registry;
mod screen;
mod session;

pub use backend::PtyBackend;
pub use client::{run_command, send_signal, status};
pub use host::{ShellBackend, bind, serve};
pub use launch::{await_ready, cast_path, lock_path, run_host, socket_path, spawn_detached};
pub use log::{CastWriter, CommandRecord, MetaIndex, meta_path_for, read_command_output};
pub use proto::{ShellRequest, ShellResponse, ShellSignal, ShellState, encode};
pub use pty::{
    CommandOutcome, Nonce, PtyShell, exit_var, parse_outcome, strip_markers, wrap_command,
};
pub use registry::{
    Owner, ShellId, ShellRecord, ShellRegistry, adopt_at_startup, allocate_index, check_owner_cap,
    host_is_alive, parse_cast_name, scan_shell_ids, spawn_for, sweep_stale, with_shells,
};
pub use screen::ShellScreen;
pub use session::{login_shell, shell_for_owner, spawn_detached_host};

use std::sync::OnceLock;

static REGISTRY: OnceLock<ShellRegistry> = OnceLock::new();

/// The daemon's single live shell registry. `ShellRegistry` is `Default` and
/// holds its own `Arc<Mutex<…>>`, so this needs no initialisation step and
/// has no ordering hazard: the first caller creates it, everyone gets the
/// same one.
pub fn registry() -> &'static ShellRegistry {
    REGISTRY.get_or_init(ShellRegistry::default)
}

/// The first shell id `owner` currently holds in the daemon's registry, if
/// any. Read-only convenience over [`registry`] so error paths can name the
/// shell without reopening the registry lock themselves.
pub fn registry_lookup<T>(owner: &Owner, f: impl FnOnce(&ShellId) -> T) -> Option<T> {
    let key = owner.key();
    let mut result: Option<T> = None;
    with_shells(registry(), |store| {
        if let Some(record) = store.values().find(|record| record.owner.key() == key) {
            result = Some(f(&record.id));
        }
    });
    result
}
