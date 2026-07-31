# Phase 09: Pane and Archive Retention

**Milestone:** M6 — Verification & Hygiene
**Status:** in-progress
**Depends on:** phase-07 (done), phase-08 (done)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Close the last three artifact gaps phase 07's table left `Pending { owned_by:
"phase-09" }`:

1. **`var/log/panes/`** — 264 files, no sweep at all.
2. **`agents/*/mailbox/`** — one file per ghost exit, forever.
3. **The off-by-default asymmetry** — `sessions.archive_retention_days` defaults
   to `0` (keep forever) while `events.retention_days` defaults to `90`, and
   nothing tells the operator.

Both new retentions are **7 days**, and both **must be operator-configurable**
— PE decision, 2026-07-30. Shipping hard-coded values is not acceptable here.

## Architecture references

Read before starting:

- `src/config/lifecycle.rs` — phase 07's policy table. The `var/log/panes` and
  `agents/*/mailbox` entries are `Pending { owned_by: "phase-09" }` and say
  explicitly that phase 09 must add their config keys. **You update both.**
- `src/daemon/utils/event_log.rs:228` (`sweep_event_segments`) — **the pattern to
  copy.** Read it first; your two sweeps are its siblings.
- `src/daemon/utils/mod.rs:20` (`sweep_session_archives`) — the other existing
  sweep, and the one whose `0` default this phase surfaces.
- `src/daemon/utils/log_rotation.rs` — phase 08's pure-seam split. Same idea
  applies to task 4.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read both existing sweeps in full before writing either new one.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 972 lib / 30
   integration (2 ignored) / 8 isolation (1 ignored).

## Current state

**Verified against the tree while drafting.**

Both existing sweeps share one shape (`sweep_event_segments`,
`sweep_session_archives`):

```rust
pub fn sweep_x(retention_days: u32 /*, …*/) {
    if retention_days == 0 { return; }          // 0 = keep forever
    let dir = crate::config::…;
    let Ok(entries) = std::fs::read_dir(&dir) else { return; };
    let cutoff = /* now - retention_days */;
    for entry in entries.filter_map(|e| e.ok()) {
        // skip non-matching names; compare mtime (or parsed date) to cutoff;
        // log::info! then remove_file, log::warn! on failure
    }
}
```

Copy that shape. Note `sweep_session_archives` also takes an `active_sessions`
set and skips live sessions — your sweeps have no such liveness concern, so do
not invent one.

**Directories, verified:**

- Pane logs: `crate::config::pane_logs_dir()` → `var/log/panes/`.
- Mailboxes: `crate::agents::mailbox::mailbox_dir(agent_name)` →
  `agents/<name>/mailbox/`, holding `<job_id>.json` written by
  `write_mailbox_on_exit` on every ghost exit. There is **one mailbox per
  agent**, so the sweep must iterate agents, not a single directory.

**The tick that fires sweeps** is in `src/daemon/mod.rs`, inside the
`"session-cleanup"` supervisor's `async move` block, guarded by
`if sweep_counter.is_multiple_of(60)`. Phase 08 added a `rotate_log_file` call
there; yours go beside it. Values reach that block from `startup_config`, which
the closure already captures — so `startup_config.<your_section>.<your_key>` is
how your new settings get in. **Do not add a second timer, task, or thread.**

## Spec

### 1. `sweep_pane_logs`

A sibling of `sweep_event_segments`, over `pane_logs_dir()`, deleting `.log`
files older than the retention. Takes `retention_days: u32` as a parameter;
`0` means keep forever, exactly like its siblings.

### 2. `sweep_agent_mailboxes`

Same shape, but iterate every agent's mailbox directory and delete `.json` files
older than the retention. An agent with no mailbox directory is not an error —
skip it.

### 3. Two config keys, both defaulting to 7

Add operator-tunable retentions for panes and mailboxes, following the existing
convention in `src/config/types.rs` — `#[serde(default = "…")]` plus a
`default_*()` function, as `default_severity_threshold` and phase 08's logging
defaults do. Put them wherever they read best alongside the existing
`events.retention_days` / `sessions.archive_retention_days`.

**7 days for both, by PE decision.** Do not choose different numbers, and do not
ship the sweeps reading hard-coded constants — the whole point of this task is
that an operator can change them.

### 4. Surface the off-by-default asymmetry — as a testable function

The criterion is that a sweep which is **off by default says so where the
operator will see it**. Today `sessions.archive_retention_days` is `0` and
nothing mentions it.

Follow phase 08's split — the decision is a pure function, the side effect is
the daemon's:

- **Pure and testable:** a function taking `&Config` and returning the warnings
  that apply — one per artifact class whose retention is `0` (keep-forever),
  naming the class, the config key, and what the operator can set it to. Empty
  vec when nothing is disabled.
- **Daemon-side:** log each warning once at startup, at `WARN`, in `run_daemon`.

**Do not change `archive_retention_days`'s default.** The criterion asks for
visibility, not a behaviour change, and silently switching a keep-forever
default to a deleting one would destroy operator data.

