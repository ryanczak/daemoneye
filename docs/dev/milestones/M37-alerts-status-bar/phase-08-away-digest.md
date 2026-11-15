# Phase 08 — away-digest

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-15
**Depends on:** nothing in M37
**Estimated diff:** ~300 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

GitHub issue #5. A scheduled script job that fires while no chat is attached
leaves no trace the operator can read: `notify_job_result`
(`src/daemon/scheduled.rs`) sends over the live channel, finds no receiver,
and drops the message. `tmp-cast-purge-daily` ran six mornings in a row on
pinky and nothing reached any session.

PE decision (2026-11-15): do **not** queue each message. The first chat to
attach after a gap gets **one** summary of what happened while nobody was
attached: **how many times scripts ran** (per script job, succeeded and
failed) and **how many alerts fired** (per alert name). It is delivered once.
Nothing is shown when nothing happened.

## Architecture references

- `src/daemon/server/chat_presence.rs`: `handle_chat_attach` registers the
  connection, sends the catch-up brief (zero or one `SystemMsg`) and then
  `Ok`. The client's attach parser accepts exactly *one optional `SystemMsg`,
  then `Ok`*, so the digest **joins the brief into that one message**; it is
  never a second frame before `Ok`. On the last close of a session it stamps
  detach state.
- `src/daemon/session.rs`: the presence map (`presence_register`,
  `presence_unregister`, `presence_session_ids`).
- `src/daemon/scheduled.rs`: `run_scheduled_job` reads the job's record after
  the run and ends the run log with `scheduled_run_state(..)` (phase 07).
  `run_scheduled_job_inner` is allowlisted at 193 lines and **may not grow**.
- `src/daemon/utils/event_log.rs`: `for_each_event_between(from, to, f)`
  streams parsed `events.jsonl` records for a window.
- `src/webhook/process.rs`: `webhook_alert` events carry `alert_name`,
  `status`, `severity`; the record is written **before** the severity gate, so
  a below-threshold alert is in the log. `severity_rank` and the gate at
  step 5 are the rule for "reaches the operator".
- `src/config/load.rs`: `var_run_dir()`.

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **Record every script and alert run** — emit a `scheduled_job_finished`
   event (`job_name`, `action`: `"script"` or `"alert"`, `outcome`:
   `"succeeded"` or `"failed"`) once per run of an `ActionOn::Script` or
   `ActionOn::Alert` job. A `Ghost` job (it injects its own lifecycle events)
   and a deprecated `Command` job (refused, never runs) emit nothing. Emit it
   from `run_scheduled_job`, where `run_log.finish(state)` already knows the
   outcome (`RunState::Complete` is `succeeded`, any other end state is
   `failed`), **not** from `run_scheduled_job_inner` and **not** from
   `ScheduleStore::mark_done` (its unit tests hold no HOME guard). Build the
   record in a small pure function so its test needs no filesystem.
2. **The last-seen stamp** — a new module `src/daemon/away_digest.rs` (child
   of `src/daemon/mod.rs`) with `read_last_seen()` and `stamp_last_seen(now)`
   over one file, `var_run_dir()/chat_last_seen`, holding unix seconds. A
   missing, empty or unparseable file reads as `None`. The write is atomic
   (a temp file, then rename). It is a stamp, not a log: one value, replaced.
3. **The digest builder** — in the same module, `build_away_digest(from, to,
   alert_threshold) -> Option<String>`. It streams
   `for_each_event_between(Some(from), Some(to), ..)` and counts:
   - `scheduled_job_finished` with `action == "script"`: per `job_name`,
     succeeded and failed;
   - an alert fired: a `scheduled_job_finished` with `action == "alert"`
     (a watchdog alert job), **or** a `webhook_alert` whose `status` is
     `"firing"` and whose `severity` passes the same rule as the gate in
     `process_alert` step 5 (rankable, and at or above `alert_threshold`;
     an unrankable threshold behaves as `warning`). Counted per
     `alert_name` / `job_name`. A `"resolved"` alert is not a firing.

   Split the counting (a pure function over an iterator of parsed records)
   from the formatting (a pure function over the counts, the window and
   nothing else) so each tests without files. The text is one block, a header
   then at most two bullets; a bullet whose count is zero is **omitted**, and
   when both are zero the function returns `None`:

   ```
   [Away digest] While no chat was attached (2026-11-14 04:00 UTC, 7h 40m):
     • Scripts: 6 runs — tmp-cast-purge (5 ok, 1 failed), backup (0 ok, 1 failed)
     • Alerts: 3 fired — disk-high ×2, deadman ×1
   ```

   Each bullet lists at most five names, most frequent first, then
   `+N more`. The script output itself is **not** in the digest.
