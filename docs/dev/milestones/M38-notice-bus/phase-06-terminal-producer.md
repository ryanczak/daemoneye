# Phase 06 — terminal-producer

**Milestone:** M38 — The notice bus
**Status:** review
**Drafted:** 2026-11-17
**Depends on:** phase-05
**Estimated diff:** ~300 lines, more than half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

A program in a daemon PTY shell that reports `blocked`, `error` or `done`
over OSC 7501 publishes one `Shell` notice per transition into that state.
The daemon learns of program status only when something asks (`list_shells`,
`top`, `watch_shell`), so this phase adds a small supervised tick that
probes each live shell, diffs against its own baseline, and publishes. With
default routes a `Shell` notice reaches no sink, only the journal. That is
2.9.0 parity: 2.9.0 delivered program transitions to no chat. Phase 07's
route config is how a user sends them somewhere, and phase 10's `daemoneye
notices` shows them.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design.
- `src/shell/program_status.rs`: `State`, `Kind`, `Record`.
- `src/daemon/executor/knowledge/shell_watch.rs`: `program_check` and the
  `already_reported` rule. A record is news when its `(id, state)` pair was
  not in the baseline, which is the same rule this phase uses.
- `src/daemon/executor/knowledge/shell_screen.rs`: `program_line` (masked,
  control-escaped, one line).
- `src/daemon/startup.rs`: `spawn_session_counters_task` /
  `session_counters_loop`, the supervised-tick pattern to copy.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

A shell host answers `crate::shell::status(&socket_path(id), timeout)` with
`ShellResponse::Status { programs, .. }`: the OSC 7501 records its
terminal holds. Nothing in the daemon polls this on its own, and nothing
publishes a notice about it. `default_routes` (`notice_sinks.rs`) names no
`Source::Shell` filter.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Everything this phase needs is in these files; **read no others**:

- `src/daemon/program_notices.rs` (new) and `src/daemon/mod.rs` (one `mod`
  line): the work.
- `src/daemon/startup.rs`: the spawn.
- `src/shell/program_status.rs`, `src/shell/registry.rs` (`Owner`,
  `ShellRecord`, `with_shells`), `src/bus/notice.rs`: read once.
- `src/daemon/executor/knowledge/shell_watch.rs` and `shell_screen.rs`:
  read `program_check` and `program_line` once; do not change them.
- `CLAUDE.md`: one Key-files row.

Do the edits in this order. Each step ends at a green `cargo build --tests`.

1. Create the module with `ProgramWatcher` and its tests (Spec task 1).
2. Add `program_notice` and `collect_notices` and their tests (Spec tasks
   2–3).
3. Add the tick and the spawn (Spec task 4), the CLAUDE.md row, then run the
   gates once (Spec task 5).

## Spec

1. **`ProgramWatcher`** (in `src/daemon/program_notices.rs`): the tick's own
   baseline, keyed by shell id, holding each shell's last-seen `(id, state)`
   pairs. It never reads or writes `ShellRecord::last_programs`.
   - `observe(&mut self, shell_id: &str, records: &[Record]) -> Vec<Record>`
     returns the records whose state is `Blocked`, `Done` or `Error` and
     whose `(id, state)` pair was not in that shell's baseline, then stores
     the new pairs as the baseline.
   - **The first observation of a shell is baseline only.** It returns
     nothing, even for a record already `blocked` (a daemon restart must not
     replay old states).
   - `Idle` and `Working` are never returned.
   - `retain(&mut self, live: &HashSet<String>)` drops shells not in `live`.
     A shell seen again after that is a first observation.

2. **`program_notice(shell_id: &str, owner: &Owner, record: &Record) ->
   Notice`**: pure. Source `Shell`. Severity `Warning` for `Blocked`,
   `Error` for `Error`, `Info` for `Done`. `name` is the record's `app`, or
   the shell id when there is none. `source_id` is the shell id. Title
   `"[Shell <shell_id>] "` + `program_line(record)`, with an empty body.
   Labels: `state`, `kind` (only when `Some`), `program_id` (only when
   `record.id` is `Some`). `session` is the chat's session id for
   `Owner::Chat(sid)`, and unset for every other owner.

3. **`collect_notices(watcher: &mut ProgramWatcher, probed: Vec<(String,
   Owner, Option<Vec<Record>>)>) -> Vec<Notice>`**: pure. For each probed
   shell with `Some(records)`, observe and build a notice per returned
   record. **A `None` probe (the host did not answer) leaves that shell's
   baseline untouched and yields nothing.** It must not be read as "no
   records", or a host hiccup would replay every held state. Afterwards,
   `retain` keeps only the probed ids.

4. **The tick**: `async fn program_notices_tick(reg: &ShellRegistry,
   watcher: &mut ProgramWatcher, bus: &'static Bus)`.
   - Collect `(id, owner)` for every record with `exited_unix == None`
     under one `with_shells`.
   - Probe them all at once with no lock held. Use `crate::shell::status`
     with a 1 s timeout each, as `refresh_programs` does; a non-`Status`
     reply or an error is `None`.
   - Call `collect_notices`, then `bus.publish` each notice.
   - In `startup.rs`, add `spawn_program_notices_task(shutdown)` beside
     `spawn_session_counters_task`, supervised as `"program-notices"`. Its
     loop owns one `ProgramWatcher` and ticks every 2 s with
     `MissedTickBehavior::Delay`, passing `crate::shell::registry()` and
     `crate::bus::global()`. Call it from `run_daemon` next to the counters
     spawn.

5. **CLAUDE.md** gets a Key-files row for `src/daemon/program_notices.rs`.
   **Gates**: `cargo fmt --all`, `cargo clippy --all-targets --all-features
   -- -D warnings`, `cargo test`. Run each **once**; a green result ends the
   task. If `cargo test` fails in a test this phase did not touch, record
   it in the Update Log and stop; do not investigate it.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes.
- [ ] `grep -n "spawn_program_notices_task" src/daemon/startup.rs` prints
      the definition and the call.
- [ ] Preservation: `grep -n "Source::Shell" src/daemon/notice_sinks.rs`
      prints nothing. Default routes still deliver no shell notices.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.

## Test plan

All in `src/daemon/program_notices.rs` (inline `#[cfg(test)]`). Build
`Record`s directly; no shell host, no tokio runtime.

- `watcher_first_sight_is_baseline`: a shell first seen with a `blocked`
  record yields nothing.
- `watcher_reports_each_entry_into_blocked_done_error`: `working` then
  `blocked` yields the blocked record. `blocked` again yields nothing.
  `done` then yields the done record. `error` for a second program id
  yields that one only.
- `watcher_never_reports_idle_or_working`.
- `watcher_forgets_a_shell_dropped_by_retain`: after `retain` without it, a
  `blocked` record on the next sight yields nothing (first sight again).
- `program_notice_maps_state_to_severity`: the three states, source
  `Shell`, `source_id`, the `state` label, and `kind` only on blocked.
- `program_notice_targets_only_a_chat_owner`: `Owner::Chat("c1")` gives
  `session == Some("c1")`; `Owner::Ghost(..)` and `Owner::Scheduler(..)`
  give `None`.
- `program_notice_name_falls_back_to_the_shell_id`: no `app` means the
  name is the shell id.
- `collect_notices_skips_an_unanswered_probe`: baseline a shell, then probe
  it `None`, then probe it with the same records. Both calls yield nothing.
  A transition after that is still reported once.

## End-to-end verification

Not applicable in this phase. The tick needs a live shell host, and the
milestone close's live check covers it: a terminal `blocked` report shows in
`daemoneye notices --follow`.

## Authorizations

- May create `src/daemon/program_notices.rs` and add its `mod` line to
  `src/daemon/mod.rs`.
- May touch `src/daemon/startup.rs` (the new spawn fn and its one call)
  and CLAUDE.md (the one row).

## Out of scope

- A default route for `Shell` notices (a delivery change; phase 07 makes
  routes configurable).
- `watch_shell`, `list_shells`, `refresh_programs` and `last_programs`:
  unchanged.
- A push frame from the shell host; this phase polls.

## Update Log

<!-- entries below this line -->

### Update — 2026-11-17 18:30 (started, executor)

Phase-06 execution started by the executor. Status flipped `todo → in-progress`
(here and in the milestone README). Plan: build `ProgramWatcher` and its tests,
then `program_notice` / `collect_notices`, then the tick and spawn, then the
CLAUDE.md row and the gates.

### Update — 2026-11-17 18:55 (executor, gates)

All five spec tasks done; the three gates ran clean, each once:

- `cargo fmt --all` — exit 0.
- `cargo clippy --all-targets --all-features -- -D warnings` — exit 0, no
  warnings.
- `cargo test` — all suites green: 2997 unit tests + every integration suite
  (`tests/hygiene.rs`: 16 passed, no new allowlist line —
  `grep -c "daemon/program_notices" tests/hygiene_allowlist.txt` prints 0).

Acceptance-criteria greps:

```
$ grep -n "spawn_program_notices_task" src/daemon/startup.rs
61:fn spawn_program_notices_task(shutdown: Arc<AtomicBool>) {
62:    crate::daemon::program_notices::spawn_program_notices_task(shutdown);
543:    spawn_program_notices_task(Arc::clone(&shutdown));
$ grep -n "Source::Shell" src/daemon/notice_sinks.rs
(no matches — exit 1; default routes still deliver no shell notices)
```

Notes for review: two small adaptations inside the phase's authorization —
the baseline uses `BTreeSet<(Option<String>, &'static str)>` (storing
`state.as_str()`) because `State` derives neither `Hash` nor `Ord`, and the
title renders through the existing
`crate::daemon::executor::knowledge::program_line` rather than a new copy.
`spawn_program_notices_task` in `startup.rs` is a one-line delegation to
`program_notices::spawn_program_notices_task` so `run_daemon`'s
hygiene-capped body grows by one line (its call site). The 2 s tick, the 1 s
probe timeout and the supervised name `"program-notices"` follow
`spawn_session_counters_task` exactly. End-to-end verification is deferred
to the milestone close's live check per the phase doc.

### Update — 2026-11-17 18:47 (complete, server-authored)

**Summary:** **Summary:** Phase 06 (terminal-producer) is implemented and committed (`fc0a72cf`). `src/daemon/program_notices.rs` adds `ProgramWatcher` (baseline of `(id, state)` pairs per shell id, first-sight-is-baseline-only, `retain` to forget dropped shells), the pure `program_notice` (Source `Shell`; Warning/Error/Info for blocked/error/done; `state`/`kind`/`program_id` labels; `session` only for `Owner::Chat`; title `[Shell <id>] <program_line>`), `collect_notices` (a `None` probe leaves the baseline untouched, then `retain` over probed ids), and the supervised 2 s tick with a 1 s probe timeout, spawned from `run_daemon` via `spawn_program_notices_task` beside the counters spawn; CLAUDE.md got the one Key-files row. All 8 phase tests pass plus the full gate set: `cargo fmt` clean, `cargo clippy --all-targets --all-features -- -D warnings` exit 0, `cargo test` all suites green (2997 unit; hygiene 16 passed with no new allowlist line), and the two acceptance greps verified (`spawn_program_notices_task` definition + call present; no `Source::Shell` in `notice_sinks.rs`).

**Notes for review:** two small adaptations inside the phase's authorization — the baseline stores `state.as_str()` in a `BTreeSet<(Option<String>, &'static str)>` because `program_status::State` derives neither `Hash` nor `Ord`, and the title reuses the existing `knowledge::program_line` rather than duplicating it; `startup.rs`'s `spawn_program_notices_task` is a one-line delegation so `run_daemon`'s hygiene-capped body grows by exactly one line. The phase status remains `in-progress` with no hand-written complete entry, per the contract; end-to-end verification is deferred to the milestone close's live check as the phase doc specifies.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.9.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.83s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_installs_no_tmux_hooks ... ok
test an_empty_session_starts_no_tmux_server ... ok
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
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-be1094999a9dca70)
kill: (376109): No such process
kill: (-376823): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-b8585d869a796627)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-6cbc19420996c806)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-3fb2c691dd962733)
     Running tests/hygiene.rs (target/debug/deps/hygiene-4f2c6668d96a79df)
     Running tests/integration.rs (target/debug/deps/integration-77b5711c91e98a6c)
     Running tests/isolation.rs (target/debug/deps/isolation-1ac695f945812242)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-92b086f9df7140f9)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-70b357fc5c8ec2c6)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -0
- `docs/dev/milestones/M38-notice-bus/README.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/phase-06-terminal-producer.md` — +43 -2
- `src/daemon/mod.rs` — +1 -0
- `src/daemon/program_notices.rs` — +355 -0
- `src/daemon/startup.rs` — +9 -0

**Commit:** fc0a72cfa2d985e8dd67c4e50612b3390dbc45f9

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
