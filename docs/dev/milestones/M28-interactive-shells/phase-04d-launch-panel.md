# Phase 04d: launch-panel — the approval panel says a launch keeps the shell

**Milestone:** M28 — Interactive shells
**Status:** done
**Depends on:** phase-04c
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

The PE run of the live close (`phase-04-live-close.md`, Update Log)
decided two things:

- **D6's panel line, fix before 2.1.0.** The approval panel for
  `run_terminal_command("claude", interactive: true)` shows only
  `$ claude`. Nothing tells the user that the command will keep the shell
  and that nothing waits for it. The same is true of an auto-upgraded
  `ssh zoidberg`.
- **F10 (nit).** The busy-shell refusal reads `Error: Error: shell s1 …`:
  the `RefusedBeforeWrite` branch of `shell_run_error_text` prefixes
  `Error: ` to a chain whose own message already starts with `Error: `.

The constraint on the panel: **the command string is not changed.** It is
what the approval classification, the ghost `is_safe` check and the user's
session approval all read. The launch fact travels as its own field.

## Spec

**Files:** `src/ipc.rs`, `src/daemon/executor/mod.rs`,
`src/daemon/executor/foreground.rs` (one literal), `src/cli/commands/stream.rs`,
plus the `Response::ToolCallPrompt` literals the compiler names in
`src/cli/commands/slash.rs` and `src/ipc_tests.rs` (5).

1. **The frame carries the fact.** `Response::ToolCallPrompt` gains
   `#[serde(default)] interactive: bool`, so an older daemon's frame still
   parses with `false`. `ApprovalRequest` gains `interactive: bool`, and
   `prompt_and_await_approval` copies it into the frame. Every other
   `ApprovalRequest` literal (`send_input`, `foreground.rs`, the named-host
   arm) passes `false` except the two `run_terminal_command` arms in Task 2.
2. **What counts as a launch.** Add a pure
   `fn is_launch(interactive: bool, cmd: &str) -> bool` in
   `executor/mod.rs`: true when `interactive` is set **or**
   `crate::daemon::utils::is_interactive_command(cmd)` recognises the
   command, so an auto-upgraded `ssh host` is labelled too. Both
   `run_terminal_command` approval sites (local and named host) pass
   `is_launch(*interactive, cmd)`.
3. **The panel line.** `tool_call_panel_body` (`stream.rs`) takes
   `interactive: bool` and, when true, pushes
   `→ interactive: the command keeps the shell; nothing waits for it`
   right after the `$ <cmd>` line, before any classification line.
   `prompt_tool_call_ratatui` takes and forwards it; the client's
   `ToolCallPrompt` arm destructures the new field. Every existing call of
   `tool_call_panel_body` passes `false`; the compiler names them, and their
   assertions do not change.
4. **F10, one prefix.** In the `RefusedBeforeWrite` branch of
   `shell_run_error_text`, strip one leading `Error: ` from the chain
   before formatting, so the result starts with exactly one `Error: `. A
   chain that does not start with it (the cap refusal: `owner '…' already
   holds …`) is unchanged.
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-04d.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, with the `PASTE MATCH`
   line after the fence.

**Gotchas.**
- Do not touch `prompt_classification`, `ipc::classify` or the ghost
  `is_safe` path. The panel line is a display fact only.
- `tool_call_panel_label` ("terminal · visible to you") is unchanged: the
  launch still runs in a shell the user can see.
- Find anchors with `grep -n`; read windows, not lines.

## Tests

- `tool_call_prompt_without_interactive_parses_as_false` (`ipc_tests.rs`):
  a `ToolCallPrompt` JSON line with no `interactive` key deserialises with
  `interactive == false`; one with `"interactive":true` gives `true`.
- `a_launch_is_the_flag_or_a_recognised_interactive_command`
  (`executor/mod.rs` tests): `is_launch(true, "vim")` and
  `is_launch(false, "ssh host")` are true; `is_launch(false, "ls")` is
  false.
- `the_launch_panel_says_the_command_keeps_the_shell` (`stream.rs` tests,
  next to `the_local_command_panel_is_unchanged`): with `interactive: true`
  the body is `$ claude` then the `→ interactive: …` line; with `false` the
  body has no `interactive` line.
- `a_refusal_starts_with_exactly_one_error_prefix` (`executor/mod.rs`
  tests): `shell_run_error_text` on a `RefusedBeforeWrite` error whose
  context starts `Error: shell s1 is running claude (TUI)…` starts with
  `Error: shell s1` and does not contain `Error: Error:`; on one whose
  context is `owner 'chat:x' already holds 5 shells (cap 5)` it starts with
  `Error: owner`.

## Acceptance criteria

