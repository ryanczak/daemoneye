# Phase 10a — Rehearsal fixes: chat turns in the turn log, idle-shell foreground

**Status:** done
**Milestone:** M30 — Telemetry & dashboard
**Drafted:** 2026-10-22 (from phase 10 rehearsal 1, F1 and F2)

## Goal

Two small findings from the live rehearsal, plus one doc-comment nit. After
this phase, `daemoneye top` and `daemoneye run list` show a chat run's turn
count, and the Shells panel shows `shell prompt` for a shell whose last
waited command has finished.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.7 (`daemoneye top`: Runs show turn,
  Shells show foreground).
- `src/daemon/run/turn_log.rs` — the turn log records
  (`run_started` / `turn_started` / `note` / `run_finished`).
- `src/shell/foreground.rs` — `Foreground` and `classify_record`.

## Pre-flight

- `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`
  and `cargo test` are green at HEAD (`75365eaa`).
- The tests in `src/daemon/run/guard.rs` show how a run's turn log is
  written and read under a temp `HOME` (the `HomeGuard` there).
- The tests in `src/daemon/executor/shell_exec.rs` (for example
  `shell_exec_returns_the_real_exit_code`) show how a real PTY shell is run
  through `run_in_shell_for_test` and how its registry record is read.

## Current state

- **F1.** The chat loop (`run_conversation_loop`, `src/daemon/stream.rs`)
  counts a turn with `run_log.metrics_mut().record_turn()` before every
  model call, so the `run` record's `turns` is right (a `hostname` ask
  recorded `turns: 2`). But it writes no `turn_started` record: the chat
  turn log holds only `run_started` and `run_finished`. The ghost loop
  (`src/daemon/ghost.rs`) writes one through its private
  `emit_turn_started`. `RunSummary.turns` (`src/daemon/run/list.rs`) is the
  highest `turn_started` in the log, so `run list` and `top` show `0` for
  every chat run.
- **F2.** `ShellRecord.last_foreground` is written by exactly two paths:
  `probe_foreground` (`src/shell/session.rs`, only for a caller-named
  shell) and `read_shell_screen`. A waited command's completion — the
  daemon has just seen the END marker, so it knows the shell is back at its
  prompt — records nothing. The Shells panel renders `-` for the common
  idle shell (rehearsal: `s0 chat:… - pinky`).
- **Nit.** `src/cli/commands/top_tui.rs`: the doc comment on `run_tui`
  repeats its first line.

## Spec

1. **One `turn_started` writer.** Move the ghost's `emit_turn_started` to
   `src/daemon/run/turn_log.rs` as a `pub fn emit_turn_started(run_id:
   &RunId, turn: u32)` and re-export it from `src/daemon/run/mod.rs` like
   its neighbours; the ghost call site keeps its behaviour (it passes a
   `usize` today). The chat loop calls it immediately after
   `record_turn()`, with the tally's new `turns` value, so the log and the
   record agree: a chat run whose record says `turns: 2` has
   `turn_started` 1 and 2. Nothing else about the chat loop changes.
2. **A finished waited command records the prompt.** In
   `src/daemon/executor/shell_exec.rs`, `finish_run` is reached only with a
   marker-terminated `CommandOutcome` (the plain path and the sudo path
   both end there). There, store
   `last_foreground = Some((Foreground::Prompt, now_unix()))` on the
   shell's record through `with_shells`, as `probe_foreground` does. A
   no-op when the id is not in the registry. **Must not** be stored on the
   timeout, shell-exited, prompt-given-up or refused-before-write paths:
   those never reach `finish_run`, and the `InFlightGuard` drop is the
   wrong place because it fires on all of them.
3. **Nit.** Delete the duplicated first line of `run_tui`'s doc comment.

## Acceptance criteria

- [x] `cargo test emit_turn_started_appends_the_turn` passes: after
      `emit_turn_started(&id, 1)` and `(&id, 2)` under a temp `HOME`, the
      run's turn log holds two `TurnStarted` records and
      `read_turn_log` reports them in order.
- [x] `grep -c 'emit_turn_started' src/daemon/stream.rs` is at least 1,
      and `grep -c 'fn emit_turn_started' src/daemon/ghost.rs` is 0.