4. **Deliver once, to the first chat to attach** — `take_away_digest_on_attach`
   (name is the executor's), called from `handle_chat_attach` after
   `presence_register`, and its result joined to the attach brief into the one
   `SystemMsg` (separated by a blank line; alone when there is no brief):
   - it acts only when this connection is the **only live presence
     connection in the daemon**. Any other chat attached means the operator
     is already receiving live messages, and nothing is delivered or stamped;
   - no last-seen stamp (first run, or a deleted file) → write the stamp
     `now`, deliver nothing;
   - a stamp `t` → digest over `[t, now)`, then stamp `now`. The stamp is
     written **whether or not** a digest was produced, so the same window is
     never reported twice.

   File reads and writes must not run inside `with_sessions` or on a
   runtime thread that matters: build the digest the way `from_disk` is built
   in the same function, outside the lock. `handle_chat_attach` is **not**
   allowlisted and must stay within the 150-line cap, so the call site is a
   few lines and the work lives in `away_digest.rs`.
5. **Stamp when the last chat leaves** — in `handle_chat_attach`'s detach
   path, after `presence_unregister`, when no presence connection remains in
   the daemon (`presence_session_ids()` is empty), `stamp_last_seen(now)`.
   This is what bounds the window: events from while a chat *was* attached
   were seen live and are never counted. A client that never detaches cleanly
   still ends in the same path (EOF or error ends the hold).
6. **Docs** — README's scheduler or "while you were away" section (one
   paragraph: the digest, what it counts, once per gap); CLAUDE.md's key-files
   table gets an `src/daemon/away_digest.rs` row, and its `[Catch-up]`
   paragraph one sentence saying the digest rides the same attach message.
7. **Capture the evidence** — run both mutation checks and the § End-to-end
   block verbatim, and paste them into one Update Log entry.

## Constraints

- The live path does not change: `notify_job_result` still sends to
  `notify_tx`, still `presence_broadcast`s and still fires `on_alert`. A chat
  attached when a job finishes gets the message live and the job is **not**
  in a later digest (its event is older than the stamp written when that chat
  left, or the chat is still attached and nothing is delivered).
- The digest is a notice, not history: it is sent as a `SystemMsg` and is not
  written into the session's messages. (The runs themselves stay auditable in
  `var/log/runs/` and the event log, as today.)
- `run_scheduled_job_inner`'s line count may not grow; `handle_chat_attach`
  stays at or under 150 lines; no new `tests/hygiene_allowlist.txt` line.
- Tests are hermetic: `crate::test_home_guard()` plus a throwaway `$HOME` (the
  shape `scheduled_tests.rs` uses), no daemon, no clock sleeps, `now` passed
  in. A test that calls `handle_chat_attach` must keep the presence map clean
  (the map is process-global; unregister what you register).
- Do not touch the ghost lifecycle path (`inject_ghost_event`), the
  `[notifications] on_alert` hook, or `list_schedules`.

## Test plan

In `src/daemon/away_digest.rs` tests:

- `last_seen_round_trips_and_a_bad_file_reads_none`: stamp then read gives the
  value; a missing file, an empty file and `garbage` all read `None`.
- `counts_script_runs_per_job_and_outcome`: records for two script jobs, one
  with a failure, give the right per-job `(ok, failed)`.
- `counts_only_alerts_that_reach_the_operator`: a `firing` warning counts; a
  `firing` info below a `warning` threshold does **not**; a `resolved` alert
  does not; an unrankable severity does not; a `scheduled_job_finished` with
  `action: "alert"` does.
- `ignores_events_outside_the_window_and_other_event_kinds`: a record before
  `from`, a record after `to`, and a `ghost_lifecycle` record are not counted.
- `format_omits_empty_bullets_and_returns_none_when_all_zero`.
- `format_caps_each_bullet_at_five_names`: seven script names show five, then
  `+2 more`.
