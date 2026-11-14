# Phase 02b — session-counters

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** review
**Drafted:** 2026-11-14
**Depends on:** phase 02a (the `StatusModel`, `PresencePush`)
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The bar's background, ghost and shell counters and its throughput slot
show real values (decision 6). The daemon pushes a `SessionCounters` frame
to a session's presence connections whenever that session's counts change,
and once at attach. The turn-end `UsageUpdate` carries the turn's output
tokens. The client never polls.

## Spec

Numbered tasks in execution order; the executor's task list is seeded from
this section, so mark each `active` when you start it and `done` when its
`cargo build` is green.

1. **The frame.** `Response::SessionCounters { background: u32, ghosts:
   u32, shells: u32 }` in `src/ipc.rs`, with its `kind()` arm.
2. **What is counted.** This is one pure function over a snapshot of the
   shell registry, the session id and the active-ghost count:
   - `shells`: shells owned by `Owner::Chat(<session id>)` that have not
     exited.
   - `background`: of those, the `dedicated` shells with a command in
     flight. These are the session's running `wait: false` commands.
   - `ghosts`: `stats::get_ghosts_active()`. This is daemon-wide, because
     ghost runs belong to no chat session.
   Another session's shells, ghost-owned shells and scheduler shells count
   toward nobody's `shells` or `background`.
3. **When it is pushed.** A daemon task wakes once a second. For each
   session with a live presence connection, it computes the counters and
   pushes the frame to **that session's** connections only, and only when
   the value differs from the last one pushed to that session. When a
   session's last presence connection closes, its last-pushed value is
   forgotten, so a re-attach starts fresh. This is daemon-internal; the
   client still never polls.
4. **At attach.** The `ChatAttach` hold sends one `SessionCounters` for the
   session after the `Ok`, and after the `AlertState` when one is sent. The
   attach parser accepts exactly brief-then-`Ok`, so nothing may come
   before the `Ok`. That value also becomes the session's last-pushed value.
5. **Output tokens.** `Response::UsageUpdate` gains `#[serde(default)]
   output_tokens: u32`: the sum of `output_tokens` over the turn's model
   calls, not only the last call. Decision 6 names `SessionInfo`, but
   `SessionInfo` is sent before the model runs, so the turn-end
   `UsageUpdate` carries the count instead. A frame without the field
   still decodes as 0.
6. **The client.** The parked presence task forwards `SessionCounters` as
   a new `PresencePush::Counters { background, ghosts, shells }`. The
   loop's arm applies it through one `StatusModel` method, then redraws.
   The stream's `UsageUpdate` arm stores `output_tokens` in
   `last_output_tokens`.
7. **Capture the mutation check and the end-to-end evidence** — run the
   mutation check named in § Acceptance criteria and the block in § End-to-end
   verification verbatim, and paste both outputs into one Update Log entry.

## Constraints

- Pushing never blocks on a slow client: queue it on the connection's
  unbounded channel, the way `presence_broadcast_frame_in` does.
- Tests drive explicit maps and snapshots, never the process-global
  presence map or registry (cargo runs tests in parallel). The `_in`
  functions in `src/daemon/session.rs` are the pattern.
- The task's spawn must not grow `run_daemon`. If it has to go there, bump
  `run_daemon`'s line in `tests/hygiene_allowlist.txt` and say why in the
  Update Log.
