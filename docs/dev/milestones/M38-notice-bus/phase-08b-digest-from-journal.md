# Phase 08b — digest-from-journal

**Milestone:** M38 — The notice bus
**Status:** todo
**Drafted:** 2026-11-17
**Depends on:** phase-08a
**Estimated diff:** ~250 lines, mostly test fixtures
**Tags:** language=rust, kind=refactor, size=s

## Goal

The away digest counts script runs and alerts from the **notice journal**,
read by field. It no longer reads the `scheduled_job_finished` and
`webhook_alert` event records. Its text and its once-per-window delivery
stay exactly as they are.

## Architecture references

- `src/bus/journal.rs` `for_each_notice_between` (phase 08a).
- The producers whose notices the digest counts:
  - `src/daemon/scheduled.rs`: `script_job_notice`, `alert_job_notice`,
    `failure_notice`.
  - `src/webhook/process.rs`: the alert notice `process_alert` publishes
    (source `Webhook`, label `kind` = `alert`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Read **only** these files:

- Step 1: `src/daemon/scheduled.rs` (only `failure_notice` and its
  callers) and `src/daemon/scheduled_tests.rs` (only
  `failure_notice_is_a_scheduler_error`).
- Step 2: `src/daemon/away_digest.rs`.
- Step 3: `src/daemon/server/chat_presence.rs` (only the away-digest tests
  under `// ── M37 phase 08`), and the `away_digest.rs` row of `CLAUDE.md`.

Each step ends at a green `cargo build --tests`.

## Spec

1. **Label the failure notice** (step 1). `failure_notice` adds a label
   `action` naming the job's action: `script`, `alert`, `ghost` or
   `command`. Today a script that cannot start and a refused legacy
   command job publish identical notices, and only the first counts as a
   script run.

2. **Count notices** (step 2). The digest streams
   `for_each_notice_between` over `[stamp, now)` and counts each notice by
   field. Nothing else counts.
   - **A succeeded script run:** source `Scheduler`, label `kind` =
     `script`, label `exit_code` = `0`.
   - **A failed script run:** source `Scheduler`, `kind` = `script` with
     any other `exit_code`; **or** source `Scheduler`, `kind` = `failure`
     with `action` = `script`.
   - **An alert, counted under the notice's `name`:**
     - source `Scheduler`, `kind` = `alert`; **or**
     - source `Webhook`, status `Firing`, `kind` = `alert`, severity ≥
       `Warning`.
   - The digest's `alert_threshold` parameter goes. The alert rule is the
     fixed `warning` the webhook gate uses. `take_away_digest_on_attach`
     stops passing it.
   - The module doc comment says where the counts come from now.

3. **Fixtures** (steps 2–3). Every test that seeds the event log seeds
   notice records instead, written as `Bus::new` journals them: the
   serialized `Notice` plus `event: "notice"` and `pid`. This includes the
   chat-presence test `attach_joins_the_digest_to_the_brief_in_one_system_msg`.

**Must stay:** `format_away_digest` and its tests, the stamp functions
(`read_last_seen`, `stamp_last_seen`, `stamp_on_last_detach`) and their
tests, and the digest's text. Every assertion on digest text in the
existing tests keeps its expected string.

## Acceptance criteria

- [ ] `sed -n '/#\[cfg(test)\]/q;p' src/daemon/away_digest.rs | grep -cE 'scheduled_job_finished|webhook_alert|alert_threshold'`
      prints `0`.
- [ ] `grep -c 'label("action"' src/daemon/scheduled.rs` prints `1`.
- [ ] Preservation: `git diff 9d30499b -- src/daemon/away_digest.rs | grep -cE '^-.*fn (format_away_digest|read_last_seen|stamp_last_seen|stamp_on_last_detach)'`
      prints `0`.
- [ ] Every test in the Test plan exists and passes. `cargo clippy …
      -D warnings` exits 0.

## Test plan

Each counting test calls the digest's own counting function over `Notice`
values. **Do not restate its rules in the test.** The delivery tests call
`take_away_digest_on_attach`.

- `away_digest.rs`:
  - `counts_script_runs_per_job_and_outcome` (reworked): exit 0, 0 and 1
    for one job, plus a `failure` notice with `action` = `script` for
    another, give `(2, 1)` and `(0, 1)`.
  - `counts_only_alerts_that_reach_the_operator` (reworked): counted are
    a webhook firing `Warning` and a scheduler `kind` = `alert`. **Not**
    counted:
    - a webhook firing `Info`;
    - a webhook resolved `Warning`;
    - a webhook firing `Debug` (an unrankable raw severity);
    - a webhook analysis (`Event`, `kind` = `analysis`).
  - `ignores_notices_that_are_neither_runs_nor_alerts` (replaces
    `ignores_events_outside_the_window_and_other_event_kinds`):
    - Count nothing for a ghost, watchdog, background and shell notice,
      and a `failure` with `action` = `command`.
    - Also count nothing for a legacy `scheduled_job_finished` record fed
      through the journal reader. This proves the old records are no
      longer read.
  - `a_later_attach_delivers_the_digest_once` and
    `events_older_than_the_stamp_are_not_reported`: notice fixtures, same
    assertions.
- `scheduled_tests.rs`: `failure_notice_is_a_scheduler_error` also asserts the
  `action` label, for a script job and a command job.
- `chat_presence.rs`: `attach_joins_the_digest_to_the_brief_in_one_system_msg`
  with notice fixtures, same assertions.

## End-to-end verification

`a_later_attach_delivers_the_digest_once` reads real journal files through
`take_away_digest_on_attach`. Paste its `cargo test` line into the Update
Log.

## Authorizations

- May touch exactly the files listed in "Edit order".

## Out of scope

- The catch-up brief (08c).
- Removing the `scheduled_job_finished` or `webhook_alert` events. Other
  readers (`top`) still use them.
- **Accepted deltas:**
  - A webhook alert with severity `error` now counts, as phase 03
    accepted for the gate.
  - A script that exits 0 but whose watchdog analysis was refused now
    counts as succeeded (it was failed).
  - The first digest after an upgrade from 2.9.0 does not count events
    from before the upgrade: there are no notice records for them.

## Update Log

<!-- entries appended below this line -->
