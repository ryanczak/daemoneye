# Phase 05 — ghost-background-producers

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-17
**Depends on:** phase-04
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=refactor, size=m

## Goal

Ghost lifecycle events and background-command completions publish notices
instead of writing into session history and the chat overlay themselves.
With default routes, delivery is unchanged from 2.9.0: ghost events reach
every chat's history and the chat notice; a background completion reaches
its own session's history only. Also fixes the `ScheduleStore::new_empty`
temp-file race that cost phase 04 ~800 turns.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design and § Notes —
  read the `test_bus` / `rt.enter()` gotcha before touching any test.
- `src/daemon/notice_sinks.rs`: `default_routes` (Ghost → history +
  chat_notice; Background → history only), `deliver_history` (a notice
  with `session: Some(id)` reaches that session only), `test_bus`.
- `src/bus/notice.rs`: `Notice::new` and its builders, `history_text()`
  (`title`, or `title\nbody`).
- Phase 04's `src/daemon/scheduled.rs` builders (`alert_job_notice`,
  `failure_notice`) are the pattern to follow.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

`inject_ghost_event(sessions, content)` (`src/webhook/process.rs`, glob
re-exported from `src/webhook/mod.rs`) calls `inject_into_sessions`, then
`notify_chat_panes` with the content's first line via `off_runtime`, then
`log_event("ghost_lifecycle", {content})`. It has 13 callers, each passing
a `[Ghost Shell Started|Completed|Failed|Skipped] …` string:
`src/webhook/process.rs` (4, `state.bus` in scope), `src/daemon/scheduled.rs`
(5, `bus` in scope since phase 04), `src/daemon/stream/conversation.rs` (2)
and `src/daemon/executor/knowledge/ghost.rs` (1) — the last two have no
bus.

`bg_shell::await_and_notify` (`src/daemon/executor/bg_shell.rs`) builds a
`[Background Task Completed]\n<completion or interrupted message>` user
message and delivers it to its own session with `append_session_message`
and `push_event`, after a locked check that returns silently when the
session entry is gone. `await_notify_and_close` wraps it. Their production
caller is `src/daemon/executor/dispatch_background.rs`; the rest are the
inline tests in `bg_shell.rs`.

`ScheduleStore::new_empty()` (`src/scheduler.rs`) names its file by
`process::id()` alone, so parallel tests share one path and a `save`
renames another test's `.tmp` away (`No such file or directory`). See
`docs/dev/TODO.md` item 2.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns** —
step 1 is small on purpose. Everything this phase needs is in these files;
**read no others**:

- `src/scheduler.rs`, `docs/dev/TODO.md` — step 1.
- `src/webhook/process.rs` — the ghost builder and its 4 callers.
- `src/daemon/scheduled.rs`, `src/daemon/stream/conversation.rs`,
  `src/daemon/executor/knowledge/ghost.rs` — the other ghost callers.
- `src/daemon/executor/bg_shell.rs`, `src/daemon/executor/dispatch_background.rs`
  — the background producer.
- `src/daemon/notice_sinks.rs`, `src/bus/notice.rs` — read once; do not
  change them.
- `CLAUDE.md` — one bullet (Spec task 5).

Do the edits in this order. Each step ends at a green `cargo build --tests`;
start the next step only then.

1. Spec task 1 (`new_empty`) and its test.
2. In `process.rs`, add `GhostEvent`, `ghost_event_notice` and
   `publish_ghost_event` beside `inject_ghost_event` (Spec task 2).
3. Move all 13 callers to `publish_ghost_event`, then delete
   `inject_ghost_event` (Spec task 3).
4. The background producer and its callers and tests (Spec task 4).
5. CLAUDE.md, the Test-plan tests, then the gates once (Spec tasks 5–6).

## Spec

1. **`new_empty` gets a distinct file per call** — the path stays in
   `std::env::temp_dir()` and still contains the pid, plus a process-wide
   `AtomicU64` counter so two calls in one process never share a path. Its
   doc comment's "unique temp file" becomes true. Delete item 2 from
   `docs/dev/TODO.md` (the `---` separator above it too).