**Do not extend the IPC `Response::DaemonStatus` payload** to carry this. That
touches `ipc.rs`, the server handler and `cli/status.rs` for a one-line benefit;
a startup WARN meets the criterion. If you think otherwise, report a blocker
rather than doing it.

### 5. Wire both sweeps into the existing tick

Beside `sweep_event_segments` / `sweep_session_archives` / `rotate_log_file`, in
the same `is_multiple_of(60)` block, reading your new values off
`startup_config`.

### 6. Update the phase-07 policy table

Flip **both** `var/log/panes` and `agents/*/mailbox` to implemented, each naming
its new `config_key`, and update their notes so they no longer say phase 09 must
add the knob — it did. Phase 07's Direction B test asserts the table stays
truthful.

**Change no other entry.**

## Acceptance criteria

- [ ] `sweep_pane_logs` and `sweep_agent_mailboxes` each take a retention
      parameter, are called directly from tests, and treat `0` as keep-forever.
- [ ] A test writes an old file and a fresh file into each location, sweeps, and
      asserts the old one is gone and the fresh one survives.
- [ ] A test asserts `0` sweeps nothing.
- [ ] Both config keys exist, default to **7**, and the sweeps read them —
      nothing hard-codes a retention.
- [ ] The warning function returns a warning for `archive_retention_days = 0`
      and none when it is non-zero, and the daemon logs it at startup.
- [ ] `archive_retention_days`'s default is still `0`; `events.retention_days`
      is still `90`.
- [ ] Both policy-table entries are implemented and name their config keys; no
      other entry changed.
- [ ] Phase 07's three lifecycle tests and phase 08's five rotation tests still
      pass.
- [ ] All four gates green.

## Test plan

Prefer `tempfile::tempdir()` and a path parameter over `HOME` juggling wherever
the function shape allows it — phase 08's rotation tests needed no `HOME` guard
at all, which is a good sign the seam is right. Where `HOME` is unavoidable
(the sweeps resolve their own directories via `config::`), take
`crate::test_home_guard()` (`src/lib.rs:45`) — not the raw `TEST_HOME_LOCK`
(`:32`) — hold it through all HOME-dependent work, and drop it at the end.

Set file ages with an explicit mtime rather than sleeping — `filetime` is already
a dev-dependency. **`STANDARDS.md` §3.3 forbids `sleep` in tests**, and phase 06a
was bounced for exactly that.

**Mutation-check both sweeps before reporting.** Break each cutoff comparison so
nothing is ever deleted, confirm the corresponding test **fails**, revert,
confirm it passes. Quote both runs. A retention test that passes when the sweep
is disabled is the vacuous coverage this milestone exists to eliminate.

**Do not pin a test count in advance.** Report the resulting count in the Update
Log and explain the delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the file's contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase receives automatically. **It does not satisfy
this requirement** — it has cost six bounces on this milestone.

Run exactly this:

```sh
# Mutation: disable both cutoffs so nothing is deleted.
cargo test --lib sweep -- --nocapture \
  > /tmp/e2e-09-red.txt 2>&1; echo "exit=$?" >> /tmp/e2e-09-red.txt

git checkout -- src/

cargo test --lib sweep -- --nocapture \
  > /tmp/e2e-09-green.txt 2>&1; echo "exit=$?" >> /tmp/e2e-09-green.txt
```

Paste both files' contents. Also paste a directory listing from one of your
tempdirs showing the old file gone and the fresh one surviving, and the warning
string your function produces for `archive_retention_days = 0`.

## Authorizations

- [ ] May add sweeps to `src/daemon/utils/` and the warning function wherever it
      reads best.
- [ ] May add two config keys to `src/config/types.rs`.
- [ ] May modify the cleanup tick and `run_daemon`'s startup path in
      `src/daemon/mod.rs`.
- [ ] May update **only** the `var/log/panes` and `agents/*/mailbox` entries in
      `src/config/lifecycle.rs`.

No new dependencies (`filetime` is already a dev-dependency). No changes to
`docs/architecture.md`.

## Out of scope

- **Do not change any existing default** — `archive_retention_days` stays `0`,
  `events.retention_days` stays `90`, phase 08's logging defaults stay as they
  are.
- **Do not modify `sweep_event_segments`, `sweep_session_archives`, or
  `rotate_log_file`** — only add your calls beside them.
- **Do not extend the IPC status payload** (see task 4).
- **Do not add a second timer, background task, or thread.**
- **Do not touch `src/pane_prefs.rs`** — pane *preferences* are phase 10 and are
  unrelated to pane *logs*, despite the similar name.
- **Do not touch `.gitignore`, `main.rs`'s stale `daemon.log` help strings, or
  the pre-existing `tokio::time::sleep` at `tests/integration.rs:615`.** Phase 11
  and milestone housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-31 00:14 (started)

**Executor:** model (phase-09 executor)

Implementing sweep_pane_logs, sweep_agent_mailboxes, two config keys (defaulting to 7), the off-by-default warning function, wiring into the cleanup tick, and updating the phase-07 policy table.