- [x] `cargo test a_finished_command_leaves_the_shell_at_its_prompt`
      passes: run `echo hi` through `run_in_shell_for_test`; the shell's
      record then has `last_foreground` of `Some((Foreground::Prompt, t))`
      with `t` within the test's own wall-clock window.
- [x] `cargo test a_busy_shell_is_not_marked_at_its_prompt` passes: a
      command refused by the busy-shell probe (the shape of
      `run_terminal_command_refuses_a_busy_named_shell`) leaves
      `last_foreground` reading `Program`, not `Prompt`.
- [x] `grep -c 'The live loop. Enters raw mode' src/cli/commands/top_tui.rs`
      is 1.
- [x] Preservation: `cargo test guard_` and `cargo test shells_text_` are
      all green; `cargo test` has no failures.

## Test plan

- Unit, `src/daemon/run/turn_log.rs`: `emit_turn_started_appends_the_turn`.
- Unit, `src/daemon/executor/shell_exec.rs`:
  `a_finished_command_leaves_the_shell_at_its_prompt`,
  `a_busy_shell_is_not_marked_at_its_prompt`.
- Mutation check, stated in the Update Log: remove the `last_foreground`
  write in `finish_run`, confirm the prompt test fails, restore.
- The live effect (`top` TURNS for a chat run, Shells `shell prompt`) is
  checked in phase 10's rehearsal 2, not here.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m30-10a.txt; : > "$A"
for t in emit_turn_started_appends_the_turn a_finished_command_leaves_the_shell_at_its_prompt a_busy_shell_is_not_marked_at_its_prompt; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "stream_emits=$(grep -c 'emit_turn_started' src/daemon/stream.rs)" >> "$A"
echo "ghost_defines=$(grep -c 'fn emit_turn_started' src/daemon/ghost.rs)" >> "$A"
echo "turn_log_defines=$(grep -c 'pub fn emit_turn_started' src/daemon/run/turn_log.rs)" >> "$A"
echo "nit_lines=$(grep -c 'The live loop. Enters raw mode' src/cli/commands/top_tui.rs)" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: three `ok. 1 passed` lines, `stream_emits` ≥ 1, `ghost_defines=0`,
`turn_log_defines=1`, `nit_lines=1`, clippy finished with no warnings,
`fmt_exit=0`, `failed=0`.

## Authorizations

- Edit `src/daemon/run/turn_log.rs`, `src/daemon/run/mod.rs`,
  `src/daemon/ghost.rs` (the move and its call site only),
  `src/daemon/stream.rs` (one call after `record_turn()`),
  `src/daemon/executor/shell_exec.rs`, `src/cli/commands/top_tui.rs`.
- No new dependencies. No change to the `run` record, `RunMetrics`, the
  Shells panel renderer or the IPC types.

## Out of scope

- A `turn_started` for the scheduled wrapper run (it makes no model calls).
- `last_foreground` for `wait: false` dedicated shells and for a command
  that timed out (the shell is still busy; a later probe answers).
- Anything the rehearsal filed as an observation (O1–O5 in phase 10).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-22 10:21 (progress)

Started phase 10a (executor, headless run). Flipped the phase and the M30
README's 10a row to `in-progress`. Working through the Spec in order: 1 the
one `turn_started` writer, 2 the prompt record in `finish_run`, 3 the doc
nit; then the test plan (including the stated mutation check) and the
end-to-end capture.

### Update — 2026-10-22 10:45 (progress)

All three Spec tasks implemented and committed (`fa12c94c`): `emit_turn_started`
moved to `turn_log.rs` (re-exported from `run/mod.rs`, ghost call site
behaviour unchanged), the chat loop calls it after `record_turn()` with the
new tally, `finish_run` stamps `Prompt` through `with_shells`, and the
`run_tui` doc-comment duplicate line is gone. Three new tests written
(`emit_turn_started_appends_the_turn`,
`a_finished_command_leaves_the_shell_at_its_prompt`,
`a_busy_shell_is_not_marked_at_its_prompt`); one pre-existing test's
assumption (`launch_interactive_returns_the_settled_screen`) updated: its
setup command now leaves `Prompt` on the record, so the pre-launch
assertion reads `Prompt` instead of `None`. Mutation check done: replaced
the `finish_run` stamp with `None` — the prompt test failed with
`the record holds the stamp`, restored, it passes again. Full `cargo test`
green (2509 passed, 0 failed). Next: the end-to-end capture entry.

