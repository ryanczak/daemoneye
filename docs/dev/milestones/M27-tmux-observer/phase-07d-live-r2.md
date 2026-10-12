# Phase 07d: live-r2 — alerts reach a reconnected chat's history; mistyped tool arguments get an error, not a silent drop

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** done
**Depends on:** phase-07a, 07b, 07c (all done)
**Tags:** language=rust, kind=fix, size=s

## Goal

Rehearsal 2 of phase 07 found two major defects that block the live close
with the PE present:

- **F8.** After a daemon restart the open chat reconnects (presence), but
  its session is not resident in the in-memory `SessionStore` until it
  sends a message. `inject_into_sessions` only reaches resident sessions,
  so an alert or ghost event in that window shows on screen but never
  lands in the session's history. The model then says no alert arrived.
- **F10.** Every `ToolArgs::from_value` is `serde_json::from_value(..).ok()`.
  When the model sends one mistyped argument to a **known** tool (for
  example `"wait": "true"`, a string), `dispatch_tool_event` returns
  `None`, and the backends log and show it as "model called unknown tool
  'run_terminal_command' — call dropped". The model gets no tool result,
  the turn ends with "The model returned an empty reply", and it cannot
  correct itself. The raw arguments are logged nowhere.

Plus one nit: the chat's reconnect notices render as `⚙ ⚙ …` (doubled gear).

## Work list — this table is the whole job

Rules (they turned 07b and 07c into clean runs):

- **Open only the files a row names.** The compiler will name any further
  exhaustive `match` you must extend; extend those and nothing else.
- **Your first `patch` lands within 10 turns.** That patch is row 1.

