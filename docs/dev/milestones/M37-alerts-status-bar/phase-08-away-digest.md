# Phase 08 — away-digest

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** review
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

### Update — 2026-11-15 03:30 (started)

Started phase 08 (away-digest). Status flipped `todo` → `in-progress` (phase doc and
milestone README table). Baseline confirmed: `cargo test` passed=2984, failed=0 at
`3bfffe89`-equivalent tree. Executor: GLM-5.3-Flash (headless run).

### Update — 2026-11-15 (escalation)

**Chosen lever:** resume
**Rationale:** the run ended `hard_fail` (`IdenticalToolCallRepetition`, 6×, turn 445) re-running `cargo build | grep -v 'never used'` filters. The 16 warnings are dead code only because `take_away_digest_on_attach` and `stamp_on_last_detach` are not yet called from `handle_chat_attach`; wiring them clears every warning. Tasks 1–3 and the module's task 4/5 functions are on disk and the design is right. The architect's gate run found two `E0308` compile errors in the module's tests (an `i64`/`u64` mismatch at `from_timestamp`, near `away_digest.rs:544`). It also found one logic gap: the only-presence check counts `presence_session_ids()`, but the spec requires the only live **connection**. Still to do: the two compile errors, the connection count, fmt, the wiring in `chat_presence.rs`, the two `chat_presence` tests and the `scheduled_tests.rs` test, the docs and the evidence.

### Update — 2026-11-15 (end-to-end verification)

Resumed run completed the remaining tasks: fixed the two test `E0308`s, replaced the
session-count gate with a live-connection count (sum of `Vec::len` over the presence
map), wired `take_away_digest_on_attach` after the `from_disk` insert and
`stamp_on_last_detach` after `presence_unregister` in `handle_chat_attach` (136 lines,
cap 150) with the join in a `join_brief_and_digest` helper, added
`a_script_run_records_a_finished_event` (`scheduled_tests.rs`),
`attach_joins_the_digest_to_the_brief_in_one_system_msg` and
`the_last_detach_stamps_last_seen` (`chat_presence.rs`), and wrote the README and
CLAUDE.md docs. Three pre-existing test bugs surfaced on the first green compile and
were fixed before evidence capture: the `ignores_events_outside…` record used
`action: "script"` with no `outcome` (a shape the emitter never writes — now
`action: "command"`, matching its assertion), the `log_event_helper` wrote pinned-2023
records into today's segment so the `[2023-11-14, 2023-11-15)` window never read them
(now the helper picks the segment from the record's own `ts`, exactly how production's
records land), and `an_attach_with_another_chat_present…` registered only the *other*
chat so the gate saw one connection and delivered (now it registers the attaching
connection too, production order). Mutation checks, run then reverted:

Mutation 1 — the post-delivery `stamp_last_seen(now)` removed from
`take_away_digest_on_attach`:

```
test daemon::away_digest::tests::a_later_attach_delivers_the_digest_once ... FAILED
thread '…' panicked at src/daemon/away_digest.rs:571:9:
assertion `left == right` failed: the stamp is now, whether or not a digest was produced
  left: Some(1700000000)
 right: Some(1700027600)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2907 filtered out; finished in 0.00s
```

Mutation 2 — the `severity` rule dropped from the `webhook_alert` count (every
`firing` alert counts):

```
test daemon::away_digest::tests::counts_only_alerts_that_reach_the_operator ... FAILED
thread '…' panicked at src/daemon/away_digest.rs:427:9:
a firing info below a warning threshold does not
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2907 filtered out; finished in 0.00s
```

End-to-end block, verbatim:

```
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 2897 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2907 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2907 filtered out; finished in 0.01s
emit_sites=2
mark_done_sites=0
build_warnings=0
passed=2998 failed=0
```

Both mutations reverted and re-run green before the block. Hygiene: no
`tests/hygiene_allowlist.txt` change (`run_scheduled_job_inner` still allowlisted at
193, body untouched); `handle_chat_attach` is 136 lines. `cargo test --test hygiene`
16 passed, `--test doc_truth` 19 passed, `--test bug_tracker` 6 passed, build 0
warnings, fmt clean, clippy `-D warnings` clean.

### Update — 2026-11-15 04:36 (complete, server-authored)

**Summary:** **Summary**

Phase 08 (away-digest) is complete and committed (`4a2cbd7e`); working tree clean. Built on the prior run's on-disk work: fixed the two test `E0308`s, replaced the session-count gate with a live-**connection** count (sum of `Vec::len` over the presence map, acting only when the total is exactly 1), and wired `take_away_digest_on_attach` after the `from_disk` insert plus `stamp_on_last_detach` after `presence_unregister` in `handle_chat_attach` (136 lines, cap 150) with the brief/digest join in a small `join_brief_and_digest` helper. Added the three missing tests (`a_script_run_records_a_finished_event`, `attach_joins_the_digest_to_the_brief_in_one_system_msg`, `the_last_detach_stamps_last_seen`) and the README/CLAUDE.md docs.

**Notes for review**

- Three latent test bugs surfaced on first green compile and were fixed rather than worked around: the `ignores_events_outside…` record used `action: "script"` with no `outcome` (a shape the emitter never writes — now `action: "command"`), the test event helper wrote pinned-2023 records into today's segment so the window never read them (now the helper picks the segment from the record's own `ts`), and `an_attach_with_another_chat_present…` only registered the *other* chat so the gate saw one connection and delivered (now it registers the attaching connection too, matching production order).
- Both mutation checks ran and failed the pinned tests as required, then reverted; the end-to-end block is pasted verbatim in the phase doc's end-to-end Update Log entry: `emit_sites=2`, `mark_done_sites=0`, `build_warnings=0`, `passed=2998 failed=0`, plus hygiene (16), doc_truth (19) and bug_tracker (6) all green; fmt and clippy `-D warnings` clean; no allowlist change.
- `cargo test` count is 2998 vs the 2984 baseline (+14 new tests), all other suites unchanged.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Checking daemoneye v2.8.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.08s


TEST
ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test event_log_entry_format ... ok
test attach_sends_session_counters_after_ok ... ok
test alert_list_request_returns_the_snapshot ... ok
test g4_briefing_injects_on_next_run ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 40 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.27s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-e4eaed9c20b7817e)
kill: (3605608): No such process
kill: (-3606273): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-84596a3046ce91b6)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-61684e0b51107e5b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c69cc98b7b8dfe7a)
     Running tests/hygiene.rs (target/debug/deps/hygiene-1cb80d9fbff8faf7)
     Running tests/integration.rs (target/debug/deps/integration-9d0880a1c76a698b)
     Running tests/isolation.rs (target/debug/deps/isolation-882adad6d3253c89)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-bad9d4ccd89b2daf)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-3f0be99859fd1842)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +2 -1
- `README.md` — +1 -0
- `docs/dev/milestones/M37-alerts-status-bar/phase-08-away-digest.md` — +60 -0
- `src/daemon/away_digest.rs` — +56 -35
- `src/daemon/scheduled_tests.rs` — +66 -0
- `src/daemon/server/chat_presence.rs` — +146 -2

**Commit:** 4a2cbd7e5810b3f0083888706bff437b69b8c5ae

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
