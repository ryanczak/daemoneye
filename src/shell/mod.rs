//! Shell engine building blocks shared by the M20 phases.
//!
//! `pty.rs` holds the two primitives every later phase builds on: a PTY-backed
//! shell spawned through `portable-pty`, and the marker protocol that returns a
//! command's real exit code and its exact output bytes.

mod pty;

pub use pty::{
    CommandOutcome, Nonce, PtyShell, exit_var, parse_outcome, strip_markers, wrap_command,
};
