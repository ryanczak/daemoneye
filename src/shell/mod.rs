//! Shell engine building blocks shared by the M20 phases.
//!
//! `pty.rs` holds the two primitives every later phase builds on: a PTY-backed
//! shell spawned through `portable-pty`, and the marker protocol that returns a
//! command's real exit code and its exact output bytes.

mod backend;
mod client;
mod foreground;
mod host;
mod launch;
mod log;
mod proto;
mod pty;
mod registry;
mod sbx_backend;
mod screen;
mod session;
pub mod status;

pub use crate::daemon::utils::PromptKind;
pub use backend::PtyBackend;
pub use client::{
    CommandHandle, PromptGivenUp, PromptReply, ShellExited, close_or_term, run_command,
    run_command_with_prompts, screen, send_input, send_resize, send_signal, spawn_command, status,
};
pub use foreground::{Foreground, classify, classify_record, classify_transport};
pub use host::{ShellBackend, bind, serve, serve_until_exit};
pub use launch::{
    await_ready, cast_path, lock_path, resolve_cast, run_host, socket_path, spawn_detached,
};
pub use log::{
    CastWriter, CommandRecord, MetaIndex, marked_inner_range, meta_path_for, read_command_output,
    read_output_since,
};
#[cfg(test)]
pub(crate) use log::{decode_payload, extend_output_line};
pub use proto::{ShellRequest, ShellResponse, ShellSignal, ShellState, encode};
pub use pty::{
    CommandOutcome, Nonce, PtyShell, exit_var, parse_outcome, strip_markers, wrap_command,
};
#[cfg(test)]
pub(crate) use pty::{test_begin_marker, test_end_marker};
pub use registry::{
    InFlight, InFlightGuard, Owner, ShellId, ShellRecord, ShellRegistry, adopt_at_startup,
    allocate_index, check_owner_cap, clear_in_flight, host_is_alive, is_user_shell,
    mark_interactive, now_unix, parse_cast_name, reap_exited, scan_shell_ids, set_in_flight,
    spawn_for, spawn_for_in_sandbox, spawn_for_on_host, sweep_stale, with_shells,
};
pub use sbx_backend::{SbxBackend, SbxRunner, VmRun, real_runner};
pub use screen::ShellScreen;
pub use session::{
    PROBE_TIMEOUT, clear_sandbox_sidecar, host_entry, host_entry_or_bare, login_shell,
    marker_shell_name, master_running, probe_foreground, shell_for_owner,
    shell_for_owner_in_sandbox, shell_marker_name, spawn_dedicated_for, spawn_detached_host,
    spawn_ghost_ssh_host, spawn_sbx_host, spawn_ssh_host, which_sbx, which_ssh,
};

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
