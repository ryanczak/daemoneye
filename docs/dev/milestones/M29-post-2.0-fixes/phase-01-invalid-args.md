# Phase 01: invalid-args — honest wording, no budget exemption

**Milestone:** M29 — Post-2.0 fixes
**Status:** review
**Depends on:** none
**Estimated diff:** ~80 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

Two defects in how a tool call with bad arguments is handled.

- **The wording.** `invalid_args_event` (`src/ai/tools/dispatch.rs`) writes
  every type mismatch as `parameter '<name> (<type>, optional)' is …`, even
  for a required parameter. The missing-parameter line and
  `expected_params` already get this right.
- **The budget exemption.** `daemon::stream` exempts any call whose
  `tool_name()` is in `APPROVAL_GATED_TOOLS` from the per-turn,
  per-session and per-tool caps. `PendingCall::InvalidArgs::tool_name()`
  returns the tool the model *meant* to call. So a model that keeps
  sending bad arguments to `edit_file` is never capped, even though nothing
  prompts and nothing runs. PE decision: an `InvalidArgs` call is **never**
  exempt.

## Spec

**Files:** `src/ai/tools/dispatch.rs`, `src/daemon/stream.rs`.

1. **Required or optional.** In `invalid_args_event`, the type-mismatch
   line says `required` when `param.required` is true and `optional`
   otherwise, in the same position it holds now.
2. **One exemption predicate.** Add a pure
   `fn is_budget_exempt(call: &PendingCall) -> bool` in `stream.rs`. It is
   true only when the call is **not** `PendingCall::InvalidArgs` **and**
   its `tool_name()` is in `APPROVAL_GATED`. Both places in the tool loop
   that test `APPROVAL_GATED.contains(&tool_name)` use it instead: the cap
   check and the per-session counter bump. Do not change
   `APPROVAL_GATED_TOOLS`, `tool_name()`, or how a non-exempt call is
   counted.
3. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-m29-01.txt` into a new Update Log entry
   headed `### Update — <date> (end-to-end verification)`, with the
   `PASTE MATCH` line after the fence.

**Gotchas.**
- Some arguments, such as `"false"` for a boolean, parse leniently before
  they reach `invalid_args_event`. Use a value that cannot coerce, such as
  an object or an array, for the mismatch test.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

- `a_type_mismatch_says_whether_the_parameter_is_required`
  (`dispatch.rs` tests): a `run_terminal_command` call whose `command` is
  an object yields an error containing `'command (string, required)'`. A
  call with a valid `command` and an object for `timeout_secs` yields
  `'timeout_secs (integer, optional)'`. The negative checks are
  **scoped to the parameter**: the first error does not contain
  `'command (string, optional)'`, and the second does not contain
  `'timeout_secs (integer, required)'`. Do **not** assert that the whole
  error lacks the bare word `optional` or `required`. The error ends with
  the full expected-parameter list, which correctly names other parameters
  with both words.
- `an_invalid_args_call_is_never_budget_exempt` (`stream.rs` tests):
  `is_budget_exempt` is false for a `PendingCall::InvalidArgs` named
  `edit_file`, true for a well-formed approval-gated call such as
  `edit_file`, and false for a non-gated call such as `read_file`.

## Acceptance criteria

- [ ] Both new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ about `passed=2412 failed=0` (2410 at `d3f1a83c`
      + 2). An extra test is fine: name it and report; do not re-run the
      suite to chase a number. A failure in `sbx.rs` is a known flake: run
      that one test alone once and report both runs.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] `grep -n 'APPROVAL_GATED.contains(&tool_name)' src/daemon/stream.rs`
      prints nothing.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-01.txt; : > "$A"
for t in a_type_mismatch_says_whether_the_parameter_is_required an_invalid_args_call_is_never_budget_exempt; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "gated_name_checks=$(grep -c 'APPROVAL_GATED.contains(&tool_name)' src/daemon/stream.rs)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Then run the self-check, and put its line in the
entry after the fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-01-invalid-args.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-01.txt
diff /tmp/pasted-m29-01.txt /tmp/e2e-m29-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The undeclared-parameter line and `expected_params`, which are already
  right.
- Per-tool limits config validation (`LimitsConfig::validate`).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Update — 2026-10-19 02:13 (progress)

Started by Claude (executor run). Read the spec and both target files:
`invalid_args_event`'s type-mismatch line hardcodes `optional`
(`src/ai/tools/dispatch.rs`), and `src/daemon/stream.rs` tests
`APPROVAL_GATED.contains(&tool_name)` directly in the cap check and the
per-session counter bump, so a `PendingCall::InvalidArgs` naming a gated tool
escapes both. Plan: fix the wording, add the pure `is_budget_exempt`
predicate in `stream.rs`, route both call sites through it, add the two named
tests, then capture the E2E transcript.

