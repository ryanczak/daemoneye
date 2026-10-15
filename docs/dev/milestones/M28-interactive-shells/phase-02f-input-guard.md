# Phase 02f: input-guard — `send_input` sees what holds the shell

**Milestone:** M28 — Interactive shells
**Status:** review
**Depends on:** phase-02e
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

The `send_input` approval panel describes the shell from its coarse
`ShellState` (`Idle` / `Running` / …). It cannot say that the program
reading the keys is `claude` in raw mode, and it shows nothing of the
screen the keys will land on. A ghost that the `ToolPolicy` allows to use
`send_input` may type into any shell, including a full-screen program.
This phase classifies the shell with the live foreground probe **before**
the panel. The panel names what holds the shell and, for a TUI, shows the
last five rows of its screen, masked. A ghost may type only into a shell at
its prompt. The runbook opt-in that lifts that restriction
(`allow_tui_input`) is phase 02g.

## Architecture references

- `docs/design/interactive-shells.md` D9: the guard table. Chat is always
  allowed and the panel informs. A ghost is refused everywhere except
  `Prompt`, until 02g adds the opt-in.

## Spec

**Files:** `src/daemon/executor/mod.rs` (the `SendInput` arm and a helper
next to `ghost_may_send_input`) and `src/daemon/executor/knowledge/shell.rs`.
Do not touch `src/shell/`, the runbook parser, `GhostConfig` or
`GhostPolicy`; that is phase 02g.

1. **The order in the `SendInput` arm** becomes:
   1. the existing `ToolPolicy` gate (unchanged);
   2. `crate::shell::probe_foreground(crate::shell::registry(), &id, crate::shell::PROBE_TIMEOUT)`.
      An id that does not parse with `ShellId::parse` classifies as
      `Foreground::Unknown`;
   3. **the ghost gate**: if `is_ghost` and `!ghost_may_type_into(&fg)`,
      return `ghost_input_refusal(shell, &fg)` as the tool result, with no
      prompt and nothing sent;
   4. the existing screen read for the modes. Keep its `ScreenText.text`
      for the tail;
   5. `compose_input` (unchanged);
   6. the approval panel from `input_panel(...)` (Task 3);
   7. send (unchanged).

   The `crate::shell::status` call that fed `input_target_note` goes away.
2. **`ghost_may_type_into(fg: &Foreground) -> bool`** (`executor/mod.rs`):
   true only for `Foreground::Prompt`. **`ghost_input_refusal(shell, fg) -> String`**
   returns `Error: …`. It names the shell id and `fg`'s `Display` (e.g.
   `running claude (TUI)`), says a ghost may type only into a shell at its
   prompt, and says nothing was sent.
