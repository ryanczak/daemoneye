# Phase 02 — sinks

**Milestone:** M38 — The notice bus
**Status:** review
**Drafted:** 2026-11-16
**Depends on:** phase-01
**Estimated diff:** ~450 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Three of today's outputs become bus consumers: chat history, the chat
notice (tmux overlay + presence push) and the `on_alert` command. The
daemon registers them on the global bus at start, with default routes
equal to what 2.9.0 delivers. Nothing publishes yet — producers move in
phases 03–05 — so runtime behaviour does not change in this phase.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design.
- `src/bus/` (phase 01): `Bus::subscribe_inline`, `Bus::subscribe`,
  `Filter`, `Notice`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

The outputs exist as free functions the producers call directly:
`inject_into_sessions` and `notify_chat_panes` (`src/webhook/process.rs`,
re-exported from `crate::webhook`), `fire_notification`
(`src/daemon/utils/response.rs`), and the targeted history append in
`src/daemon/executor/bg_shell.rs` (`append_session_message` then
`push_event` on that one entry inside `with_sessions`). The daemon builds
its `SessionStore` in `run_daemon` (`src/daemon/startup.rs`). The webhook
threshold is `config.webhook.severity_threshold` (a string; unrankable
behaves as `warning`).

## Spec

1. **Notice text** — in `src/bus/notice.rs`, add
   `Notice::history_text(&self) -> String`: the title, or `title + "\n" +
   body` when the body is non-empty; and `Notice::line(&self) -> &str`:
   the title's first line. Tests `notice_history_text_joins_title_and_body`
   (both cases) and `notice_line_is_the_titles_first_line`.

2. **Routes** — new module `src/daemon/notice_sinks.rs` (register it in
   `src/daemon/mod.rs`). `SinkRoutes { history, chat_notice, command }`,
   each a `Vec<Filter>`; a notice goes to a sink when **any** of its
   filters matches (`routes_match(&[Filter], &Notice) -> bool`; an empty
   list matches nothing). `default_routes(threshold: Severity) ->
   SinkRoutes`, reproducing 2.9.0:
   - history: webhook ≥ threshold; ghost (any); background (any).
   - chat_notice: webhook ≥ threshold; ghost; scheduler; watchdog.
   - command: webhook ≥ threshold; scheduler; watchdog.
   `threshold_from_config(&str) -> Severity` is `Severity::parse`, falling
   back to `Warning`.

3. **History sink (inline)** — delivers `Message { role: "user", content:
   notice.history_text(), .. }`. `session: None` → the same set
   `inject_into_sessions` reaches (call it). `session: Some(id)` → that
   session only: `append_session_message(id, ..)`, then `push_event` on
   its entry if resident — the `bg_shell.rs` sequence. Doc comment
   constraint: **a producer must not publish while holding the
   `SessionStore` lock** — this inline sink takes it, and the bus lock is
   held across inline delivery, so that would deadlock.

4. **Chat-notice sink (queued)** — a spawned task drains its receiver and,
   per notice, runs `notify_chat_panes(&sessions, notice.line())` through
   `crate::process::off_runtime`, as `inject_ghost_event` does.

5. **Command sink (queued)** — a spawned task drains its receiver; per
   notice it reads `config::current().notifications.on_alert` (so a reload
   applies), does nothing when it is empty, and otherwise spawns `sh -c
   <cmd>` like `fire_notification`, with env `DAEMONEYE_JOB` = `name`,
   `DAEMONEYE_MSG` = `history_text()`, plus new `DAEMONEYE_SEVERITY` and
   `DAEMONEYE_SOURCE` (lowercase names). Put the env pairs in a pure
   `command_env(&Notice) -> Vec<(&'static str, String)>`.

6. **Registration** — `register_default_sinks(bus: &Bus, sessions:
   SessionStore, routes: SinkRoutes)` subscribes all three (each queued
   sink with capacity 256, subscriber names `history`, `chat_notice`,
   `command`) and spawns the two tasks. `run_daemon` calls it once, on
   `crate::bus::global()`, right after the `SessionStore` is created, with
   `default_routes(threshold_from_config(..))` from `config::current()`.
   No existing call site of `inject_into_sessions`, `notify_chat_panes` or
   `fire_notification` changes.

7. **Docs** — add a `src/daemon/notice_sinks.rs` row to CLAUDE.md's Key
   files table.

