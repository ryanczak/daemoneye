//! Shell engine building blocks shared by the M20 phases.
//!
//! `pty.rs` holds the two primitives every later phase builds on: a PTY-backed
//! shell spawned through `portable-pty`, and the marker protocol that returns a
//! command's real exit code and its exact output bytes.

mod backend;
mod host;
mod launch;
mod log;
mod proto;
mod pty;
mod registry;
mod screen;

pub use backend::PtyBackend;
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