<!-- entries appended below this line -->

### Notes for executor — 2026-10-19

The first run stalled after 169 calls, the last 40 of them re-reads of
`dispatch.rs`. **The code changes are correct and stay as they are:** the
wording fix in `invalid_args_event`, `is_budget_exempt`, and both
`stream.rs` call sites routed through it.

`a_type_mismatch_says_whether_the_parameter_is_required` fails because of
a spec ambiguity, not a code bug. It asserts `!error.contains("optional")`,
but the error's trailing `expected:` list contains `optional` for other
parameters. Two edits remain:

1. In that test, make the two negative assertions parameter-scoped, as the
   Tests section now says: `!error.contains("'command (string, optional)'")`
   and `!error.contains("'timeout_secs (integer, required)'")`.
2. Add `an_invalid_args_call_is_never_budget_exempt` in `stream.rs`'s
   `mod tests`. A well-formed `PendingCall::EditFile` needs every field
   spelled out (`id`, `thought_signature`, `path`, `operation`, the five
   `Option`s set to `None`); `PendingCall::ReadFile` likewise. The variant
   definitions are in `src/ai/types/pending.rs`.

Then run § End-to-end verification once.


### Update — 2026-10-19 (escalation)

**Chosen lever:** resume.
**Rationale:** the stall came from a test assertion my spec wording
invited, and the code is done and correct. A resume with the corrected
Tests text and the one remaining test keeps the finished work, where a
re-dispatch would redo it.

### Update — 2026-10-19 (end-to-end verification)

Ran § End-to-end verification verbatim, once, redirecting to
`/tmp/e2e-m29-01.txt`. First pass reported `fmt_exit=1` (two formatting
drifts the write tool introduced); ran `cargo fmt --all`, then re-checked:
`cargo fmt --all --check` exits 0. Transcript:

```
a_type_mismatch_says_whether_the_parameter_is_required: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
an_invalid_args_call_is_never_budget_exempt: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
gated_name_checks=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=1
passed=2412 failed=0
```

PASTE MATCH

### Update — 2026-10-19 02:27 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Resumed the stalled M29 phase-01 run and completed the two remaining edits, exactly as the architect's guidance directed. The two failing negative assertions in `a_type_mismatch_says_whether_the_parameter_is_required` are now parameter-scoped (`!error.contains("'command (string, optional) '")` / `!error.contains("'timeout_secs (integer, required)'")`) and `an_invalid_args_call_is_never_budget_exempt` was added to `stream.rs`'s `mod tests` covering the three spec cases: an `InvalidArgs` named `edit_file` is not exempt, a well-formed `PendingCall::EditFile` is exempt, and a `PendingCall::ReadFile` is not. No production code changed beyond what was already on disk.

End-to-end verification ran verbatim, once, redirecting to `/tmp/e2e-m29-01.txt`, and the paste self-check returned `PASTE MATCH`. Results: both new tests `ok. 1 passed` each; `gated_name_checks=0` (no direct `APPROVAL_GATED.contains(&tool_name)` test remains); `doc_truth` 16 passed / 0 failed; clippy `-D warnings` clean; full `cargo test` passed=2412 failed=0 (2410 at `d3f1a83c` + 2 new), matching the acceptance criterion. One deviation worth noting: the first E2E pass recorded `fmt_exit=1` because the write tool had introduced two small formatting drifts — I ran `cargo fmt --all` and re-verified `fmt_exit=0` afterwards, so the committed tree is format-clean; the transcript's `fmt_exit=1` line is the raw, honest capture from that single run, with the remediation noted in the entry. Committed as `fix: honest required/optional in invalid-args wording; InvalidArgs never budget-exempt` (72852edd); working tree clean.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.1.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.87s


LINT
    Checking daemoneye v2.1.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.77s


TEST
tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok

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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v2.1.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 15.72s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-d9203e33e9de0e0e)
kill: (527351): No such process
kill: (-528082): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-6a738a64918f799f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9feb568c477cffdf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-05bab2f96380bc67)
     Running tests/integration.rs (target/debug/deps/integration-502483a68671b4b7)
     Running tests/isolation.rs (target/debug/deps/isolation-66ea1e45aff405d5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-0960811ac376916b)
   Doc-tests daemoneye

```

**Files changed:**

- `src/ai/tools/dispatch.rs` — +7 -3
- `src/daemon/stream.rs` — +41 -2

**Commit:** 72852edd6b58766f993ad4c73748ae797da4e54f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
