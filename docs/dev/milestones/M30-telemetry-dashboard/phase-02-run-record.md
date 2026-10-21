# Phase 02: run-record — one `run` event per finished run; `failure_class` from the tally

**Milestone:** M30 — Telemetry & dashboard
**Status:** done
**Depends on:** phase-01
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 01 gave every run a `RunMetrics` tally on its `RunLogGuard`, but
nothing reads it. This phase turns it into one `event: "run"` record in
the dated events segments when each run ends. The record carries the run's
model, its outcome, a computed `failure_class` and the tally. It also
fills the ghost `RunResult`'s `Budget` with what the run actually spent.
Readers (`scorecard`, `profile`) are phases 03–04.

## Spec

**Files:** `src/daemon/run/record.rs` (new), `src/daemon/run/mod.rs`,
`src/daemon/run/metrics.rs`, `src/daemon/run/guard.rs`,
`src/daemon/stream.rs`, `src/daemon/ghost.rs`, `src/ai/types/wire.rs`,
`docs/design-reference.md`.

1. **The tally grows two counts.** `RunMetrics` gains `turns: u32` with
   `record_turn()`, and `backend_errors: u32` with
   `record_backend_error()`, both `saturating_add`. Feed them:
   - **turns**: the ghost loop calls it wherever it emits `TurnStarted`;
     the chat loop calls it once per iteration that sends a request to the
     model. Scheduled jobs record none.
   - **backend errors**: every `AiEvent::Error` arm in both loops, and the
     chat loop's give-up when the stream closes without completing after
     its retries.
   - **model**: both loops set `metrics.provider` / `metrics.model` from
     the client's configured provider and model **before** the first model
     call. That way a run whose first call fails still names its model;
     `record_model_call` overwrites them as before.
2. **`failure_class`**: a pure function in `run/record.rs` taking
   `(&RunState, &RunMetrics)`, plus `is_model_attributable(class) -> bool`
   and a `FAILURE_CLASSES` list of every class. Rules, in order:
   - `Complete` ⇒ `none`; `Cancelled` ⇒ `cancelled`; `BudgetExceeded` ⇒
     `budget_exceeded`. The tally is ignored for these: a complete run
     with failed tools is still `none`.
   - `HardFail`, first match wins: `backend_errors > 0` ⇒
     `backend_error`; else `parse_failures > 0` ⇒ `parse_format`; else
     `tool_successes < tool_calls` ⇒ `tool_errors`; else `other`.
   - `stall` is in `FAILURE_CLASSES` but nothing produces it yet (the
     governor is a later milestone).
   - Only `none`, `cancelled`, `budget_exceeded`, `backend_error` and
     `other` are **not** model-attributable.
3. **`RunRecord`** in `run/record.rs`, re-exported from `run/mod.rs`:
   `Serialize + Deserialize + Default`, `#[serde(default)]` on the struct
   so later readers parse older or partial lines. Fields: `run_id`, `kind`
   (the `RunKind::label()`), `agent: Option<String>`, `provider`, `model`
   (`Option<String>`), `status` (`complete` | `hard_fail` |
   `budget_exceeded` | `cancelled`), `failure_class`,
   `is_model_attributable`, `turns`, `tool_calls`, `tool_success_rate`
   and `parse_failure_rate` (`Option<f64>`: JSON `null` with no tool
   calls), `tokens` (`TokenBreakdown`), `cost_usd`, `wall_clock_s: f64`,
   `tags: BTreeMap<String, String>`. Build it with a pure constructor from
   the run id, kind label, state, metrics, wall clock and tags. No field
   may be named `ts`, `event` or `pid`, because `log_event` adds those.
   `TokenBreakdown` derives only `Serialize` today; add `Deserialize`.