### Update — 2026-10-22 10:45 (end-to-end verification)

Ran the block in § End-to-end verification verbatim, unmodified; the
artifact `/tmp/e2e-m30-10a.txt` was generated mechanically by the block
(`A=/tmp/e2e-m30-10a.txt; : > "$A"`, the three `cargo test <name>` lines,
the four greps, the clippy tail, the `cargo fmt --all --check` exit and
the full-suite awk) and its contents are pasted byte-exact below.

```
emit_turn_started_appends_the_turn: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_finished_command_leaves_the_shell_at_its_prompt: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_busy_shell_is_not_marked_at_its_prompt: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
stream_emits=1
ghost_defines=0
turn_log_defines=1
nit_lines=1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2509 failed=0
```

Expected vs actual: three `ok. 1 passed` lines — present;
`stream_emits` ≥ 1 — got 1; `ghost_defines=0` — got 0; `turn_log_defines=1`
— got 1; `nit_lines=1` — got 1; clippy finished with no warnings — the
tail line is a bare `Finished`, no warnings; `fmt_exit=0` — got 0;
`failed=0` — got 0. Every line of the expected shape is present.

### Update — 2026-10-22 17:47 (complete, server-authored)

**Summary:** </think>The tree is clean. Let me verify against the completion checklist:

- [x] Phase doc Status: `in-progress` (never flipped to review by me)
- [x] Milestone README row: `in-progress`
- [x] No hand-written `(complete)` entry
- [x] `cargo fmt --all` run before staging (yes, ran it before the first commit)
- [x] All verification commands ran clean (fmt=0, build ok, clippy no warnings, test 2509 passed 0 failed)
- [x] Final message will be substantive Summary + Notes for review
- [x] `git status --short` empty
- [x] `git log -1 --stat` includes all touched files

Wait — one thing: I committed the progress + e2e docs entries in a separate `docs:` commit, which is fine ("One conventional commit per logical change"). The contract says step 7 commits code plus start-of-phase flip; step 4-5 Update Log entries are also mine to write. The final docs commit carries the started/progress/e2e entries. Good.

Let me double-check the acceptance criteria one final time:

1. ✅ `cargo test emit_turn_started_appends_the_turn` passes — E2E line confirms.
2. ✅ `grep -c 'emit_turn_started' src/daemon/stream.rs` ≥ 1 (1); `grep -c 'fn emit_turn_started' src/daemon/ghost.rs` = 0.
3. ✅ `cargo test a_finished_command_leaves_the_shell_at_its_prompt` passes.
4. ✅ `cargo test a_busy_shell_is_not_marked_at_its_prompt` passes — leaves `last_foreground` reading `Program`.
5. ✅ `grep -c 'The live loop. Enters raw mode' src/cli/commands/top_tui.rs` = 1.
6. ✅ `cargo test guard_` (29 passed) and `cargo test shells_text_` (2 passed) green; full `cargo test` no failures.

Hmm, one criterion said "run `echo hi` through `run_in_shell_for_test`" — I used `echo hi` ✓.

Now the final message: Summary + Notes for review, 2-6 sentences. Concise but substantive.</think>
Summary + Notes for review:

