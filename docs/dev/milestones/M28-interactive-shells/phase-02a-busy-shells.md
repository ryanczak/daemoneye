# Phase 02a: busy-shells — never write a command into a busy shell

**Milestone:** M28 — Interactive shells
**Status:** done
**Depends on:** phase-01c
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Today a bare `run_terminal_command` reuses the owner's lowest-numbered shell
even when a TUI such as Claude Code holds it, and `shell: "<id>"` writes the
marker-wrapped command into whatever holds that shell. This phase probes
the shell's live state first: reuse skips a busy shell, and a named busy
shell is refused before any byte is written. This is the design's D5 and
the live-probe half of D4.

## Architecture references

- `docs/design/interactive-shells.md` D3 (the transport rule), D4 ("decisions
  probe live"), D5 (the refusal and its wording), § 5 (the matrix).

## Spec

**Three files only:** `src/shell/foreground.rs`, `src/shell/session.rs`,
`src/daemon/executor/shell_exec.rs`. No tool is added, so **no
count-pinned test moves**. Do not read `src/ai/`, `CLAUDE.md`, `README.md`
or the prompt assets.

1. **Narrow the prompt digit rule** — in `src/shell/foreground.rs`,
   `is_prompt_line`'s "not preceded by a digit" exclusion applies to `%`
   **only**. bash's default prompt on a host with no rc file is
   `bash-5.2$ `, which the current rule wrongly reads as busy; sbx
   sandboxes and minimal hosts show it. Extend
   `classify_transport_prompt_rule`: `bash-5.2$ ` and `sh-5.2# ` must
   match; `Progress 50%` and `100%` must still not.
2. **One live probe** — in `src/shell/session.rs`,
   `pub async fn probe_foreground(reg: &ShellRegistry, id: &ShellId,
   timeout: Duration) -> Foreground`: `crate::shell::status` over the
   shell's socket; for a transport record (named host or sandbox) also
   `crate::shell::screen` for the last non-blank row; then
   `classify_record` and store the result as the record's
   `last_foreground`. Any socket error ⇒ the status/row is `None`, which
   classifies `Unknown`. **A transport shell's screen is blank until its
   prompt draws**, and a blank screen has no last row, which the transport
   rule reads as busy. So for a transport record, while the screen has no
   non-blank row, re-read it up to 10 times, 100 ms apart, before
   classifying. A screen still blank after that classifies as the rule
   says (busy). **Never hold the registry lock across an
   `.await`**: copy what you need out, probe, then lock again to classify
   and store.
3. **Reuse skips a busy shell** — in `shell_for_owner` and
   `shell_for_owner_in_sandbox`, the candidates are the owner's matching
   live shells in index order (today only the lowest is considered). Take
   the first whose `probe_foreground` is `Foreground::Prompt`; if none is,
   fall through to create a new shell. A busy shell is skipped, never
   removed. Use a 2 s probe timeout.
4. **A named busy shell is refused** — in `run_in_shell_core`
   (`shell_exec.rs`), after the existing exited / host checks and before
   the interactive-command check, probe a shell the caller named. If it is
   not `Prompt`, re-probe up to two more times 150 ms apart (a job that
   was just interrupted can read `Running` for a moment); if it is still
   not `Prompt`, refuse **without writing anything**:
   `Error: shell <id> is <Foreground>; a command written there would be
   typed into it. Use send_input to type into that program,
   read_shell_screen to see it, or shell: "new" for a fresh shell.` For
   `Unknown` say the shell's state could not be read instead of naming a
   program.
5. **Capture the end-to-end evidence** — run the block in § End-to-end
   verification verbatim and paste `/tmp/e2e-02a.txt` into a new Update Log
   entry headed `### Update — <date> (end-to-end verification)`, ending with
   the `PASTE MATCH` line. The server-authored `(complete)` entry does not
   satisfy this.

**Gotchas.**
- An idle interactive bash reads `raw_mode: Some(true)` (readline). That
  is correct and `classify` ignores it unless the state is `Running`.
- The test fixture is the **real** in-process host: `spawn_in_process_host`
  in `session.rs`'s and `shell_exec.rs`'s test modules (a real bash on a
  real socket). Make a shell busy with `crate::shell::send_input(&socket,
  b"sleep 30\n", …)` and wait until `probe_foreground` reports
  `Program { cmd: "sleep", .. }`; end it with `b"\x03"`.
- `ShellRecord` is not `Clone`; classify inside `with_shells`.

## Tests

- `foreground.rs`: `classify_transport_prompt_rule` (extended per Task 1).
- `session.rs`: `probe_foreground_reads_a_prompt_on_an_idle_shell`,
  `probe_foreground_names_a_running_program`,
  `shell_for_owner_skips_a_busy_lowest_shell` (the owner's only shell is
  busy ⇒ a new shell is returned and the busy one is still in the
  registry), `shell_for_owner_reuses_the_next_idle_shell` (lowest busy,
  next idle ⇒ the idle one).
- `shell_exec.rs`: `run_terminal_command_refuses_a_busy_named_shell` (the
  error names the shell and `running sleep`; afterwards the `sleep` is
  still running and the refused command's text never appears on the
  shell's screen).

**Preservation — these existing tests target a named shell and must pass
unmodified:** `run_terminal_command_refuses_a_shell_whose_host_differs_from_the_named_host`,
`run_terminal_command_with_a_matching_shell_and_host_is_not_an_error`,
`an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable`,
`an_answer_after_the_timeout_still_completes_the_command`,
`a_hang_after_an_answer_still_times_out`,
`a_sandbox_shell_targeted_by_id_uses_the_posix_marker`, and every
`session_*` / `shell_for_owner_*` test. If one fails, the probe or the
grace is wrong; do not change the test.

## Acceptance criteria

- [ ] The six tests in § Tests pass (`cargo test --lib <name>` ⇒ `test
      result: ok. 1 passed`).
- [ ] `cargo test` ⇒ `passed=2357 failed=0` — exactly 2352 at `87abb8ee`
      plus the five new tests (the extended one is not new).
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The preservation tests above pass unmodified.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02a.txt; : > "$A"
for t in classify_transport_prompt_rule probe_foreground_reads_a_prompt_on_an_idle_shell probe_foreground_names_a_running_program shell_for_owner_skips_a_busy_lowest_shell shell_for_owner_reuses_the_next_idle_shell run_terminal_command_refuses_a_busy_named_shell an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable a_sandbox_shell_targeted_by_id_uses_the_posix_marker; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run the full `cargo test` line **once**. If it reports failures, re-run at
most twice more, then paste the last capture as it is and name every
failing test; a test this phase added is the first suspect.

Then the self-check, whose output line goes into the entry:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02a-busy-shells.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02a.txt
diff /tmp/pasted-02a.txt /tmp/e2e-02a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Removing the static `interactive` flag, and showing the foreground in
  `list_shells` / `[SHELLS]` — phase 02b.
- Anything in `send_input` — phases 02c and 02d.
- Refactoring `read_shell_screen` onto `probe_foreground` — later.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-14 (second resume)

The resumed run implemented Task 4 correctly, then stalled while verifying
by running test suites module by module. The governor counts each test run
as no progress, so 120 of them end the run. **Do not run module test
suites. Run only the commands named below.**

**Done and correct — do not redo or re-verify:** Tasks 1–4, all the
`session.rs` work, the refusal in `shell_exec.rs`, the restored
preservation test. Clippy is clean and five of the six phase tests pass.

**Exactly three edits remain:**

1. Run `cargo fmt --all` (one import list in `src/shell/mod.rs` is
   unformatted).
2. Write `run_terminal_command_refuses_a_busy_named_shell` in
   `src/daemon/executor/shell_exec.rs`'s test module, per § Tests. Use the
   module's `spawn_in_process_host`; make the shell busy with
   `crate::shell::send_input(&socket, b"sleep 30\n", …)` and wait until
   `probe_foreground` reports `Program { cmd: "sleep", .. }`. Run it once
   with `cargo test --lib run_terminal_command_refuses_a_busy_named_shell`.
3. Run the § End-to-end verification block **once**, paste it, add the
   `PASTE MATCH` line.

Finish: the block's last line reads `passed=2357 failed=0`.

### Update — 2026-10-14 (escalation, second)

**Chosen lever:** resume
**Rationale:** Tasks 1–4 are implemented and green, and the second stall was verification run as dozens of separate test invocations rather than a spec gap; three bounded edits remain.

### Notes for executor — 2026-10-14 (resume)

The first run stalled after 305 actions. It was right to chase the failing
named-host reuse test: **the spec was missing a rule**, now added to Task
2. A freshly spawned transport shell has a blank screen until its prompt
draws, and the transport rule reads blank as busy. That is why
`shell_for_owner_reuses_a_shell_on_the_requested_host` and
`shell_for_owner_in_sandbox_creates_then_reuses` fail. Do not investigate
further; implement the blank-screen re-read in Task 2.

**Already done and correct — do not redo:** Task 1 (the `%`-only digit
rule and the extended prompt test), Task 2's `probe_foreground` apart from
the re-read, Task 3's reuse change, and four of the five new tests.

**Exactly this remains, in order:**

1. Remove every `eprintln!("DBG …")` from `src/shell/session.rs`, and
   restore `shell_for_owner_reuses_a_shell_on_the_requested_host` to its
   original body: delete the `probe_foreground` / `screen` debug block and
   the commented-out `assert_eq!` you added. `git diff src/shell/session.rs`
   shows them. Preservation tests are not edited.
2. Task 2's blank-screen re-read for transport records.
3. The clippy error: `assert_eq!` with a literal bool (use `assert!`).
4. Task 4 in `src/daemon/executor/shell_exec.rs` and its test
   `run_terminal_command_refuses_a_busy_named_shell`.
5. Task 5, the end-to-end capture and `PASTE MATCH`.

Finish: `cargo test` reports exactly **2357** passed, 0 failed.

Do not write throwaway scripts to `/tmp` to run `true` or to print files;
call the tool you need directly.

### Update — 2026-10-14 (escalation)

**Chosen lever:** resume, after correcting the spec
**Rationale:** Tasks 1–3 are on disk and correct apart from one rule the spec omitted (a fresh transport shell's blank screen reads as busy); with that rule added, a resumed run keeps the work and has five enumerated items left.

### Update — 2026-10-14 (started, third resume)

Resumed by Claude (third run). Tasks 1–4 are on disk from the prior runs and
left untouched; the remaining work is exactly the three edits the second-resume
note enumerates: `cargo fmt --all`, the test `run_terminal_command_refuses_a_busy_named_shell`,
and the end-to-end capture with its `PASTE MATCH` line.

### Update — 2026-10-14 (end-to-end verification)

```sh
A=/tmp/e2e-02a.txt; : > "$A"
for t in classify_transport_prompt_rule probe_foreground_reads_a_prompt_on_an_idle_shell probe_foreground_names_a_running_program shell_for_owner_skips_a_busy_lowest_shell shell_for_owner_reuses_the_next_idle_shell run_terminal_command_refuses_a_busy_named_shell an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable a_sandbox_shell_targeted_by_id_uses_the_posix_marker; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

`/tmp/e2e-02a.txt` contents, pasted verbatim from the single run:

```
classify_transport_prompt_rule: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
probe_foreground_reads_a_prompt_on_an_idle_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
probe_foreground_names_a_running_program: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
shell_for_owner_skips_a_busy_lowest_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
shell_for_owner_reuses_the_next_idle_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
run_terminal_command_refuses_a_busy_named_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
a_sandbox_shell_targeted_by_id_uses_the_posix_marker: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2295 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2357 failed=0
```

Self-check: the fenced block between lines 15–25 of the entry reproduces
`/tmp/e2e-02a.txt` byte for byte; `diff /tmp/pasted-02a.txt /tmp/e2e-02a.txt`
exits 0.

**PASTE MATCH**

### Update — 2026-10-14 16:35 (complete, server-authored)

**Summary:** </think>All tasks done. The completion checklist:

- [x] Phase doc's Status: line says `in-progress` — yes, I flipped it at start, did NOT flip to review.
- [x] Milestone README phase-table row says `in-progress` — yes.
- [x] I did NOT hand-write a `(complete)` Update Log entry — correct, only started entry + end-to-end verification entry.
- [x] `cargo fmt --all` was run immediately before `git add` — yes.
- [x] All verification commands ran clean — clippy clean, fmt check exit 0, tests passed=2357 failed=0.
- [x] Final message is a substantive Summary + Notes for review — about to write.
- [x] `git status --short` shows nothing — confirmed.
- [x] `git log -1 --stat` shows the commit includes every file touched — yes, 6 files.

Now write the final Summary + Notes for review (2-6 sentences).</think>
**Summary + Notes for review**

Third resume completed the three remaining edits: (1) `cargo fmt --all` (one import list in `src/shell/mod.rs`), (2) the new test `run_terminal_command_refuses_a_busy_named_shell` in `src/daemon/executor/shell_exec.rs`'s test module — it opens the module's real in-process host, makes the shell busy with `send_input(&socket, b"sleep 30\n", …)` until `probe_foreground` reports `Program { cmd: "sleep", .. }`, then asserts the refusal names the shell id, `running sleep`, and D5's remedy wording; that the status still reads `Running` and the screen contains no trace of `BUSY_REFUSAL_MUST_NOT_RUN` afterwards; and that after `\x03` the same named shell runs a follow-up command (the refusal left it untouched) — and (3) the § End-to-end verification block, run once, pasted into a new `### Update — 2026-10-14 (end-to-end verification)` entry. All six phase tests pass, clippy `-D warnings` is clean, `cargo fmt --all --check` exits 0, and the full suite reports **passed=2357 failed=0** (the 2352 baseline plus the five new tests). The entry's self-check shows `diff /tmp/pasted-02a.txt /tmp/e2e-02a.txt` ⇒ **PASTE MATCH**. Everything (code plus the start-of-phase status flip, the started entry, and the evidence entry) is committed as `1f7baddb`; `git status --short` is clean and the phase doc still reads `in-progress` for the server's completion tail. No spec deviations beyond the blank-screen re-read rule the architect added mid-phase; Tasks 1–4's on-disk work was left as the prior run left it.

Status: phase-02a implementation complete, ready for review.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.18s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
st readme_tools_counts_are_accurate ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test config_pricing_round_trip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

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
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (3340214): No such process
kill: (-3340952): No such process
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
- `docs/dev/milestones/M28-interactive-shells/phase-02a-busy-shells.md` — +41 -1
- `src/daemon/executor/shell_exec.rs` — +125 -1

**Commit:** 1f7baddb7ed1f54d8f5c592cd7dd54ab34acb4e3

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-14

- **Verdict:** approved_first_try
- **Bounces:** none at review. Three executor runs: the first stalled
  (`NoProgressStall`) after Tasks 1–3 on a rule the spec omitted — a fresh
  transport shell's blank screen reads as busy — and ended in a numbered
  `/tmp/dbgN.sh` loop the identical-repetition detector cannot see; the
  first resume implemented Task 4 then stalled verifying by module test
  suites; the second resume did the last three edits in 106 turns.
- **Executor:** GLM-5.3-Flash, all three runs.
- **Gates (reviewer re-run, once each):** fmt exit 0; build 0 warnings;
  clippy `-D warnings` exit 0; `cargo test` passed=2357 failed=0, the
  pinned count. `PASTE MATCH` present.
- **Spot check:** `probe_foreground` holds no lock across an await; the
  blank-screen re-read is bounded at 10 × 100 ms; reuse walks candidates in
  index order and skips, never removes, a busy shell; the digit exclusion
  is `%`-only. All eight preservation tests are byte-identical to
  `70f568fc`. Mutation: disabling the refusal (`if false && …`) turns
  `run_terminal_command_refuses_a_busy_named_shell` red; restored, green.
- **Scope deviations:** none in the final tree. The first run edited a
  preservation test with debug code; the resume restored it.
- **Calibration:** two governor gaps for rexyMCP — near-identical commands
  with a changing filename evade the repetition detector, and verification
  by many separate test runs reads as a no-progress stall. Third
  `continue_phase` client idle-timeout (fold candidate). Held for the M28
  retrospective.