| # | File · symbol | Change |
|---|---|---|
| 1 | `src/daemon/session.rs` · presence registry | Add `pub fn presence_session_ids() -> Vec<String>`: the session ids that have at least one live presence connection. |
| 2 | `src/webhook/process.rs` · `inject_into_sessions` | Append `msg` to disk (`append_session_message`) for every id in the **union** of the store's keys and `presence_session_ids()`, each id once. The in-memory push stays as it is: resident entries only. Do **not** insert a store entry for a non-resident id: `handle_ask` loads history from disk only when the session is not resident, so an empty resident entry would lose it. |
| 3 | `src/ai/types/events.rs` · `AiEvent` | Add `InvalidToolArgs { id: String, name: &'static str, arguments: String, error: String, thought_signature: Option<String> }`. |
| 4 | `src/ai/tools/dispatch.rs` | Add `pub fn invalid_args_event(id: &str, name: &str, args: &Value, ts: Option<String>) -> Option<AiEvent>`. `None` when `name` is not in `TOOLS`. Otherwise `Some(InvalidToolArgs)`, where `name` is the `TOOLS` entry's `&'static str`, `arguments` is `args` serialised, and `error` names every problem it finds from the `ToolDef`'s `ParamDef`s: each missing required param, and each present param whose JSON type does not match its `ParamTy` (`Str` → string, `Bool` → boolean, `Int` → integer). If it finds none, `error` says the arguments do not match the schema. Either way, `error` ends with the expected parameter list, `name (type, required)`. |
| 5 | `src/ai/backends/openai.rs`, `anthropic.rs`, and `gemini.rs` (the drain loop, not the `has_dispatchable` predicate) | Where `dispatch_tool_event` returns `None`: first try `invalid_args_event`. If that is `Some`, log WARN `model sent invalid arguments for tool '<name>': <args>` and send the event. Only if it is `None` keep today's "unknown tool" path. |
| 6 | `src/ai/types/pending.rs` · `PendingCall` | Add `InvalidArgs { id, name: &'static str, arguments: String, error: String, thought_signature }`. `to_tool_call` gives back `name` and the raw `arguments` unchanged, so history shows the call exactly as the model made it. Fill in the `id`, `tool_name` (returns `name`) and `summary` arms. `should_emit_tool_feedback` falls to the catch-all. Add it to any test that lists every variant. |
| 7 | `src/daemon/stream.rs` · the `AiEvent` match | `AiEvent::InvalidToolArgs` → push `PendingCall::InvalidArgs`. |
| 8 | `src/daemon/executor/mod.rs` · `execute_tool_call` | `PendingCall::InvalidArgs` → `Ok(ToolCallOutcome::Result(..))` with `Error: invalid arguments for <name>: <error>. Fix the arguments and call <name> again.` It runs nothing and prompts nothing. |
| 9 | `src/cli/commands/chat_presence.rs` | The two notices drop their leading `"⚙ "` (`daemon connection lost — reconnecting…`, `reconnected`); the renderer already adds the gear. Update the test that asserts them. |

### Tests (names are pinned; bodies are yours)

- `inject_reaches_a_presence_attached_session_that_is_not_resident`
  (`src/webhook/process.rs`): register a presence connection for an id that is
  **not** in the store, call `inject_into_sessions`, and assert the
  message is in that id's session file. Then unregister. Use
  `crate::test_home_guard()` and a temp `HOME`, as the session tests do.
- `invalid_args_event_names_the_mistyped_param` (`src/ai/tools/dispatch.rs`):
  `run_terminal_command` with `{"command":"x","wait":"true"}` gives
  `Some(InvalidToolArgs)` whose `error` contains `wait` and `boolean`. Also
  `{"wait":true}` gives an `error` containing `command` (missing required).
  Also `no_such_tool` gives `None`.
- `invalid_args_pending_call_keeps_the_raw_call` (`src/ai/types/pending.rs`):
  `to_tool_call` returns the original name and the raw arguments string.

Mutation check: make `invalid_args_event` always return `None`. Confirm
`invalid_args_event_names_the_mistyped_param` fails, then restore. Do the
same for row 2: drop the presence ids from the union and confirm the inject
test fails. Paste both failing lines.

## Acceptance criteria

- [x] The three tests above each report `1 passed`.
- [x] `grep -c '"⚙ ' src/cli/commands/chat_presence.rs` prints `0`. Today `4`.
- [x] *Preservation:* `cargo test --test tmux_read_only` and
      `cargo test --test doc_truth` report `failed=0`.
- [x] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [x] `cargo test` reports `failed=0` and `passed=2322`: today's 2319 plus
      the three tests above.

## End-to-end verification

Paste the three named test results, both mutation-check failing lines,
the grep, and the full-suite totals verbatim in the Update Log with
`PASTE MATCH: yes`. The live check is phase 07 rehearsal 3 (architect).

## Authorizations

- [x] Edit the files the rows name, and extend any exhaustive `match` the
      compiler reports for the new `AiEvent` / `PendingCall` variants.

## Out of scope

- Injecting scheduled-job results into session history. Today they are
  broadcast only, by design.
- F9 (`[ACTIVE PANE]` labels the non-chat pane when the chat pane is
  excluded). It is a labelling design question, deferred.
- Coercing mistyped arguments (for example `"true"` → `true`). The model
  gets an error and retries; nothing guesses for it.
- Changing `ToolArgs::from_value`'s signature.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-12 17:45 (started)

**Executor:** Claude (executor run). Status flipped to `in-progress`; README
phase-table row updated to match. Starting the 9-row work list in order.

### Update — 2026-10-12 18:20 (end-to-end verification)

All three pinned tests, both mutation checks, the gear grep, and the full
suite were captured mechanically in one pass each; the transcripts below are
pasted verbatim from those runs. Commit `025a1684` carries the code; the
working tree is clean.

**The three pinned tests** (`cargo test --lib <name>` each):

```
test webhook::process::tests::inject_reaches_a_presence_attached_session_that_is_not_resident ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2260 filtered out; finished in 0.00s

test ai::tools::dispatch::tests::invalid_args_event_names_the_mistyped_param ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2260 filtered out; finished in 0.00s

test ai::types::pending::tests::invalid_args_pending_call_keeps_the_raw_call ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2260 filtered out; finished in 0.00s
```

**Mutation check 1 — `invalid_args_event` forced to `None`** (an env-gated
early `return None` at the top of the function, run with
`DAEMONEYE_MUTATION_CHECK=1`): the pinned dispatch test fails on its first
expect, exactly as required.

```
test ai::tools::dispatch::tests::invalid_args_event_names_the_mistyped_param ... FAILED

thread 'ai::tools::dispatch::tests::invalid_args_event_names_the_mistyped_param' (907738) panicked at src/ai/tools/dispatch.rs:1174:10:
a known tool always yields an event
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2260 filtered out; finished in 0.00s
```

**Mutation check 2 — presence ids dropped from the union** (an env-gated
`if !union_blocked { ids.extend(presence_session_ids()) }`, run with
`DAEMONEYE_MUTATION_CHECK_INJECT=1`): the pinned inject test fails on the
session-file read.

```
test webhook::process::tests::inject_reaches_a_presence_attached_session_that_is_not_resident ... FAILED

thread 'webhook::process::tests::inject_reaches_a_presence_attached_session_that_is_not_resident' (908791) panicked at src/webhook/process.rs:812:33:
the session file "/tmp/.tmpyu2OhO/.daemoneye/var/log/sessions/reconnected-after-restart.jsonl" exists: No such file or directory (os error 2)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2260 filtered out; finished in 0.00s
```

Both mutations were reverted after capture (`git diff --stat` returned
empty before the final full-suite run, which is the run below).

**Gear grep** — `grep -c '"⚙ ' src/cli/commands/chat_presence.rs`:

```
0
```

(grep exits 1 on a zero count; the count printed is the evidence. It was 4
before this phase.)

**Preservation:**

```
$ cargo test --test tmux_read_only
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

$ cargo test --test doc_truth
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

**Full suite** (`cargo test`, one run, tee'd to a file and summed
mechanically — `awk '{p+=$4; f+=$6} END {print "passed="p" failed="f}'`
over the pasted result lines):

```
test result: ok. 2251 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 64.23s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

passed=2322 failed=0
```

**Gate commands:**

```
$ cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo build
    Checking daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.36s
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.94s
```

**PASTE MATCH: yes.** The live check is phase 07 rehearsal 3 (architect).

### Update — 2026-10-12 18:23 (complete, server-authored)

**Summary:** </think>All the spec-pinned literals landed. Working tree is clean, status is `in-progress`, no `(complete)` entry hand-written, all gates pass. Let me do a final completion checklist:

- [x] Phase doc's Status: line still says `in-progress` ✓ (line 4)
- [x] Milestone README's phase-table row still says `in-progress` ✓
- [x] No hand-written `(complete)` entry ✓ (I wrote `started` and `end-to-end verification` entries only)
- [x] `cargo fmt --all` run before final commit ✓
- [x] All verification commands ran clean ✓ (fmt, clippy -D warnings, build zero new warnings, test passed=2322 failed=0)
- [x] Three pinned tests each `1 passed` ✓
- [x] Mutation checks captured ✓
- [x] Gear grep prints 0 ✓
- [x] Preservation tests failed=0 ✓
- [x] Full suite passed=2322 ✓
- [x] git status clean ✓
- [x] Final message: Summary + Notes for review — now.

One more consideration: the phase's acceptance criterion says `cargo test` reports `passed=2322` — confirmed. Clippy and fmt exit 0 — confirmed.

I should write the final Summary + Notes for review now. Keep it 2-6 sentences, substantive.</think>
**Summary + Notes for review**

Implemented all 9 rows of phase 07d: `presence_session_ids()` exposes the live presence registry (row 1); `inject_into_sessions` appends to the session file for the union of resident store keys and presence-attached ids, each id once, while the in-memory push stays resident-only and no store entry is created for a presence-only id (row 2); `AiEvent::InvalidToolArgs` and `PendingCall::InvalidArgs` carry the raw arguments so history shows the call exactly as the model made it (rows 3, 6, 7); `invalid_args_event` enumerates every missing required param and mistyped param type from the `ToolDef`'s `ParamDef`s, ending with the expected parameter list, and `None`s only for names not in `TOOLS` (row 4); the three backend drain loops (anthropic, openai, gemini — not the `has_dispatchable`-era predicates, which no longer exist) try `invalid_args_event` first and only fall back to the unknown-tool path when it returns `None` (row 5); the executor returns the listed mistakes as a tool result that runs nothing and prompts nothing (row 8); the two reconnect notices dropped their leading gear, count now 0 (row 9). End-to-end verification: the three pinned tests each report `1 passed`; both mutation checks (env-gated early `None` in `invalid_args_event`, env-gated union skip in `inject_into_sessions`) produced failing runs whose lines are pasted verbatim in the Update Log, then were reverted; `grep -c '"⚙ '` prints `0`; `tmux_read_only` (4 passed) and `doc_truth` (16 passed) report `failed=0`; full `cargo test` reports `passed=2322 failed=0` — today's 2319 plus the three new tests, matching the criterion exactly. fmt, clippy `-D warnings`, and build all exit 0 with no new warnings; committed as `025a1684` (code) and `94ef5560` (evidence); working tree clean; phase status left at `in-progress` for the server's completion tail. Notes for review: (1) the mutation checks were done with temporary env-gated code paths rather than hand edits, both reverted before the final suite run — if you prefer hand-edit capture, the pasted lines still show the real failure modes; (2) `ParamTy::as_str` was widened from private to `pub(crate)` so `invalid_args_event` can name types — the only file touched beyond the rows' named files plus the compiler-mandated `match` extensions (pending.rs `id`/`summary`/`tool_name` arms, ghost.rs's `AiEvent` match, stream.rs, executor); (3) `invalid_args_event` does not consult the model's `loaded` tool set — a call to a known-but-deferred tool with bad args gets the invalid-args error rather than the unknown-tool notice, which I judged correct since the tool exists; (4) gemini's early `has_dispatchable`-style call site at line 39 was left alone per the row's "drain loop, not the predicate" rule.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
st readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (920288): No such process
kill: (-920997): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-a6c1d042be279283)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M27-tmux-observer/README.md` — +1 -1
- `docs/dev/milestones/M27-tmux-observer/phase-07d-live-r2.md` — +103 -1
- `src/ai/backends/anthropic.rs` — +5 -2
- `src/ai/backends/gemini.rs` — +12 -2
- `src/ai/backends/openai.rs` — +9 -1
- `src/ai/tools/dispatch.rs` — +149 -0
- `src/ai/tools/mod.rs` — +1 -1
- `src/ai/tools/schema.rs` — +1 -1
- `src/ai/types/events.rs` — +10 -0
- `src/ai/types/pending.rs` — +54 -0
- `src/cli/commands/chat_presence.rs` — +4 -5
- `src/daemon/executor/mod.rs` — +17 -0
- `src/daemon/ghost.rs` — +15 -0
- `src/daemon/session.rs` — +14 -0
- `src/daemon/stream.rs` — +15 -0
- `src/webhook/process.rs` — +65 -3

**Commit:** 94ef55600ce94d754129407d211048764278f7b2

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-12

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (284 turns)
- **Reviewer re-run:** fmt, build and clippy clean; `passed=2322 failed=0`;
  the three named tests report `1 passed` each; the gear grep prints `0`;
  no mutation scaffolding (`DAEMONEYE_MUTATION*`) is left in `src/`.
- **Scope deviations:** none of substance. `ParamTy::as_str` went from
  private to `pub(crate)`, and `ghost.rs`'s `AiEvent` match was extended,
  both compiler-required. The mutation checks used temporary env-gated code
  rather than hand edits. Accepted: the failing lines are pasted and the code
  is gone.
- **Follow-up (not blocking):** `PendingCall::InvalidArgs::tool_name()`
  returns the real tool name. An invalid call to an approval-gated tool
  (`run_terminal_command`) therefore inherits that tool's exemption from the
  per-turn and per-session budget caps, so a model looping on bad arguments is
  bounded only by the turn's other limits. Worth capping in a later
  milestone. The live check is phase 07 rehearsal 3.
- **Calibration:** none
