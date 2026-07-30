# Phase 08: Daemon-Log Rotation

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-07 (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Bound `var/log/daemon.log`. Phase 07 recorded its policy as **Rotate**, owned by
this phase; phase 08 makes that true and flips the table entry to implemented.

The exit criterion is explicit that the bound must be **exercised by a test, not
asserted in prose** — so the rotation logic has to be reachable from a test
without standing up a daemon.

## Architecture references

Read before starting:

- `src/config/lifecycle.rs` — the phase-07 policy table. The `var/log/daemon.log`
  entry is `LifecycleIntent::Rotate`, `config_key: None`,
  `ImplementationStatus::Pending { owned_by: "phase-08" }`. **You update it.**
- `src/daemon/mod.rs:371-394` — where the log file is opened and `dup2`'d. Read
  this before designing anything; see the constraint below.
- `src/daemon/mod.rs:819-828` — the existing cleanup tick that fires
  `sweep_event_segments` / `sweep_session_archives` every 60th iteration.
- `src/daemon/utils/event_log.rs:228` (`sweep_event_segments`) — the shape a
  sweep function takes in this codebase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read `src/daemon/mod.rs:340-400` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 967 lib / 30
   integration (2 ignored) / 8 isolation (1 ignored).

## Current state — the constraint that decides the design

**The log is not written through a Rust writer you control.** `run_daemon`
(`src/daemon/mod.rs:371-394`) opens the file `O_APPEND` and `dup2`s its
descriptor onto **stdout (1) and stderr (2)**:

```rust
let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
let fd = file.as_raw_fd();
unsafe {
    if libc::dup2(fd, 1) < 0 { … }
    if libc::dup2(fd, 2) < 0 { … }
}
```

`env_logger` then writes to stderr. So:

- **A plain rename is not enough.** After `rename(daemon.log, daemon.log.1)`,
  fds 1 and 2 still refer to the *same inode*, now named `daemon.log.1`. Logging
  would silently continue into the rotated file and the live log would stay
  empty — a rotation that looks like it worked and doesn't. Whatever you do must
  **re-open the new path and `dup2` the fresh descriptor onto 1 and 2**.
- `O_APPEND` means a truncate-in-place variant is also viable (copy out, then
  `ftruncate`), because appending writes always go to the current end. Either
  approach is acceptable; say in a comment which you chose and why.

**There is no logging config section today.** `daemon.log`'s policy entry has
`config_key: None` precisely because the knob does not exist yet. This phase adds
it — that is in scope here, unlike in phase 07.

**There is one cleanup tick already** (`src/daemon/mod.rs:819-828`), firing every
60th iteration. It is the natural home for a periodic size check; a startup-only
check would not bound a daemon that runs for weeks, which is exactly how the live
log reached 25.8 MB since May 8.

## Spec

### 1. A testable rotation seam

Split the work so the part that can be tested is not entangled with the part that
cannot:

- **File shifting — pure and directly testable.** Given a path, a size bound and
  a keep-count, decide whether rotation is due and perform the on-disk shuffle
  (`daemon.log` → `daemon.log.1` → `.2` … dropping beyond the keep-count).
  Takes its inputs as parameters; touches no globals; returns whether it rotated.
  Put it beside the existing sweeps in `src/daemon/utils/`.
- **Descriptor re-attach — daemon-only.** Re-opening the path and `dup2`-ing onto
  1 and 2 is process-global and cannot be asserted in a unit test. Keep it in the
  daemon path, calling the function above.

This split is the phase's main design requirement. A rotation function that
performs its own `dup2` internally cannot be tested and will be bounced.

### 2. Configuration

Add a logging section with a size bound and a keep-count, with defaults. Follow
the existing config conventions in `src/config/types.rs` — `#[serde(default =
"…")]` plus a `default_*()` function, exactly like `default_severity_threshold`
and `default_dedup_window`.

**Choose defaults that bound the observed problem** and say why in a comment: the
live log reached 25.8 MB in ~12 weeks. State your numbers in the Update Log so
the reviewer can weigh them.

### 3. Wire it into the existing tick

Call the size check from the cleanup tick at `src/daemon/mod.rs:819-828`,
alongside the two existing sweeps. Do not add a second timer, a background task,
or a new thread.

### 4. Update the phase-07 policy table

Flip `var/log/daemon.log`'s entry from `Pending { owned_by: "phase-08" }` to
implemented, and set `config_key` to the key you added. Phase 07's Direction B
test asserts entries stay truthful, so leaving it `Pending` after implementing it
makes the table lie.

**Change no other entry.** `var/log/panes` and `agents/*/mailbox` stay
`Pending { owned_by: "phase-09" }`, including their proposed retention numbers —
those are phase 09's to confirm or revise.

## Acceptance criteria

- [ ] A rotation function takes path, size bound and keep-count as parameters and
      is called directly from a test — no daemon, no globals.
- [ ] A test writes a file over the bound, rotates, and asserts: the old content
      is in `.1`, the live path is a fresh empty file, and no file beyond the
      keep-count survives.
- [ ] A test asserts a file **under** the bound is left alone.
- [ ] The daemon re-attaches fds 1 and 2 after rotating, so logging continues to
      the live path rather than the rotated inode.
- [ ] A logging config section exists with documented defaults.
- [ ] `var/log/daemon.log`'s policy entry is implemented and names the config key;
      every other entry is unchanged.
- [ ] Phase 07's three lifecycle tests still pass.
- [ ] All four gates green.

## Test plan

**Tests that touch `HOME` must take `crate::test_home_guard()`**
(`src/lib.rs:45`) — not the raw `TEST_HOME_LOCK` (`:32`). Edition 2024, so
`std::env::set_var` needs `unsafe`. Hold the guard through all HOME-dependent
work and drop it at the end.

The rotation tests should not need `HOME` at all if the function takes a path
parameter — prefer a plain `tempfile::tempdir()`, which sidesteps the guard
entirely. That is a signal the seam in task 1 is right.

**Mutation-check the bound before reporting.** Change the comparison so rotation
never triggers (or always triggers), confirm the over-bound test **fails**,
revert, confirm it passes. Quote both runs. A rotation test that passes when
rotation is disabled is exactly the vacuous coverage this milestone exists to
eliminate.

**Do not pin a test count in advance.** Report the resulting count in the Update
Log and explain the delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the file's contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase receives automatically. **It does not satisfy
this requirement** — this has now cost six bounces on this milestone.

Capture, as literal commands:

```sh
# Mutation: disable rotation, prove the test goes red.
#   (edit the size comparison so rotation never fires)
cargo test --lib <your rotation test module> -- --nocapture \
  > /tmp/e2e-08-red.txt 2>&1; echo "exit=$?" >> /tmp/e2e-08-red.txt

git checkout -- src/

cargo test --lib <your rotation test module> -- --nocapture \
  > /tmp/e2e-08-green.txt 2>&1; echo "exit=$?" >> /tmp/e2e-08-green.txt
```

Paste both files' contents. The `exit=` markers are the point: a command that
prints nothing still has to be observable.

Also paste a real directory listing showing a rotated set (`daemon.log`,
`daemon.log.1`, …) with sizes, from a throwaway directory your test used.

## Authorizations

- [ ] May add a rotation function under `src/daemon/utils/`.
- [ ] May add a logging config section to `src/config/types.rs`.
- [ ] May modify the cleanup tick at `src/daemon/mod.rs:819-828` and the log-open
      path at `:371-394`.
- [ ] May update **only** the `var/log/daemon.log` entry in
      `src/config/lifecycle.rs`.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not touch `var/log/panes/`, `agents/*/mailbox/`, or
  `sessions.archive_retention_days`.** All three are phase 09's, including the
  proposed retention numbers phase 07 recorded for them.
- **Do not modify `sweep_event_segments` or `sweep_session_archives`** — only add
  your call beside them.
- **Do not add a second timer, background task, or thread.** Reuse the existing
  tick.
- **Do not change `events.retention_days`** or any other existing default.
- **Do not touch `.gitignore`, `src/pane_prefs.rs`, `main.rs`'s stale
  `daemon.log` help strings, or the pre-existing `tokio::time::sleep` at
  `tests/integration.rs:615`.** Phase 11 and milestone housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