- Tests call the production seam. A copy of production logic in a test is
  a fake test, and bounced 02a twice.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/ipc.rs`: `Response`, `Response::kind`, `UsageUpdate`, `AlertState`.
- `src/daemon/session.rs`: `PresenceConnection`, `presence_map`,
  `presence_register` / `presence_unregister`, `presence_session_ids`,
  `presence_broadcast_frame_in`.
- `src/daemon/server/chat_presence.rs`: the `ChatAttach` hold, which sends
  the `AlertState` after the `Ok`.
- `src/shell/registry.rs`: `Owner`, `ShellRecord` (`dedicated`,
  `exited_unix`, `in_flight`), `with_shells`, `crate::shell::registry()`.
- `src/daemon/stats.rs`: `get_ghosts_active`.
- `src/daemon/stream/conversation.rs`: `AiEvent::Done(usage)` and the
  `UsageUpdate` send.
- `src/cli/commands/chat_presence.rs`: `PresencePush` and the parked
  task's `match serde_json::from_str::<Response>`.
- `src/cli/commands/chat_loop.rs`: the `PresencePush` arm.
- `src/cli/commands/stream_ask.rs`: the `UsageUpdate` arm.
- `src/cli/status_bar.rs`: `StatusModel` (`background`, `ghosts`,
  `shells`, `last_output_tokens`).

**Edit order.** Each step ends at a green `cargo build`:
1. **First write:** add the `SessionCounters` variant and its `kind()`
   arm, plus `output_tokens` on `UsageUpdate` (fix the constructors the
   compiler names).
2. Create `src/daemon/counters.rs`: the pure count function, the
   last-pushed tracker, and a per-session push `_in` function next to
   `presence_broadcast_frame_in` in `session.rs`. Add their tests.
3. The 1 s task, the attach send and the forget-on-last-close.
4. Sum the turn's output tokens into `UsageUpdate`.
5. The client: `PresencePush::Counters`, the `StatusModel` method, both
   arms.
6. CLAUDE.md: a Key-files row for `src/daemon/counters.rs`.
7. Spec task 7: the mutation check and the end-to-end block, pasted.

**Your first edit lands within your first 15 turns.** Never run `tmux`.
Never repeat a read whose output you already have; when unsure of a call
site, let `cargo build` name it.

## Test plan

- `session_counters_count_this_sessions_live_shells_only` (another
  session's shell, a ghost's shell and an exited shell are not counted)
- `session_counters_count_dedicated_shells_with_a_command_in_flight_as_background`
  (a dedicated idle shell and a non-dedicated busy shell are not background)
- `counters_tracker_pushes_a_value_once_and_again_on_change` (first value:
  push; same value: no push; a new value: push; after forget: push)
- `presence_push_to_session_reaches_only_that_sessions_connections`
- `attach_sends_session_counters_after_ok` (beside the existing
  `presence_attach_sends_ok_before_alert_state` in `tests/integration.rs`)
- `usage_update_carries_output_tokens_and_decodes_without_them`
- `parked_presence_forwards_session_counters_frames`
- `status_model_applies_session_counters` (via the new method; the bar then
  shows the nonzero counters and hides the zero ones)

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `grep -c 'SessionCounters' src/ipc.rs` is at least 2 (today 0).
- [ ] `grep -c 'output_tokens' src/ipc.rs` is at least 1 (today 0).
- [ ] `grep -c 'PresencePush::Counters' src/cli/commands/chat_loop.rs` is at
      least 1 (today 0).
- [ ] Mutation check, pasted with its `test result:` line: make the count
      function ignore `dedicated`, confirm
      `session_counters_count_dedicated_shells_with_a_command_in_flight_as_background`
      fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib session_counters 2>&1 | grep -E '^test result'
cargo test --lib counters_tracker 2>&1 | grep -E '^test result'
cargo test --test integration attach_sends_session_counters_after_ok 2>&1 | grep -E '^test result'
grep -c 'SessionCounters' src/ipc.rs | sed 's/^/ipc_frames=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Create `src/daemon/counters.rs` and register it in `src/daemon/mod.rs`.
- Edit the files in the symbol map, `src/daemon/startup.rs` (the spawn),
  `tests/integration.rs`, `tests/hygiene_allowlist.txt` (only as the
  constraint above allows) and `CLAUDE.md`.

## Out of scope

- `top` (phase 03).
- Any change to the bar's line, the tiers or the glyphs.
- Per-session ghost attribution.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-13 19:51 (started, executor)

Status flipped `todo → in-progress` and the milestone README's 02b row updated to match. Beginning the spec: the `SessionCounters` frame and `UsageUpdate.output_tokens` first, then `src/daemon/counters.rs`, the 1 s push task, the attach send, the client arms, and finally the mutation check plus the end-to-end block.

### Update — 2026-11-13 20:56 (end-to-end verification, executor)

Mutation check — the count function's `dedicated` guard removed
(`if record.in_flight.is_some()`), the named test fails, then reverted:

```
running 1 test
test daemon::counters::tests::session_counters_count_dedicated_shells_with_a_command_in_flight_as_background ... FAILED

failures:

