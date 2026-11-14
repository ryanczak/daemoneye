# Phase 07 — scheduler-recovery

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-14
**Depends on:** nothing in M37
**Estimated diff:** ~200 lines, about two thirds of it tests
**Tags:** language=rust, kind=bugfix, size=m

## Goal

GitHub issue #9, plus the sibling bug found while triaging it. Two ways a
recurring scheduled job dies silently today, because `take_due()`
(`src/scheduler.rs`) dispatches only `Pending` jobs:

1. **Restart mid-run.** `take_due()` persists `Running` and leaves
   `next_run` on the fired slot. A daemon restart before `mark_done()`
   leaves the job `Running` forever; nothing reconciles it at startup.
   `deadman-watchdog` on pinky skipped six days this way.
2. **One failed run.** `mark_done(.., false, ..)` sets `Failed` for every
   kind of job, so a `Cron` or `Every` job that fails once (a ghost
   concurrency skip, a refused script) never fires again.

PE decisions (2026-11-14): a recurring job left `Running` by a restart
**skips to its next future slot** — the interrupted run is not repeated; and
a recurring job's failed run **does not end the job**.

## Architecture references

- `src/scheduler.rs`: `ScheduleStore::{load_or_create, take_due, mark_done}`,
  `ScheduleKind::{advance, skip_to_future}`, `JobStatus`.
- `src/daemon/startup.rs`: `run_daemon` loads the store (`load_or_create`),
  then starts the scheduler task. Allowlisted function; its count may not grow.
- `src/daemon/scheduled.rs`: `run_scheduled_job` reads the job's status
  after the run and maps it to the run log's end state
  (`scheduled_terminal_state`, `scheduled_run_state`).

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **Startup recovery** — `ScheduleStore::recover_interrupted(&self) ->
   Vec<String>` (the recovered job names). For every job whose status is
   `Running`: a `Cron` or `Every` job advances to its next **future** slot
   (`advance` then `skip_to_future`, as `mark_done` does) and becomes
   `Pending`; a `Once` job becomes `Failed("daemon restarted mid-run")`.
   Any other status is untouched. Persist once if anything changed. Each
   recovered job logs a `warn` naming it and the outcome, and writes a
   `scheduler_job_recovered` event (`job`, `kind`, `outcome`:
   `"skipped_to_next_slot"` or `"failed"`).
2. **Call it from the daemon only** — directly after the store loads in
   `run_daemon`, through a small helper so `run_daemon` does not grow.
   **Never from `load_or_create`**: the CLI (`src/cli/local_cmds.rs`) loads the
   store too, possibly while a live daemon is running a job, and a reset
   there would double-fire it.
3. **A failed recurring run keeps the job alive** — in `mark_done`, an
   unsuccessful run of a `Cron` or `Every` job advances to the next future
   slot and returns to `Pending`, exactly as a successful one does; the
   `warn` log carries the error text. A `Once` job still ends `Failed`.
   `Cancelled` stays terminal.
4. **The failure must stay visible to the run log** — today
   `run_scheduled_job` reads the status after the run, and `Failed` maps to a
   `hard_fail` end state with the error as the blocker. A recurring job's
   failed run now reads back `Pending`, which maps to `Complete`. That would
   turn every failed recurring run into a reported success. Keep the failed
   run's `hard_fail` end state and its error text. How the failure reaches
   `run_scheduled_job` is the executor's choice (for example `mark_done`
   returning the status it wrote alongside the outcome, or a
   `last_error: Option<String>` field on `ScheduledJob` with
   `#[serde(default)]` so existing `schedules.json` files load unchanged).
   The `[Ghost Shell Skipped]` and `fail_job` paths must end as they do
   today.
5. **Docs** — README's scheduler section (what a restart does to a running
   recurring job, and that a failed recurring run no longer ends the job);
   CLAUDE.md's `src/scheduler.rs` row if it states `Failed` semantics.
6. **Capture the evidence** — run the mutation check and the § End-to-end
   block verbatim, and paste both into one Update Log entry.

## Constraints

- The skip policy matches `mark_done`'s existing catch-up rule; no new
  catch-up behaviour is introduced. A recovered `Every`/`Cron` job never
  fires on the startup tick unless its next slot genuinely is due.
- `recover_interrupted` is idempotent: a second call finds nothing `Running`
  and changes nothing.
- A `Pending` job whose `next_run` is already past (the daemon was down
  across a slot) is **not** recovery's business; `take_due()` handles it as
  today.