2. **Ghost notice builder** — in `src/webhook/process.rs`:
   - `pub enum GhostEvent { Started, Completed, Failed, Skipped }`.
   - `pub fn ghost_event_notice(kind: GhostEvent, name: &str, content: &str)
     -> crate::bus::Notice` — pure. Source `Ghost`; severity `Info` for
     `Started` / `Completed`, `Error` for `Failed`, `Warning` for `Skipped`;
     `name` the alert, runbook or job name the caller has; label `kind` =
     `started` / `completed` / `failed` / `skipped`; title the content's first
     line, body the rest (use the file's existing `split_first_line`), so
     `history_text()` equals `content` byte for byte.
   - `pub fn publish_ghost_event(bus: &'static crate::bus::Bus, kind:
     GhostEvent, name: &str, content: &str)` — **not** async: publishes the
     notice, then keeps the `log_event("ghost_lifecycle", {content})` line
     unchanged (`daemoneye top` and the away digest read it until phase 08).

3. **Callers** — each `inject_ghost_event(&sessions, &format!(…)).await`
   becomes `publish_ghost_event(bus, GhostEvent::<kind>, <name>, &format!(…))`
   with the **same** format string; the kind is the one its `[Ghost Shell …]`
   prefix names. The bus: `state.bus` in `process.rs` (copy the `&'static`
   into the spawned task), the phase-04 `bus` parameter in `scheduled.rs`,
   `crate::bus::global()` in `conversation.rs` and `knowledge/ghost.rs`.
   Then delete `inject_ghost_event`. Do not change `inject_into_sessions`
   or `notify_chat_panes` — the sinks call them.

4. **Background producer** — in `bg_shell.rs`:
   - `pub(super) fn background_notice(session_id: &str, run_id: &RunId,
     shell_id: &ShellId, body: String, exit_code: Option<i32>) ->
     crate::bus::Notice` — pure. Source `Background`; severity `Info` when
     `exit_code == Some(0)`, `Error` otherwise; `name` and `source_id` the run
     id; `.session(session_id)`; title exactly `[Background Task Completed]`;
     body the completion or interrupted message; labels `shell` = the shell
     id and, when `Some`, `exit_code`. `history_text()` must equal the
     content 2.9.0 delivered.
   - `await_and_notify` and `await_notify_and_close` take `bus: &'static
     crate::bus::Bus` and publish `background_notice(…)` in place of the
     `Message` + `append_session_message` + `push_event`. `exit_code` is
     `outcome.as_ref().ok().map(|o| o.exit_code)` — an interrupted run's
     outcome is `Err`, so it is `None` and reports `Error`, as today. Keep
     the locked session-gone check before it and the `RunFinished` turn-log
     writes; **publish only after that lock is released** (the history sink
     takes the same lock).
   - `dispatch_background.rs` passes `crate::bus::global()`. Every test call
     in `bg_shell.rs` passes `notice_sinks::test_bus(sessions.clone(),
     Severity::Warning)` built inside the test's runtime, with its assertions
     unchanged.

5. **CLAUDE.md** — the Ghost Shell conventions "Lifecycle events" bullet
   names `publish_ghost_event()` and says the events are `Ghost` notices on
   the bus (default routes: history + chat notice), still mirrored to
   `ghost_lifecycle`.

6. **Gates** — `cargo fmt --all`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test`. Run each **once** when
   your edits are done; a green result ends the task — do not re-run it.
   If `cargo test` fails in a test this phase did not touch, record the
   failure in the Update Log and stop; do not investigate it.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes; the existing
      `bg_shell.rs` `await_and_notify` / `await_notify_and_close` tests pass
      with their assertions unchanged (only the bus argument is added).
- [ ] `grep -rn "inject_ghost_event" src/` prints nothing.
- [ ] `grep -nE "append_session_message|push_event" src/daemon/executor/bg_shell.rs`
      prints nothing.
- [ ] Preservation: `grep -rn '"ghost_lifecycle"' src/webhook/process.rs`
      still prints the one `log_event` line.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.
- [ ] Bug 05-1: `git diff 4df97778 -- src/daemon/scheduled.rs | grep -c '^-.*store.mark_done'`
      prints `0`: the scheduled ghost job's could-not-start branch marks the
      job done again ([bugs/bug-05-1.md](bugs/bug-05-1.md)).

## Test plan

- `src/scheduler.rs`: `new_empty_stores_never_share_a_file` — two
  `new_empty()` stores in one process have different paths, and adding a
  job to each succeeds.
- `src/webhook/process.rs`:
  - `ghost_event_notice_maps_kind_to_severity` — all four kinds: severity,
    source `Ghost`, the `kind` label.
  - `ghost_event_notice_history_text_is_the_content` — a one-line and a
    multi-line content (a `Failed` with a multi-line error) both round-trip
    through `history_text()` unchanged; the title is the first line.
  - `publish_ghost_event_reaches_every_session` — a `test_bus` over a store
    with two resident entries: both entries' messages gain one user message
    whose content is the published string.
- `src/daemon/executor/bg_shell.rs`:
  - `background_notice_targets_its_session` — `session` is the given id,
    source `Background`, title `[Background Task Completed]`, `history_text()`
    is `"[Background Task Completed]\n" + body`.
  - `background_notice_severity_follows_the_outcome` — `Some(0)` → `Info`;
    `Some(1)` and `None` → `Error`.
  - `background_completion_skips_other_sessions` — `await_and_notify` with a
    `test_bus` over a store holding the run's session and a second one: only
    the run's session gains the completion message.

## End-to-end verification

Not applicable — the in-process tests drive both producers through a real
bus and the real history sink; the daemon-level check is the milestone
close's live check (a background command landing in its chat).

## Authorizations

- May touch `src/scheduler.rs` (`new_empty` and its test) and
  `docs/dev/TODO.md` (delete item 2).
- May touch `src/daemon/executor/dispatch_background.rs` (the two spawn
  calls) and CLAUDE.md (the one bullet).

## Out of scope

- OSC 7501 terminal notices (phase 06); route config (phase 07).
- Retiring `ghost_lifecycle` and the catch-up brief's text prefixes
  (phase 08) — keep every `[Ghost Shell …]` / `[Background Task Completed]`
  string byte-identical.
- `docs/REQUIREMENTS.md` and `docs/design-reference.md` mentions of
  `inject_ghost_event` — updated at milestone close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-17 (escalation)

**Chosen lever:** resume
**Rationale:** steps 1–2 were done and the run stopped on one compile error
(E0521 at `src/webhook/process.rs:563`) after six fixes in a row: two calls
inside the `tokio::spawn` (the Completed and Failed arms) still said
`state.bus` instead of the `bus` copy made before the spawn. That was a
single wall, not a spec gap. *(Restored at review: the resumed run
overwrote this entry instead of appending below it.)*

### Update — 2026-11-17 (resumed run started)

Resuming after the escalation: fixed the E0521 by replacing the two
`state.bus` references inside the `tokio::spawn` with the pre-spawn `bus`
copy and deleting the unused `alert_name` clone. Continued with steps 3–6:
all 13 callers moved to `publish_ghost_event`, `inject_ghost_event`
deleted, the background producer rewritten around `background_notice` +
a bus publish after the session lock is released, CLAUDE.md bullet
updated, and the Test-plan tests added. Gates: fmt, clippy -D warnings
and the full `cargo test` (2989 lib + integration suites) green.

### Update — 2026-11-17 16:58 (complete, server-authored)

**Summary:** **Summary**

Resumed phase 05 from the escalation point and completed all six spec tasks. The E0521 was fixed by replacing the two `state.bus` references inside the `tokio::spawn` block with the pre-spawn `let bus = state.bus;` copy (Completed and Failed arms) and deleting the unused `alert_name` clone. Steps 3–6 followed: all 13 `inject_ghost_event` callers moved to `publish_ghost_event(bus, GhostEvent::<kind>, <name>, …)` with format strings byte-identical (4 in process.rs, 5 in scheduled.rs via the phase-04 `bus` param, 2 in stream/conversation.rs and 1 in knowledge/ghost.rs via `crate::bus::global()`), then `inject_ghost_event` deleted. The background producer now builds the pure `background_notice` (Source `Background`, `session` targeted, title `[Background Task Completed]`, labels `shell`/`exit_code`) and `await_and_notify`/`await_notify_and_close` take a `&'static Bus` and publish after the session-gone lock check is released; `dispatch_background.rs` passes `crate::bus::global()`. `new_empty` gained the AtomicU64 suffix (step 1 was already on disk), TODO.md item 2 is deleted, and the CLAUDE.md Lifecycle-events bullet names `publish_ghost_event()`.

**Notes for review**

- All Test-plan tests exist and pass: `new_empty_stores_never_share_a_file`, `ghost_event_notice_maps_kind_to_severity`, `ghost_event_notice_history_text_is_the_content`, `publish_ghost_event_reaches_every_session`, `background_notice_targets_its_session`, `background_notice_severity_follows_the_outcome`, `background_completion_skips_other_sessions`. The 8 pre-existing `bg_shell` `await_and_notify`/`await_notify_and_close` tests pass with assertions unchanged — only the `notice_sinks::test_bus(...)` argument was added.
- Acceptance greps verified: `grep -rn "inject_ghost_event" src/` → nothing; `grep -nE "append_session_message|push_event" src/daemon/executor/bg_shell.rs` → nothing; `grep -rn '"ghost_lifecycle"' src/webhook/process.rs` → the one `log_event` line (process.rs:301). `tests/hygiene.rs` passes with no allowlist change.
- Gates: `cargo fmt --all` clean, `cargo clippy --all-targets --all-features -- -D warnings` exits 0 (one clippy fix along the way: `get().is_none() ||` → `map().eq()` in a test), full `cargo test` green — 2989 lib tests plus all integration suites, 0 failures, including the previously flaky `daemon::scheduled` set.
- One adaptation: the phase-04-era comment on `await_and_notify` described a four-phase lock discipline; the rewrite keeps the locked session-gone check and moves the publish into the unlocked section, so the doc comment now states the don't-publish-under-the-lock rule directly. Status left at `in-progress` per the lifecycle split; committed as `28721c7a` with a clean tree.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.9.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.92s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test alert_list_request_returns_the_snapshot ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
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
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


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

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-be1094999a9dca70)
kill: (308284): No such process
kill: (-308976): No such process
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