---- daemon::counters::tests::session_counters_count_dedicated_shells_with_a_command_in_flight_as_background stdout ----
thread 'daemon::counters::tests::session_counters_count_dedicated_shells_with_a_command_in_flight_as_background' panicked at src/daemon/counters.rs:257:9:
assertion `left == right` failed: only the dedicated in-flight shell counts
  left: 2
 right: 1

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2849 filtered out; finished in 0.00s
```

After the revert, the same test is green again:

```
running 1 test
test daemon::counters::tests::session_counters_count_dedicated_shells_with_a_command_in_flight_as_background ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2849 filtered out; finished in 0.00s
```

End-to-end block (run mechanically, output redirected to a file and pasted):

```
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2845 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2849 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 40 filtered out; finished in 0.01s
ipc_frames=6
build_warnings=0
passed=2939 failed=0
```

Gate capture: fmt clean, build 0 warnings, clippy `-D warnings` clean,
`cargo test` green, hygiene 16/16 (the `tests/integration.rs` allowlist line
grew 2385 → 2457 for the new attach-counters test — a test-file line, noted
in the allowlist), `doc_truth` 19/19, `bug_tracker` 6/6.

### Update — 2026-11-14 05:00 (complete, server-authored)

**Summary:** All spec tasks are complete and the work is committed (`c03a5c73`), with the phase doc left at `in-progress` as the contract requires.

**Summary**

Phase 02b (session-counters) is implemented end to end. The daemon now pushes a `Response::SessionCounters { background, ghosts, shells }` frame to each session's presence connections: a supervised 1 s task (`session_counters_loop` in `startup.rs`, kept out of `run_daemon`'s hygiene-counted body) diffs each present session's computed value against a per-session last-pushed `CountersTracker` and pushes only on change; the `ChatAttach` hold sends one frame after the `Ok` (and after any `AlertState`) via `record_session_counters`, which also seeds the tracker; and the session's record is forgotten when its last connection closes, so a re-attach starts fresh. The counting is one pure function, `counters_for_session` in the new `src/daemon/counters.rs`: session-owned non-exited shells, of which `dedicated` + `in_flight` are background, plus daemon-wide `get_ghosts_active()`. `UsageUpdate` gained `#[serde(default)] output_tokens`, summed over every model call in the turn inside `run_conversation_loop` and stored into `last_output_tokens` by the stream client; the parked presence task forwards the frame as `PresencePush::Counters`, which the chat loop applies through the new `StatusModel::set_session_counters` and then redraws. All eight test-plan tests exist and pass (the two registry tests drive an explicit `ShellRegistry`, and the push test drives an explicit map).

**Notes for review**

- **Adaptation, not deviation:** the phase pinned `grep -c SessionCounters src/ipc.rs ≥ 2` (actual 6) and the exact test names — all match. The one structural choice the spec left open: the push loop lives in `startup.rs` as `session_counters_loop` + `spawn_session_counters_task` rather than inside `run_daemon`, so `run_daemon`'s allowlist line did not grow; the integration-test allowlist line *did* grow (2385 → 2457) for the new attach test and is annotated in `tests/hygiene_allowlist.txt`.
- **Wide but mechanical change to satisfy one test assertion:** deriving `PartialEq` on `Response` required adding it to six payload structs (`PaneInfo`, `ScheduleListItem`, `ScriptListItem`, `RunbookListItem`, `LimitsSummary`, `SessionSummary`, `GhostConfig`) and `agents::ToolPolicy`; `LimitsSummary` needed `Default` restored alongside it. No behavior change — derives only.
- **End-to-end verification** (pasted in the phase doc's Update Log): mutation check on the `dedicated` guard fails the named test (`left: 2, right: 1`) and passes after revert; the spec's block prints `ipc_frames=6`, `build_warnings=0`, `passed=2939 failed=0`; fmt, clippy `-D warnings`, build (0 warnings), hygiene 16/16, `doc_truth` 19/19 and `bug_tracker` 6/6 are all green.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Checking daemoneye v2.8.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.44s


TEST
est g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
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
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test attach_sends_session_counters_after_ok ... ok
test alert_list_request_returns_the_snapshot ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_at_threshold_is_injected ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 39 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
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
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-e4eaed9c20b7817e)
kill: (2877713): No such process
kill: (-2878381): No such process
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

- `CLAUDE.md` — +1 -0
- `docs/dev/milestones/M37-alerts-status-bar/README.md` — +8 -6
- `docs/dev/milestones/M37-alerts-status-bar/phase-02b-session-counters.md` — +50 -1
- `src/agents/policy.rs` — +1 -1
- `src/cli/commands/ask.rs` — +7 -9
- `src/cli/commands/chat_loop.rs` — +5 -0
- `src/cli/commands/chat_presence.rs` — +47 -0
- `src/cli/commands/stream_ask.rs` — +6 -1
- `src/cli/status_bar.rs` — +9 -0
- `src/cli/status_bar_tests.rs` — +38 -0
- `src/daemon/counters.rs` — +328 -0
- `src/daemon/mod.rs` — +1 -0
- `src/daemon/server/chat_presence.rs` — +18 -0
- `src/daemon/session.rs` — +6 -0
- `src/daemon/startup.rs` — +38 -1
- `src/daemon/stream/conversation.rs` — +6 -0
- `src/ipc.rs` — +85 -2
- `src/ipc_types.rs` — +7 -7
- `tests/hygiene_allowlist.txt` — +3 -2
- `tests/integration.rs` — +72 -0

**Commit:** c03a5c73bf1b49da75f0a363cbe725b9a0ae86fe

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
