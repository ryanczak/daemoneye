# Bug 1 on phase-05a: `GcKill` stole `gc_bg_windows`'s doc comment

**Severity:** minor
**Status:** open
**Filed:** 2026-07-26

## What's wrong

`struct GcKill` was inserted **between** `gc_bg_windows`'s pre-existing doc block
and the function itself, and the doc comment the spec supplied for `GcKill` was
dropped. `src/daemon/background/gc.rs:150-168`:

```rust
/// Periodic garbage collector for background windows.
///
/// Called every 60 seconds by the `bg-window-gc` supervised task.
///
/// For each session's tracked `bg_windows`:
/// - Kills windows whose pane is gone, dead, or has been idle since completing.
///
/// Also scans all tmux panes for daemon-prefixed windows not tracked by any
/// session (orphans from a daemon restart or missed completion signal) and
/// kills those too.
struct GcKill {
    session_id: String,
    window_name: String,
    tmux_session: String,
    pane_id: String,
    reason: &'static str,
}

pub fn gc_bg_windows(sessions: &crate::daemon::session::SessionStore) {
```

Two things are now wrong at once:

1. **`gc_bg_windows` is undocumented.** A `pub fn` lost its entire doc block.
2. **`GcKill` is actively mis-documented.** A five-field record of one window to
   kill is now described as a "periodic garbage collector … called every 60
   seconds … also scans all tmux panes." Every sentence of it is false of the
   struct it is attached to.

This compiles and lints clean — nothing in the gate set can see it. It is the
documentation analogue of the failure mode this milestone keeps flagging: text
that is trusted precisely because it looks authoritative.

## What should happen

The phase spec (§ Spec, task 2, "Add a private struct immediately above
`gc_bg_windows`") gave the struct **with its own doc comment**:

```rust
/// One window the GC has decided to kill, captured under the lock so the kill
/// itself can happen outside it.
struct GcKill {
```

`gc_bg_windows` keeps the doc block it had before this phase, immediately above
`pub fn gc_bg_windows`. The phase is a behavior-preserving restructure — it was
not authorized to move or remove any existing documentation.

## How to fix

In `src/daemon/background/gc.rs`, reorder so each doc block sits on its intended
item. Move the `GcKill` definition **above** the `/// Periodic garbage collector
…` block, and restore the spec's doc comment on it:

```rust
/// One window the GC has decided to kill, captured under the lock so the kill
/// itself can happen outside it.
struct GcKill {
    session_id: String,
    window_name: String,
    tmux_session: String,
    pane_id: String,
    reason: &'static str,
}

/// Periodic garbage collector for background windows.
///
/// Called every 60 seconds by the `bg-window-gc` supervised task.
///
/// For each session's tracked `bg_windows`:
/// - Kills windows whose pane is gone, dead, or has been idle since completing.
///
/// Also scans all tmux panes for daemon-prefixed windows not tracked by any
/// session (orphans from a daemon restart or missed completion signal) and
/// kills those too.
pub fn gc_bg_windows(sessions: &crate::daemon::session::SessionStore) {
```

**This is the only change.** Do not touch the restructure itself — the locked /
unlocked split, the `GcKill` fields, the `retain`'s `tracked` insert, and the two
`kill_job_window` call sites are all correct and reviewed. Do not add or remove
any other comment. Do not write the literal `sessions.lock()`, `with_sessions(`,
`cleanup_bg_windows`, or `kill_job_window` in a new comment — the phase's `grep -c`
criteria count raw text including comments.

`hook.rs` and `helpers.rs` need **no** changes.

## Verification

- [ ] `sed -n '148,175p' src/daemon/background/gc.rs` shows the `GcKill` doc
      comment on `struct GcKill` and the "Periodic garbage collector" block
      directly above `pub fn gc_bg_windows`, with no item between them.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged.
- [ ] `python3 /tmp/scan_locks.py src/daemon/hook.rs src/daemon/background/gc.rs src/daemon/background/helpers.rs`
      still prints **0** for all three.
- [ ] `grep -c "with_sessions(" src/daemon/background/gc.rs` still returns **1**.
- [ ] `grep -c "kill_job_window" src/daemon/background/gc.rs` still returns **3**.