- `first_attach_with_no_stamp_writes_one_and_delivers_nothing`.
- `a_later_attach_delivers_the_digest_once`: events since the stamp, first
  call returns `Some` containing the counts, the stamp is now `now`, a second
  call over the same events returns `None`. This must fail if the stamp is
  not written after delivery.
- `an_attach_with_another_chat_present_delivers_nothing_and_stamps_nothing`.
- `events_older_than_the_stamp_are_not_reported`: the negative case for the
  window's lower bound.

In `src/daemon/scheduled_tests.rs`:

- `a_script_run_records_a_finished_event`: the pure record function gives
  `{action: "script", outcome: "succeeded"}` for `RunState::Complete` and
  `outcome: "failed"` for a hard fail; `Ghost` and `Command` give `None`.

In `src/daemon/server/chat_presence.rs` tests, following the existing attach
tests there:

- `attach_joins_the_digest_to_the_brief_in_one_system_msg`: with a stamp, new
  events and a catch-up brief, the reply is **one** `SystemMsg` holding both,
  then `Ok`; with no brief, one `SystemMsg` holding the digest, then `Ok`;
  with nothing to report, `Ok` alone.
- `the_last_detach_stamps_last_seen`: attach, detach, and the stamp is newer
  than before; with two chats attached, the first detach does not stamp.

## Acceptance criteria

- [ ] Every test in the test plan passes, and the existing attach and
      catch-up tests (`build_catchup_brief`, `attach_brief_for`, the
      `chat_presence` attach tests) pass unchanged.
- [ ] `grep -c 'scheduled_job_finished' src/daemon/scheduled.rs` prints at
      least 1 and `grep -c 'scheduled_job_finished' src/scheduler.rs` prints
      0 (the event is emitted from `scheduled.rs`, not from `mark_done`).
- [ ] Mutation check, pasted with its `test result:` line: remove the
      `stamp_last_seen` call that follows a delivery in task 4, confirm
      `a_later_attach_delivers_the_digest_once` fails, then revert.
- [ ] Second mutation, pasted: drop the `severity` rule from the `webhook_alert`
      count (count every `firing` alert), confirm
      `counts_only_alerts_that_reach_the_operator` fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than the baseline passed. Hygiene
      (`run_scheduled_job_inner` and `handle_chat_attach` not grown, no new
      allowlist line), `doc_truth` and `bug_tracker` are ok.

## Pre-flight

Baseline at drafting (2026-11-15, `3bfffe89`): `cargo test` passed=2984,
failed=0. Run every `cargo test` under `timeout 600`: an earlier phase's
test hung on a re-entered `test_home_guard` and nothing stopped it.
`test_home_guard` is a non-reentrant mutex; a helper that takes it must not
be called from a test that already holds it.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
timeout 600 cargo test --lib away_digest 2>&1 | grep -E '^test result'
timeout 600 cargo test --lib a_script_run_records 2>&1 | grep -E '^test result'
timeout 600 cargo test --lib attach_joins_the_digest 2>&1 | grep -E '^test result'
grep -c 'scheduled_job_finished' src/daemon/scheduled.rs | sed 's/^/emit_sites=/'
grep -c 'scheduled_job_finished' src/scheduler.rs | sed 's/^/mark_done_sites=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- New file `src/daemon/away_digest.rs` and its `mod` line in
  `src/daemon/mod.rs`.
- Edit `src/daemon/scheduled.rs`, `src/daemon/scheduled_tests.rs`,
  `src/daemon/server/chat_presence.rs`, `README.md`, `CLAUDE.md`.
- Read `severity_rank` from `src/webhook/process.rs`; widen it to
  `pub(crate)` only if it is not already reachable from `away_digest.rs`.

## Out of scope

- Delivering a script's **output** to the operator, or queueing individual
  messages (declined: the digest is counts only; the output stays in the run
  log and the shell cast).
- Ghost jobs (they already inject lifecycle events into the session store).
- A digest for `daemoneye ask` (one-shot, no presence) or any other client.
- `[notifications] on_alert` semantics and its blast radius (`on_alert` fires
  for every job result); that is a documentation point for issue #5's
  stopgap, not part of this phase.
- Per-job timezones (GitHub #4).

## Update Log

<!-- entries below this line -->