- Tests are hermetic: a store at a throwaway path (the existing `tmp_store`
  shape), no daemon, no clock sleeps; build `next_run` values from
  `Utc::now()` offsets.

## Test plan

In `src/scheduler.rs` tests, following `take_due_persists_running_state` and
`mark_done_skips_past_intervals_to_prevent_catchup`:

- `recover_interrupted_skips_a_running_every_job_to_its_next_slot`: a job
  taken `Running` by `take_due`, reloaded from disk, recovered: status
  `Pending`, `next_run` in the future, not equal to the fired slot, and the
  returned names list it.
- `recover_interrupted_skips_a_running_cron_job_to_its_next_slot`: the same
  for a `Cron` kind.
- `recover_interrupted_fails_a_running_once_job`: status
  `Failed("daemon restarted mid-run")`.
- `recover_interrupted_leaves_other_statuses_alone`: `Pending`, `Succeeded`,
  `Failed(_)` and `Cancelled` jobs are unchanged and the returned list is
  empty; and calling it twice changes nothing the second time.
- `a_recovered_job_is_not_due_on_the_next_tick`: after recovery,
  `take_due()` returns nothing.
- `mark_done_failure_keeps_a_recurring_job_pending`: `Every` and `Cron`
  jobs, `mark_done(id, false, Some("boom"))`, status `Pending` with
  `next_run` in the future; a following `take_due()` is empty until the
  slot, and a due one fires again.
- `mark_done_failure_ends_a_once_job_failed`: `Failed("boom")`, unchanged.

In `src/daemon/scheduled_tests.rs`, following
`scheduled_state_pending_is_complete_because_every_jobs_reschedule`:

- `a_failed_recurring_run_still_ends_hard_fail_with_its_error`: whichever
  seam task 4 chose, a recurring job whose run failed with `"boom"` ends
  `hard_fail` with `"boom"` as the blocker, and a recurring job whose run
  succeeded still ends `Complete`. This must fail against a tree where the
  failed run reads back `Pending` and maps to `Complete`.

## Acceptance criteria

- [ ] Every test in the test plan passes, and the existing
      `take_due_persists_running_state` and
      `store_mark_done_success_reschedules_every` pass unchanged.
- [ ] `grep -c 'recover_interrupted' src/scheduler.rs` prints at least 2
      (today 0), and `grep -c 'recover_interrupted' src/daemon/startup.rs
      src/cli/local_cmds.rs` prints a non-zero count for `startup.rs` and 0
      for `local_cmds.rs`.
- [ ] Mutation check, pasted with its `test result:` line: remove the
      `skip_to_future` call from the `Every` arm of `recover_interrupted`,
      confirm `recover_interrupted_skips_a_running_every_job_to_its_next_slot`
      fails, then revert.
- [ ] Second mutation, pasted: make `mark_done` return the recurring job to
      `Pending` **without** the failure reaching the run state, confirm
      `a_failed_recurring_run_still_ends_hard_fail_with_its_error` fails,
      then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than the baseline passed. Hygiene (`run_daemon`'s
      count not grown), `doc_truth` and `bug_tracker` are ok.

## Pre-flight

Baseline at drafting (2026-11-14, `d1178753`): `cargo test` passed=2975,
failed=0.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib recover_interrupted 2>&1 | grep -E '^test result'
cargo test --lib mark_done_failure 2>&1 | grep -E '^test result'
cargo test --lib a_failed_recurring_run 2>&1 | grep -E '^test result'
grep -c 'recover_interrupted' src/daemon/startup.rs | sed 's/^/startup_calls=/'
grep -c 'recover_interrupted' src/cli/local_cmds.rs | sed 's/^/cli_calls=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit `src/scheduler.rs`, `src/daemon/scheduled.rs`,
  `src/daemon/scheduled_tests.rs`, `src/daemon/startup.rs` (the one helper
  and its call), `README.md`, `CLAUDE.md`.

## Out of scope

- `list_schedules` surfacing a long-`Running` job (after startup recovery a
  stuck `Running` needs a genuinely hung run, which is a different problem).
- Delivery of script-job results to a disconnected operator (GitHub #5) and
  per-job timezones (GitHub #4).
- Re-firing the interrupted run (declined; the job skips to its next slot).
- Any change to `Cancelled`, `Once` success handling or the cron parser.

## Update Log

<!-- entries below this line -->