**Summary:** Phase 10a is implemented and committed on top of `710c28d7`. For F1, `emit_turn_started` moved from `src/daemon/ghost.rs` (private `usize` signature, deleted) to `src/daemon/run/turn_log.rs` as `pub fn emit_turn_started(run_id: &RunId, turn: usize)`, re-exported from `run/mod.rs` beside its neighbours; the ghost call site is behaviour-unchanged, and the chat loop in `src/daemon/stream.rs` now calls it immediately after `record_turn()` with the tally's new `turns` value, so a chat run whose record says `turns: 2` carries `turn_started` 1 and 2. For F2, `finish_run` in `src/daemon/executor/shell_exec.rs` stamps `last_foreground = Some((Foreground::Prompt, now_unix()))` through `with_shells` (a no-op when the id is not in the registry) — reached only on the marker-terminated plain/sudo paths, so the timeout, shell-exited, prompt-given-up and refused-before-write paths never store it. The nit (duplicated first line of `run_tui`'s doc comment) is deleted. Three new tests per the test plan (`emit_turn_started_appends_the_turn`, `a_finished_command_leaves_the_shell_at_its_prompt`, `a_busy_shell_is_not_marked_at_its_prompt`), the stated mutation check was run (stamp replaced with `None` → prompt test failed → restored → passes), and the E2E block ran verbatim with its output pasted byte-exact into the Update Log entry `### Update — 2026-10-22 10:45 (end-to-end verification)`: all expected shapes present (`stream_emits=1`, `ghost_defines=0`, `turn_log_defines=1`, `nit_lines=1`, `fmt_exit=0`, `failed=0`, clippy clean). `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test` are all green (2509 passed, 0 failed). Commits: `fa12c94c` (feat) and `0d927603` (docs entries); working tree clean; phase status left at `in-progress` for the server's flip.

**Notes for review:** Two adaptations to flag. (1) A pre-existing test, `launch_interactive_returns_the_settled_screen`, asserted the record's `last_foreground` was `None` before an interactive launch — the F2 change makes its own setup command leave `Prompt` there, so I updated that assertion to read `Some(Prompt)` (launch still replaces it with `Program`; the post-launch assertion is untouched and passes). This is a necessary consequence of the spec'd behaviour, not a widening. (2) The busy-shell test's final "stamp untouched" assertion compared full tuples and proved flaky when the probe's `now_unix()` second rolled over between the two reads, so the test asserts the refused path leaves `Foreground::Program { cmd: "sleep" }` (the probe's classification) rather than the wall-clock second; the refusal test still proves the refused command never reaches `finish_run`. The live effects (`top` TURNS for a chat run, Shells `shell prompt`) are deferred to phase 10's rehearsal 2 per the spec.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.2.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.26s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST

test e2e_prompt_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_depth_limit_enforced ... ok
test config_pricing_round_trip ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ipc_ask_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (1025669): No such process
kill: (-1026352): No such process
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
- `docs/dev/milestones/M30-telemetry-dashboard/phase-10a-rehearsal-fixes.md` — +53 -1
- `src/cli/commands/top_tui.rs` — +0 -1
- `src/daemon/executor/shell_exec.rs` — +130 -1
- `src/daemon/ghost.rs` — +1 -12
- `src/daemon/run/mod.rs` — +3 -1
- `src/daemon/run/turn_log.rs` — +34 -0
- `src/daemon/stream.rs` — +1 -0

**Commit:** 0d927603efea12eb55f96993bd896f2c4822d96a

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-22

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (163 turns)
- **Scope deviations:** two, both accepted. `emit_turn_started` keeps the
  ghost's `usize` turn parameter instead of the Spec's `u32`, so the chat
  call site casts `turns as usize`. The cast is harmless, but the Spec asked
  for `u32`. The executor also updated one pre-existing assertion in
  `launch_interactive_returns_the_settled_screen` (the record's pre-launch
  foreground now reads `Prompt`, not `None`). That follows directly from F2.
- **Gates (architect re-run):** `cargo fmt --check` exit 0; build and
  clippy `-D warnings` clean; `cargo test` 2509 passed, 0 failed (2506 at
  phase 09, plus the three new tests). The grep criteria read
  `stream_emits=1 ghost_defines=0 turn_log_defines=1 nit_lines=1`.
  Preservation: `guard_` 29 passed, `shells_text_` 2 passed.
- **Test reality:** `a_finished_command_leaves_the_shell_at_its_prompt`
  runs on the default path, where nothing probes, so without the
  `finish_run` stamp its `expect("the record holds the stamp")` fails. That
  matches the executor's stated mutation check.
  `a_busy_shell_is_not_marked_at_its_prompt` proves the refused command
  never stamps `Prompt`.
- **Calibration:** the busy-shell test cannot catch the mutation the Spec
  warned against, a stamp in the `InFlightGuard` drop. The busy refusal
  happens before that guard is built. The timeout and shell-exited paths
  are the ones it would have caught, and no criterion asked for them.
  Next time, put the negative criterion on a path that runs through the
  wrong place.