4. **The guard emits it.** `RunLogGuard` records an `Instant` at `start`,
   and holds the tags. The tags are seeded at `start`: `kind` is always
   set, plus `runbook` for `Ghost`, `agent` for `Delegated` and `job` for
   `Scheduled`. A `tag(key, value)` method adds more. The ghost loop tags
   `agent` and `host` (the ghost's `ssh_target`) when it has them. The
   record's `agent` is the `agent` tag. In `Drop`, after the
   `RunFinished` append, the guard builds the `RunRecord` from the final
   state and calls `log_event("run", ..)`. A serialization error is logged
   and skipped, never a panic. Background command runs (`bg_shell`) do not
   use the guard and must emit no `run` record: they make no model call,
   and their kind is `Chat`, so a record would double the chat count.
5. **The ghost's real spend.** The ghost `RunResult` is built with
   `Budget::unlimited()`. Build it instead from the limits the run ran
   under: `max_turns` is the effective ghost turn cap the loop enforced,
   and the other four are `0`, since no other cap exists yet. Then record
   the tally into it: turns, tool calls, `tokens.total()`, wall-clock
   seconds, and `cost_usd` as micro-USD, rounded. A
   `RunMetrics`/`Budget` helper that does the folding is the expected
   shape.
6. **Document the event** in `docs/design-reference.md`, beside the
   `ai_cost` schema: one example `run` line, the class rules and the
   attributable set.

## Tests

- In `run/record.rs`:
  - `terminal_states_map_to_their_class`: `Complete`, `Cancelled` and
    `BudgetExceeded`, each with a tally that has failed tools, map to
    `none`, `cancelled` and `budget_exceeded`.
  - `hard_fail_is_classified_by_the_tally`: a backend error plus parse
    failures gives `backend_error`; parse failures plus failed tools give
    `parse_format`; failed tools alone give `tool_errors`; a clean tally
    gives `other`.
  - `only_model_failures_are_attributable`: walks every `FAILURE_CLASSES`
    entry; `tool_errors`, `parse_format` and `stall` are attributable, the
    other five are not.
  - `a_run_record_round_trips_and_old_lines_parse`: serialize then parse
    is equal, and `{}` parses to the default record.
  - `a_run_record_without_tool_calls_has_null_rates`: the JSON has
    `"tool_success_rate":null`.
- In `run/metrics.rs`: `the_tally_folds_into_a_budget`. A tally of 3
  turns, 4 tool calls, a known token total and $0.0125 leaves exactly
  those amounts spent; the USD dimension reads 12 500 micro-USD.
- In `run/guard.rs`:
  - `guard_drop_emits_one_run_event`: a `Ghost` run, tagged `agent` and
    `host`, with one model call, finished `HardFail` with failed tools.
    After drop, today's events segment holds exactly one `event == "run"`
    line, with this run id, `kind` `ghost`, the runbook, agent and host
    tags, `status` `hard_fail`, `failure_class` `tool_errors`, the model,
    and `wall_clock_s >= 0`.
  - `guard_without_finish_records_class_other`: an unfinished guard's
    record is `hard_fail` / `other`, not attributable.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Both loops feed turns (`stream_turns=` and `ghost_turns=` ≥ 1), and
      backend errors (`stream_backend=` ≥ 2, `ghost_backend=` ≥ 1).
- [ ] `ghost_unlimited_budget=0` (it is `1` today).
- [ ] Preservation: `bg_shell_run_records=0`.
- [ ] `docs/design-reference.md` names the `run` event
      (`design_ref_run_event=` ≥ 1; `0` today).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2438 by the new tests.
      Report the number; do not re-run to chase it.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`, no
      production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-02.txt; : > "$A"
for t in terminal_states_map_to_their_class hard_fail_is_classified_by_the_tally only_model_failures_are_attributable a_run_record_round_trips_and_old_lines_parse a_run_record_without_tool_calls_has_null_rates the_tally_folds_into_a_budget guard_drop_emits_one_run_event guard_without_finish_records_class_other; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "stream_turns=$(grep -c 'record_turn()' src/daemon/stream.rs)" >> "$A"
echo "ghost_turns=$(grep -c 'record_turn()' src/daemon/ghost.rs)" >> "$A"
echo "stream_backend=$(grep -c 'record_backend_error()' src/daemon/stream.rs)" >> "$A"
echo "ghost_backend=$(grep -c 'record_backend_error()' src/daemon/ghost.rs)" >> "$A"
echo "ghost_unlimited_budget=$(grep -c 'Budget::unlimited()' src/daemon/ghost.rs)" >> "$A"
echo "bg_shell_run_records=$(grep -cE 'RunRecord|log_event\("run"' src/daemon/executor/bg_shell.rs)" >> "$A"
echo "design_ref_run_event=$(grep -c '"event": "run"' docs/design-reference.md)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-02-run-record.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-02.txt
diff /tmp/pasted-m30-02.txt /tmp/e2e-m30-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any reader of `run` events: `scorecard` (phase 03) and `profile`
  (phase 04).
- Producing `stall` (the governor, a later milestone).
- The daily cap, and the five uncosted model calls (phase 05).
- A record for background command runs (`bg_shell`).
- Ghost caps other than turns.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-20 21:20 (started)

Executor r1 (Rexy executor, Claude Sonnet 4.6) beginning phase 02. Status
flipped to `in-progress` here and in the milestone README's phase table. Plan:
grow the tally with `turns` / `backend_errors` and the pre-call
`set_model` (task 1), `failure_class` + `is_model_attributable` +
`FAILURE_CLASSES` (task 2), `RunRecord` + the `TokenBreakdown` Deserialize
gap (task 3), the guard's `Instant`, tags and drop emission (task 4), the
ghost's real `Budget` (task 5), then the design-reference section (task 6)
and the end-to-end capture.

### Update — 2026-10-20 21:45 (end-to-end verification)

Ran `cargo fmt --all` first, then the spec's § End-to-end verification block
verbatim, once. The pasted output is the first fenced block below.

```
terminal_states_map_to_their_class: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
hard_fail_is_classified_by_the_tally: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
only_model_failures_are_attributable: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_run_record_round_trips_and_old_lines_parse: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_run_record_without_tool_calls_has_null_rates: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_tally_folds_into_a_budget: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
guard_drop_emits_one_run_event: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
guard_without_finish_records_class_other: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
stream_turns=1
ghost_turns=1
stream_backend=2
ghost_backend=1
ghost_unlimited_budget=0
bg_shell_run_records=0
design_ref_run_event=1
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2449 failed=0
```

PASTE MATCH

### Update — 2026-10-20 22:07 (complete, server-authored)

**Summary:** **Summary.** Phase 02 of M30 is implemented and committed (`c460b8ee`): `RunMetrics` grows `turns` / `backend_errors` (fed by both loops — the chat loop records a turn per model request plus a backend error on `AiEvent::Error` and on the give-up after `MAX_CHANNEL_CLOSED_RETRIES`, the ghost loop a turn per `TurnStarted` and one on its `AiEvent::Error`) and names the configured provider/model via `set_model` before the first call; `run/record.rs` ships `failure_class` / `is_model_attributable` / `FAILURE_CLASSES` and the `#[serde(default)]` `RunRecord` (hand-written `Default`/`PartialEq` because `RunStatus` has no `Default` and `TokenBreakdown` neither `Deserialize`-derived-default nor `PartialEq` — `TokenBreakdown` already had a custom `Deserialize`, so only the record needed the accommodation); `RunLogGuard` now takes an `Instant` at start, seeds tags from the kind (`runbook`/`agent`/`job`), accepts `tag()`, and at drop appends exactly one `event:"run"` line after `RunFinished` (serialization error logged and skipped); the ghost's `RunResult.budget` is the effective turn cap (`gc.max_ghost_turns.min(daemon_ceiling)`) with the tally folded in — `Budget::unlimited()` is gone from `ghost.rs` — and `docs/design-reference.md` documents the event beside `ai_cost`. End-to-end block ran verbatim once at the final tree: all 8 named tests `ok. 1 passed`, `stream_turns=1 ghost_turns=1 stream_backend=2 ghost_backend=1 ghost_unlimited_budget=0 bg_shell_run_records=0 design_ref_run_event=1`, doc_truth 16/0, clippy `-D warnings` clean, `fmt_exit=0`, full suite `passed=2449 failed=0` (2438 + 11 new: 5 record, 2 metrics, 1 budget-spent accessor folded into the folds test, 2 guard, plus `a_tally_with_no_spend_folds_to_zeroes` and `budget_exceeded_names_its_dimension_in_a_record` beyond the named list), `PASTE MATCH`.

**Notes for review.** (1) The gate's `cargo test` failure was the pre-existing `bounded_output_timeout_kills_a_forked_grandchild` in `src/process.rs`, not phase code: the grandchild's SIGKILL delivery can trail `bounded_output_with`'s return under load, and the test asserted death immediately inside its poll loop (the 1s deadline was effectively dead code — the assert preceded the deadline check). I restructured it to wait up to 1s for the state to clear, then assert — verified 8× solo plus two full-suite runs (`passed=2449 failed=0` both). Committed separately as `f3f728d1` since it is outside the phase's file list; flagging it as a scope extension rather than burying it. (2) Two acceptance-criterion arithmetic notes: `stream_turns`/`ghost_turns` read exactly 1 (the criterion asks ≥1); `passed=2449` is 11 above the 2438 baseline because of the two extra unnamed tests listed above. (3) `bg_shell_run_records=0` was verified by grep before the phase began (the file never touches the guard), and again in the artifact. Working tree clean; phase doc and README row left at `in-progress` for the server to finalize.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


TEST
 readme_tools_tables_match_the_code ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_append_read ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


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
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (2459265): No such process
kill: (-2459989): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M30-telemetry-dashboard/README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/phase-02-run-record.md` — +38 -1
- `src/daemon/ghost.rs` — +58 -3
- `src/daemon/run/guard.rs` — +272 -2
- `src/daemon/run/metrics.rs` — +99 -0
- `src/daemon/run/mod.rs` — +8 -0
- `src/daemon/run/record.rs` — +422 -0
- `src/daemon/stream.rs` — +6 -0
- `docs/design-reference.md` — +46 -0

**Commit:** c460b8ee9d77bde42b072f9a9defa79d8eaa3bf8

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
roval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (2809958): No such process
kill: (-2810680): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/design-reference.md` — +46 -0
- `docs/dev/milestones/M30-telemetry-dashboard/README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/phase-02-run-record.md` — +41 -1
- `src/daemon/ghost.rs` — +38 -3
- `src/daemon/run/guard.rs` — +267 -5
- `src/daemon/run/metrics.rs` — +99 -0
- `src/daemon/run/mod.rs` — +8 -0
- `src/daemon/run/record.rs` — +422 -0
- `src/daemon/stream.rs` — +6 -0
- `src/process.rs` — +10 -5

**Commit:** f3f728d1960f10d174ad6c096ce535b6ec94dff0

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-20

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** one, flagged by the executor: `f3f728d1` restructures the pre-existing `bounded_output_timeout_kills_a_forked_grandchild` test in `src/process.rs` (outside the phase's file list) so its 1 s deadline is no longer dead code. Test-only, correct, and it unblocked the gate; accepted.
- **Calibration:** the Spec said `TokenBreakdown` "derives only `Serialize` today; add `Deserialize`" — it already had a hand-written `Deserialize` impl, which my pre-dispatch read missed (I checked the derive line, not the file). The executor noticed and did not add a duplicate. Independent re-run at `b49cab92`: fmt/build/clippy exit 0, 2449 passed 0 failed; all seven e2e count lines verified against the tree.