8. **Gates** — `cargo fmt --all`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test`. Run each **once** when
   your edits are done; a green result ends the task — do not re-run it.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.
- [ ] `grep -n "register_default_sinks" src/daemon/startup.rs` shows the
      one call.
- [ ] Preservation: `git diff --stat` shows no change to
      `src/webhook/process.rs`, `src/daemon/scheduled.rs`,
      `src/daemon/executor/bg_shell.rs` or `src/daemon/utils/response.rs`.

## Test plan

In `src/daemon/notice_sinks.rs` unless noted. Each test builds its own
`Bus::with_journal(|_| {})` — never the global bus. Tests touching
session files take `crate::test_home()` first.

- `notice_history_text_joins_title_and_body`, `notice_line_is_the_titles_first_line` (`src/bus/notice.rs`).
- `routes_match_is_any_of_and_empty_matches_nothing`
- `default_routes_reproduce_2_9_0` — with threshold `warning`, assert
  per sink: webhook `info` → no sink; webhook `warning` → all three;
  ghost `debug` → history and chat_notice, not command; background →
  history only; scheduler and watchdog → chat_notice and command, not
  history.
- `threshold_from_config_falls_back_to_warning` — `"critical"` →
  Critical; `"bogus"` and `""` → Warning.
- `history_sink_broadcast_reaches_every_resident_session` — two resident
  entries, a `session: None` notice: both gain one message whose content
  is the notice's `history_text()`.
- `history_sink_targeted_reaches_only_its_session` — two resident
  entries, `session: Some(a)`: `a` gains it, `b` does not; `a`'s session
  JSONL file holds it.
- `history_sink_skips_unrouted_notice` — a webhook `info` notice under
  the default routes leaves both entries unchanged.
- `command_env_names_the_notice` — the four keys and values.
- `command_sink_runs_on_alert_with_notice_env` (`#[tokio::test]`) —
  under `crate::test_home()`, write the temp HOME's `config.toml` with an
  `[notifications] on_alert` that writes `$DAEMONEYE_SEVERITY
  $DAEMONEYE_SOURCE` into a file in the temp HOME (uninstalled,
  `config::current()` reads that file — **never** call
  `config::handle::install` in a test); publish a scheduler `error`
  notice; poll the file up to 5 s; it reads `error scheduler`.

## End-to-end verification

Not applicable — phase ships no reachable runtime behaviour: the sinks are
registered but no producer publishes until phase 03.

## Authorizations

- May touch `CLAUDE.md` (Key files row only) and `src/daemon/startup.rs`
  (the one registration call).

## Out of scope

- Changing any producer, or removing the free functions the sinks call.
- The alert registry (moves with the webhook producer, phase 03).
- `[[notifications.route]]` config and removing the old keys (phase 07).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-16 22:21 (started, executor)

**Executor:** phase-02 run, headless.

Flipped the phase to `in-progress`; implementing the Spec tasks in order.

### Update — 2026-11-16 22:58 (complete, server-authored)

**Summary:** **Summary**

Phase 02 (sinks) is implemented and committed (`0cf8f3b9`). `src/bus/notice.rs` gained `history_text()` and `line()`. The new `src/daemon/notice_sinks.rs` holds `SinkRoutes`, `routes_match`, `default_routes(threshold)` (reproducing 2.9.0 delivery), `threshold_from_config`, `command_env`, and `register_default_sinks` — the inline history sink (broadcast via `inject_into_sessions`, targeted via `append_session_message` + resident `push_event`, with the doc-commented no-publish-under-`SessionStore`-lock constraint), plus the queued chat-notice and command sinks (capacity 256, subscriber names `history`/`chat_notice`/`command`). `run_daemon` calls it once on `crate::bus::global()` right after the `SessionStore` is created; CLAUDE.md gained the Key-files row. No producer, and no file in the preservation set, was touched.

**Notes for review**

- All 10 spec tests exist and pass, including `command_sink_runs_on_alert_with_notice_env` (writes `error scheduler` via the real `sh -c` path against the uninstalled `config::current()`). Gates: `cargo fmt`/`build`/`clippy -D warnings` clean; full `cargo test` 3075 passed / 0 failed (lib 2975, hygiene 16 including no new allowlist line); `grep -n register_default_sinks src/daemon/startup.rs` shows the one call; `git diff` against the four preservation files is empty. End-to-end verification is N/A per the phase doc (no producers yet).
- Two adaptations worth knowing: the spec's `notice_history_text_joins_title_and_body` sketch assumed `Notice::new`'s 4th arg was the body — it's the title (`name, title`), so the test builds titles through `new(.., title)` and exercises the empty-body case directly; and `history_sink_targeted_reaches_only_its_session` uses `Source::Ghost` rather than an unroutable source, since the inline sink runs behind the route filter and default routes deliver ghost notices to history (behavior the test plan itself pins for broadcast). To keep `run_daemon` inside its allowlisted 622-line hygiene cap, the existing pipe-log/stop-sentinel sweep moved verbatim into `sweep_stale_var_run_artifacts()` in the same file — a pure extraction, no behavior change.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
spawn_ghost_shell_with_agent_merge ... ok
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
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 40 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

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
kill: (127039): No such process
kill: (-127701): No such process
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
- `docs/dev/milestones/M38-notice-bus/phase-02-sinks.md` — +7 -1
- `src/bus/notice.rs` — +47 -0
- `src/daemon/mod.rs` — +1 -0
- `src/daemon/notice_sinks.rs` — +491 -0
- `src/daemon/startup.rs` — +38 -23

**Commit:** 0cf8f3b98da6ee0cf9701b863729d6a056399889

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