- [ ] The 4 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2410 failed=0` (2406 at `9713ec5b` + 4). An
      extra test is fine: name it and report the count you got. **Do not
      re-run the suite to chase a mismatch.** A failure in `sbx.rs` is a
      known pre-M28 flake: run that one test alone once and report both runs.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`) and
      `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-04d.txt; : > "$A"
for t in tool_call_prompt_without_interactive_parses_as_false a_launch_is_the_flag_or_a_recognised_interactive_command the_launch_panel_says_the_command_keeps_the_shell a_refusal_starts_with_exactly_one_error_prefix the_local_command_panel_is_unchanged a_busy_refusal_says_nothing_was_written; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Then run the self-check, and put its line in the
entry after the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-04d-launch-panel.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04d.txt
diff /tmp/pasted-04d.txt /tmp/e2e-04d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The per-result `wait: false` note (PE decision: leave as is).
- Any change to approval classification or session-approval matching.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-19 01:05 (progress)

Started phase-04d (executor). Status flipped to `in-progress` here and in the
milestone README's phase table. Plan: add `interactive` to
`Response::ToolCallPrompt` and `ApprovalRequest`, wire `is_launch` through both
`run_terminal_command` approval sites, render the panel line in
`tool_call_panel_body`, fix the F10 double `Error: ` prefix, add the four
spec'd tests, then capture the end-to-end transcript verbatim.

### Update — 2026-10-19 01:20 (end-to-end verification)

Ran § End-to-end verification once, top to bottom, with each command's output
appended to `/tmp/e2e-04d.txt`. The pasted block below is that file verbatim.

```
tool_call_prompt_without_interactive_parses_as_false: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_launch_is_the_flag_or_a_recognised_interactive_command: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_launch_panel_says_the_command_keeps_the_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_refusal_starts_with_exactly_one_error_prefix: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_local_command_panel_is_unchanged: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_busy_refusal_says_nothing_was_written: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2410 failed=0
```

PASTE MATCH

### Update — 2026-10-19 01:20 (complete, server-authored)

**Summary:** </think>All tasks done. Status line still in-progress (I did not flip to review). The e2e paste-match verified. Working tree clean. One commit.

Let me double check the phase doc status is still in-progress and I didn't write a (complete) entry. Yes — I wrote progress + e2e verification entries only.

Summary time.</think>
All five spec tasks are implemented and committed; working tree is clean and the phase doc remains `in-progress` with no hand-written completion entry.

**Summary + Notes for review**

Built phase-04d: `Response::ToolCallPrompt` and `ApprovalRequest` gained an `interactive: bool` (serde-defaulted on the wire, so an older daemon's frame still parses as `false`); a new pure `is_launch(interactive, cmd)` in `executor/mod.rs` feeds it from both `run_terminal_command` approval arms (local and named host) and also catches auto-upgraded `ssh host` via `is_interactive_command`; `tool_call_panel_body` renders `→ interactive: the command keeps the shell; nothing waits for it` right after the `$ cmd` line when the flag is set, before any classification line, with `prompt_tool_call_ratatui` forwarding it and the client arm destructuring the new field. F10 is fixed by stripping one leading `Error: ` from the chain in the `RefusedBeforeWrite` branch of `shell_run_error_text`, so the busy refusal prints one prefix and the unprefixed cap refusal still gets exactly one. The four spec'd tests plus a regression-adjacent assertion set pass individually; end-to-end verification ran verbatim, was captured to `/tmp/e2e-04d.txt`, pasted into a dedicated `(end-to-end verification)` Update Log entry, and the self-check returned **PASTE MATCH**. Full gates: `cargo test` `passed=2410 failed=0` (spec predicted 2410 — exact match), `doc_truth` 16 passed/0 failed, clippy `-D warnings` clean, `cargo fmt --all --check` exit 0, no new `#[allow]` (the existing `too_many_arguments` comment was updated to 9 params, not silenced). Command string is untouched everywhere; classification and the ghost `is_safe` path were not modified. Notes: the `send_input` arm and `foreground.rs` pass `false` as specified; the mid-run edits routed through the verifier's machine-applicable suggestions are exactly the field additions it names. One commit, `feat: label launches on the approval panel and single-prefix refusals (04d)`; `git status --short` is empty.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.86s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
st readme_tools_tables_match_the_code ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test config_pricing_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.08s


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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (449769): No such process
kill: (-450504): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-04d-launch-panel.md` — +30 -1
- `src/cli/commands/slash.rs` — +1 -0
- `src/cli/commands/stream.rs` — +60 -4
- `src/daemon/executor/foreground.rs` — +1 -0
- `src/daemon/executor/mod.rs` — +78 -0
- `src/ipc.rs` — +6 -0
- `src/ipc_tests.rs` — +23 -0

**Commit:** 72cff2a250f37a3d721c2194a69b823249bc0249

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-18

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (one dispatch, 164 turns)
- **Scope deviations:** none. The spec said both `run_terminal_command`
  approval sites (local and named host) pass `is_launch(...)`. There is one:
  the foreground approval comes before the host split, so the single
  `is_launch` call at that site covers both arms. The premise was an
  architect error, and the outcome is correct. The other `ApprovalRequest`
  literals (the chat `wait: false` background arm, `send_input`,
  `foreground.rs`) pass `false`.
- **Calibration:** none. Every check was run before this verdict.
  - Independent re-run: 2410 passed / 0 failed (the pinned count exactly);
    fmt and clippy clean; the reviewer's self-check matches
    `/tmp/e2e-04d.txt`; no production `unwrap`/`expect`.
  - The standards grep shows one `#[allow(clippy::too_many_arguments)]`
    line. It is the **existing** allow on `prompt_tool_call_ratatui`, with
    its comment changed from "8 params" to "9 params" (a `-`/`+` pair in the
    diff). The function has 9 parameters, so the allow is not dead.
  - Classification is untouched: no diff line touches
    `prompt_classification`, `classify` or `is_safe`.
  - F10: one leading `Error: ` is stripped before the wrapper adds its own,
    and the cap refusal (no prefix) is unchanged.