3. **The panel** (`knowledge/shell.rs`):
   - `input_target_note` takes `&Foreground` instead of
     `Option<ShellState>`. `Prompt`: the input WILL run as a shell command
     (keep today's wording). `Program { tui: false }`: it goes to `<cmd>`'s
     stdin and is not run as a command. `Program { tui: true }`: it goes to
     the raw-mode TUI `<cmd>`, as keystrokes. `Paused`: the job is stopped,
     so the input sits unread. `Exited` / `Unknown`: the shell may be gone.
   - `screen_tail(text: &str, rows: usize) -> String`: drop trailing blank
     rows, keep the last `rows` rows, and pass them through `mask_sensitive`.
   - `input_panel(shell, text, input: &ComposedInput, fg: &Foreground, screen: Option<&str>) -> String`:
     today's panel string (text, keys, bracketed paste, Enter) followed by a
     `foreground: <fg> — <note>` line. Then, **only** when `fg` is
     `Program { tui: true, .. }` and `screen` is `Some`, a
     `screen (last 5 rows):` line and `screen_tail(screen, 5)`. The
     executor arm passes its screen text as `screen`.
4. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-02f.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds **only**
   the output fence, followed by the `PASTE MATCH` line.

**Gotchas.**
- The one test whose assertions change is
  `input_target_note_names_the_prompt_hazard_when_idle`. It is **renamed**
  to `input_target_note_names_each_foreground` and covers all six
  `Foreground` cases. That list is complete.
- Masking: `mask_sensitive` has no `sk-ant-` pattern. For the masking case,
  use `AKIAIOSFODNN7EXAMPLE`, which becomes `<AWS_KEY>`.
- The ghost gate goes **after** the `ToolPolicy` gate: a ghost the policy
  denies must still get the existing policy refusal, not the new one.

## Tests

- `input_target_note_names_each_foreground` (rename, `knowledge/shell.rs`):
  `Prompt` mentions the shell command, `Program { tui: false }` names the
  cmd and stdin, `Program { tui: true }` names the cmd and TUI, `Paused`
  says stopped, and `Exited` / `Unknown` say gone.
- `screen_tail_keeps_the_last_rows_masked`: 8 rows followed by 3 blank
  rows ⇒ exactly rows 4–8. A row holding `AKIAIOSFODNN7EXAMPLE` comes out
  with `<AWS_KEY>`.
- `input_panel_shows_the_foreground_and_a_tui_screen_tail`:
  `Program { cmd: "claude", tui: true }` with a screen ⇒ the panel contains
  `foreground: running claude (TUI)` and the tail's rows. `Prompt` with the
  same screen ⇒ a `foreground: shell prompt` line and **no** `screen (`
  line.
- `ghost_may_type_only_at_a_prompt` (`executor/mod.rs` tests): `Prompt` is
  true; both `Program`s, `Paused`, `Exited` and `Unknown` are false.
- `ghost_input_refusal_names_the_shell_and_what_holds_it`: the refusal for
  `s4` and `Program { cmd: "vim", tui: true }` contains `s4`,
  `running vim (TUI)` and `nothing was sent`.

## Acceptance criteria

- [ ] The 4 new tests and the renamed one ⇒ `test result: ok. 1 passed`
      each.
- [ ] `grep -n 'ShellState' src/daemon/executor/knowledge/shell.rs | grep -c input_target_note`
      ⇒ `0`.
- [ ] `cargo test` ⇒ `passed=2376 failed=0`: exactly 2372 at `1e43829f`
      plus the 4 new tests. The rename adds nothing.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02f.txt; : > "$A"
for t in input_target_note_names_each_foreground screen_tail_keeps_the_last_rows_masked input_panel_shows_the_foreground_and_a_tui_screen_tail ghost_may_type_only_at_a_prompt ghost_input_refusal_names_the_shell_and_what_holds_it ghost_may_send_input_requires_an_explicit_allow; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then the self-check:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02f-input-guard.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02f.txt
diff /tmp/pasted-02f.txt /tmp/e2e-02f.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The runbook `allow_tui_input` field, its path through `GhostConfig` and
  `GhostPolicy`, and the docs naming it: phase 02g.
- Refusing chat input to any foreground. In chat the human decides.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-14 19:17 (started)

Executor: Claude (M28 phase-02f run). Status flipped to `in-progress`; implementing the Spec in order — the `SendInput` arm's probe → ghost gate order and the `crate::shell::status` call's removal, `ghost_may_type_into` and `ghost_input_refusal` next to `ghost_may_send_input`, then `input_target_note(&Foreground)` / `screen_tail` / `input_panel` in `knowledge/shell.rs`, then the test rename and the end-to-end capture.

### Update — 2026-10-14 19:17 (end-to-end verification)

```text
input_target_note_names_each_foreground: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_tail_keeps_the_last_rows_masked: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
input_panel_shows_the_foreground_and_a_tui_screen_tail: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
ghost_may_type_only_at_a_prompt: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
ghost_input_refusal_names_the_shell_and_what_holds_it: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
ghost_may_send_input_requires_an_explicit_allow: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2376 failed=0
```

PASTE MATCH

### Update — 2026-10-15 02:36 (complete, server-authored)

**Summary:** **Summary:** Phase 02f implemented and committed (`d3abdf2d`); working tree clean, status left `in-progress` with no `(complete)` entry. The `SendInput` arm now runs: policy gate → live `probe_foreground` (unparseable id ⇒ `Foreground::Unknown`) → the new ghost gate → the screen read (kept for the TUI tail) → `compose_input` → `input_panel` → send; the `crate::shell::status` call and its `input_target_note(Option<ShellState>)` form are gone. `ghost_may_type_into` (true only for `Prompt`) and `ghost_input_refusal` (names the shell id, the `Foreground` Display, and "nothing was sent") sit next to `ghost_may_send_input`. In `knowledge/shell.rs`, `input_target_note` takes `&Foreground` with the six D9 wordings, `screen_tail` drops trailing blank rows / keeps the last `rows` / masks, and `input_panel` appends `foreground: <fg> — <note>` plus — only for `Program { tui: true }` with a screen — the `screen (last 5 rows):` tail. The renamed test `input_target_note_names_each_foreground` covers all six cases; the four new tests pass individually.

**Notes for review:** E2E ran verbatim, `PASTE MATCH` confirmed via the self-check diff, and the paste is in its own `end-to-end verification` Update Log entry; full suite `passed=2376 failed=0` (2372 baseline + the 4 new tests, rename adds nothing), `doc_truth` 16/0, clippy `-D warnings` clean, `fmt_exit=0`, and the `grep -c input_target_note` acceptance grep returns `0`. One test-side fix worth noting: my first assert needle for the screen line omitted the closing paren (`screen (last 5 rows:` vs the spec's `screen (last 5 rows):`), which is what caught the bug before anything shipped — the panel string itself was correct. The refusal text says "may type only into a shell at its prompt" rather than naming `allow_tui_input` (that field arrives in 02g; 02f's out-of-scope section excludes it). The screen read for the tail is the same `ScreenText` read that supplies the compose modes — one round-trip, as before.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.22s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s


TEST
st readme_tools_counts_are_accurate ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
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
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.08s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (4004921): No such process
kill: (-4005231): No such process
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
- `docs/dev/milestones/M28-interactive-shells/phase-02f-input-guard.md` — +19 -0
- `src/daemon/executor/knowledge/mod.rs` — +2 -1
- `src/daemon/executor/knowledge/shell.rs` — +154 -33
- `src/daemon/executor/mod.rs` — +133 -50

**Commit:** d3abdf2da2f7fa27c98db06a373382fd8bbd59df

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
t_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test resume_run_request_roundtrip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
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
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (4146877): No such process
kill: (-4147607): No such process
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
- `docs/dev/milestones/M28-interactive-shells/phase-02f-input-guard.md` — +22 -1
- `src/daemon/executor/knowledge/mod.rs` — +1 -1
- `src/daemon/executor/knowledge/shell.rs` — +164 -45
- `src/daemon/executor/mod.rs` — +122 -44

**Commit:** d3abdf2da2f7fa27c98db06a373382fd8bbd59df

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
