# Phase 04 — scheduler-producers

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-16
**Depends on:** phase-03
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=refactor, size=m

## Goal

Scheduled-job results and watchdog alerts publish notices instead of
pushing over presence and running `on_alert` themselves. With default
routes they reach the chat notice and the `on_alert` command, as in 2.9.0
(plus the tmux overlay — an accepted delta already in the README Notes).

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design and § Notes —
  read the `test_bus` / `rt.enter()` gotcha before touching any test.
- `src/daemon/notice_sinks.rs`: `default_routes`, `test_bus`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

`src/daemon/scheduled.rs` delivers every job outcome through
`notify_job_result(job_name, msg, config, notify_tx)`, which sends on
`notify_tx`, calls `presence_broadcast(&msg)` and `fire_notification`. Its
callers: the `ActionOn::Alert` arm (`"Watchdog alert: <name>"`),
`fail_job` and `fail_job_terminal` (a failure reason), `run_script_job`
(`` `<script>` succeeded|failed (exit N) `` then the `<output>` block) and
`run_watchdog` (`"[Watchdog] <name>: <response>"`). `notify_tx` is
threaded through `run_scheduled_job` and every helper, but its one
production caller (`src/daemon/startup.rs`) passes `None`, so it is dead.
After this phase `fire_notification` (`src/daemon/utils/response.rs`) has
no caller. The ghost-job paths in `scheduled.rs` call `inject_ghost_event`;
they move in phase 05.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
The previous run read 1000 turns across the whole tree and edited nothing.
Everything this phase needs is in these files; **read no others**:

- `src/daemon/scheduled.rs`, `src/daemon/scheduled_tests.rs` — the work.
- `src/daemon/startup.rs` — the one `run_scheduled_job` call.
- `src/daemon/utils/response.rs` — `fire_notification`, to delete.
- `src/daemon/notice_sinks.rs` and `src/bus/notice.rs` — read once for
  `test_bus`, `Notice::new` and its builders; do not change them.
- `CLAUDE.md` and `src/config/types.rs` — one line each (Spec task 3).

Do the edits in this order. Each step ends at a green `cargo build`
(`cargo build --tests` from step 2 on); start the next step only then.

1. In `scheduled.rs`, add the four pure notice builders (Spec task 2) next
   to `notify_job_result`, and change `notify_job_result` to take `bus:
   &'static crate::bus::Bus` and publish the notice instead of sending.
2. Replace `notify_tx` with `bus` through every function in `scheduled.rs`
   and at the `startup.rs` call site, then fix `scheduled_tests.rs`'s
   callers (Spec task 1).
3. Delete `fire_notification` and update the two doc lines (Spec task 3).
4. Add the tests in the Test plan, then run the gates once (Spec task 4).

## Spec

1. **The bus replaces `notify_tx`** — replace the `notify_tx` parameter
   with `bus: &'static crate::bus::Bus` on `run_scheduled_job`,
   `run_scheduled_job_inner` and every helper that took it. `startup.rs`
   passes `crate::bus::global()`. Tests pass `notice_sinks::test_bus(..)`
   (built inside the runtime — see the README gotcha) or a
   `Bus::with_journal` recording bus; **never the global bus in a test**.

2. **Job notices** — replace `notify_job_result` with pure builders and a
   `bus.publish`. Every notice: source `Scheduler`, name the job's name,
   `source_id` the job's id, a `kind` label, title the message's first
   line, body the rest.
   - `ActionOn::Alert`: severity `Warning`, kind `alert`.
   - A script run: `Info` on exit 0, `Error` otherwise, kind `script`.
   - `fail_job` / `fail_job_terminal`: `Error`, kind `failure`.
   - `run_watchdog`'s alert: source **`Watchdog`**, severity `Warning`,
     kind `watchdog`, title `"[Watchdog] <name>: "` + the response's first
     line.

3. **Remove `fire_notification`** — delete it and update its row in
   CLAUDE.md's Key files table (`src/daemon/utils/`) and the
   `severity_threshold` doc comment in `src/config/types.rs` that names
   it. Its behaviour now lives in the command sink.

4. **Gates** — `cargo fmt --all`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test`. Run each **once** when
   your edits are done; a green result ends the task — do not re-run it.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes;
      `scheduled_job_result_is_pushed_to_attached_clients` passes with its
      assertion unchanged (only its setup moves to the bus).
- [ ] `grep -rn "notify_tx" src/` prints nothing.
- [ ] `grep -nE "presence_broadcast\(|fire_notification\(" src/daemon/scheduled.rs`
      prints nothing, and `grep -rn "fn fire_notification" src/` prints
      nothing.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.

## Test plan

In `src/daemon/scheduled_tests.rs`.

- `alert_job_notice_is_a_scheduler_warning`
- `script_job_notice_severity_follows_the_exit_code` — exit 0 → `Info`,
  exit 1 → `Error`; the title is the `` `<script>` … (exit N) `` line and
  the body holds the `<output>` block.
- `failure_notice_is_a_scheduler_error`
- `watchdog_notice_is_a_watchdog_warning` — source `Watchdog`, title
  prefix `[Watchdog] <name>: `, a multi-line response splits into title
  and body.
- `fail_job_publishes_one_error_notice` — a recording bus; `fail_job`
  publishes exactly one notice, kind `failure`, and marks the job done.
- `scheduled_job_result_is_pushed_to_attached_clients` (existing) — now
  driven through a `test_bus`; the attached chat still receives the
  `SystemMsg`.

## End-to-end verification

Not applicable — the in-process tests drive the scheduler's notify paths
through a real bus and the real sinks; the daemon-level check is in the
milestone close's live check.

## Authorizations

- May touch `src/daemon/startup.rs` (the `run_scheduled_job` call),
  `src/daemon/utils/response.rs` and its `mod.rs` re-export,
  `src/config/types.rs` (the one doc comment) and CLAUDE.md (the one row).

## Out of scope

- The ghost-job paths and `inject_ghost_event` (phase 05).
- The `scheduled_job_finished` event and the away digest (phase 08).
- Route config (phase 07).

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-11-17

The first run hit `budget_exceeded` at 1000 turns with **no source edit**:
after reading the right files it drifted through `src/shell/`,
`src/memory.rs`, `src/search.rs` and `src/daemon/situational.rs`, none of
which this phase touches. Nothing in the spec changed except the new
§ "Edit order and the files you need": a closed file list, a first-edit
deadline, and four steps that each end at a green build. Follow it. Do not
read files outside that list; if you think you need one, you don't — the
four steps are complete without it.

### Update — 2026-11-17 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** `budget_exceeded` with zero source edits is read paralysis, not a spec gap; the refinement adds the forced first edit, closed file list and stepwise edit order that phase 01 had and this doc lacked.

### Update — 2026-11-17 02:27 (started, executor)

**Executor:** phase-04 run, headless.

Flipped the phase to `in-progress`; implementing the Spec tasks in order.