- `CLAUDE.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/README.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/phase-05-ghost-background-producers.md` — +10 -8
- `src/daemon/executor/bg_shell.rs` — +223 -34
- `src/daemon/executor/dispatch_background.rs` — +2 -0
- `src/daemon/executor/knowledge/ghost.rs` — +6 -5
- `src/daemon/scheduled.rs` — +26 -22
- `src/daemon/stream/conversation.rs` — +10 -8
- `src/scheduler.rs` — +43 -2
- `src/webhook/process.rs` — +143 -30

**Commit:** 28721c7a678af469a98e5437a97b7908c4e7245c

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Notes for executor — 2026-11-17 (bounce, bug 05-1)

**Green gates and a clean tree are expected here and are NOT evidence the
phase is done.** Everything else in this phase is reviewed and approved —
do not touch `webhook/process.rs`, `bg_shell.rs`, `scheduler.rs` or any
test. **There is exactly one edit left**, in `src/daemon/scheduled.rs`:
in `run_scheduled_job_inner`'s `ActionOn::Ghost` arm, the `Err(e)` branch
of `GhostManager::start_session_with_config` builds `msg`, logs it, and
publishes `[Ghost Shell Failed] … could not start …`. Add
`store.mark_done(&job.id, false, Some(msg));` after that
`publish_ghost_event(…);` call. This puts back the line phase 05 dropped.
Finish condition: `git diff 4df97778 -- src/daemon/scheduled.rs | grep -c
'^-.*store.mark_done'` prints `0`, and `cargo test` reports 2989 lib tests
passed, **not** 2990.

