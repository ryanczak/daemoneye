# Phase 11 — Test reliability

**Status:** review
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Depends on:** phase-10
**Estimated diff:** ~190 lines
**Tags:** language=rust, kind=test, size=m
**Findings:** M31 `FINDINGS.md` #65, #83; M32 `FINDINGS.md` #11
(#5, #6, #51, #53 and #54 move to phase 11b)

## Goal

Two flakes and one coverage gap in the suite.

1. **M32 #11** — `for_each_streams_across_segments_in_order` and
   `for_each_skips_unparseable_lines` read every event segment under
   `HOME` and have failed four times with an event they did not write.
   The writers are four tests that call `log_event` without holding the
   HOME lock, so their event lands in whichever temp `HOME` another test
   has set at that moment. Found by instrumenting `log_event` and running
   the lib suite once:

   ```
   UNGUARDED log_event event=task_panic  thread=daemon::tests::supervise_restarts_on_panic
   UNGUARDED log_event event=file_create thread=daemon::executor::file_ops::ops::tests::the_local_write_path_is_unchanged
   UNGUARDED log_event event=command     thread=daemon::executor::foreground::tests::ghost_turn_without_a_pty_shell_is_refused
   UNGUARDED log_event event=command     thread=daemon::executor::foreground::tests::chat_command_without_session_is_refused
   ```

   After this phase the four tests take `crate::test_home()`, and
   `log_event` asserts under `cfg(test)` that the calling thread holds the
   lock — so the next unguarded writer fails by name, in its own test,
   instead of as a flake in an event-log test.
2. **M31 #65** — `read_shell_screen_lines_tails_the_visible_rows` reads
   the live screen twice: once to count the rows to tail, once with that
   tail. The header says `shell prompt` as soon as bash is back at its
   prompt, before the prompt row is drawn, so a redraw between the two
   reads changes the count. After this phase the test polls one read, with
   the tail under test (`lines: 2`), until that read shows `row-c` as its
   first row.
3. **M31 #83** — no test drives `run_terminal_command`'s approved path
   through `execute_tool_call`. After this phase two tests do: an approved
   local command runs in the session's PTY shell and the result carries
   its output, and a malformed host name is refused before any prompt.

The suite goes from **2607** to **exactly 2609**: two tests added, none
removed.

## Architecture references

- `src/lib.rs` § `test_home_guard` / `TestHomeGuard`: the lock the new
  assertion reads.
- `src/daemon/executor/dispatch_foreground.rs`: the arm the #83 tests
  drive; `src/daemon/executor/mod.rs` § the `tests` module, whose
  `get_shell_context_without_chat_pane_is_shells_only` is the existing
  `execute_tool_call` fixture the new tests follow.
- M32 README, decisions 1 and 3.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11-test-reliability.py && cargo fmt --all
   ```

   The script does the whole change:
   - `src/lib.rs`: a `#[cfg(test)]` thread-local `HOLDS_HOME_GUARD`, set
     by `test_home_guard()` and cleared by `TestHomeGuard::drop`;
   - `src/daemon/utils/event_log.rs`: `log_event` asserts it under
     `cfg(test)`, naming the event and the fix;
   - the four tests above take `crate::test_home()` as their first line;
   - `read_shell_screen_lines_tails_the_visible_rows` is rewritten to one
     read per poll;
   - `dispatch_foreground.rs` gains a `mod approved_path_tests` with its
     own in-process PTY host spawn and the two #83 tests.

   **Do not open the script, and do not read any source file before
   running it.** Every line of code it writes was compiled, linted and
   tested on a scratch worktree of `a46472b6`: fmt and clippy clean, the
   full suite at `passed=2609 failed=0`. Checking that it will compile is
   the compiler's job, and it already did it. The script asserts its own
   anchors and prints four lines, the last
   `two approved-path tests added`. If an assert fires, stop and report
   it in the Update Log. Do not hand-edit anything it touches, and do not
   edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `test:`.

### The red runs are done — do not repeat them

The assertion is the regression gate for #11, and it was run red before
the four guards went in: with the assertion alone, the four tests fail,
each naming its event (`log_event("command") from a test that does not
hold the HOME lock …`), `0 passed; 4 failed`. The script applies the
assertion and the guards together, so your run is green. #65 is a race
the fix removes structurally; 25 runs of the old test passed alone, and
25 of the new one. The #83 run test was mutation-checked: dropping the
`[ran in shell …]` line from `with_shell_line` fails it on
`the result names the shell it ran in`. Do not repeat any of this.

### The script

`docs/dev/milestones/M32-findings-fixes/scripts/phase-11-test-reliability.py`
(sha256 begins `3097d6e61978f5f4`). It is kept in the repo as the record of the change.
Run it; there is nothing in it to review.

## Acceptance criteria

- [ ] The End-to-end block prints `assert=1 guarded=4 approved_tests=2
      single_read=1` (today: `assert=0 guarded=0 approved_tests=0
      single_read=0`).
- [ ] `cargo test` reports **exactly** `passed=2609 failed=0` (today
      2607).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "assert=$(grep -c 'HOLDS_HOME_GUARD.with(|c| c.get())' src/daemon/utils/event_log.rs) guarded=$(grep -A1 -E 'async fn (supervise_restarts_on_panic|the_local_write_path_is_unchanged|ghost_turn_without_a_pty_shell_is_refused|chat_command_without_session_is_refused)\(\)' src/daemon/mod.rs src/daemon/executor/foreground.rs src/daemon/executor/file_ops/ops.rs | grep -c 'crate::test_home()') approved_tests=$(grep -cE 'async fn (an_approved_local_command_runs_in_the_sessions_shell|a_malformed_host_is_refused_without_asking)\(\)' src/daemon/executor/dispatch_foreground.rs) single_read=$(grep -c 'screen_until(&reg, &id, Some(2), None' src/daemon/executor/knowledge/shell_observer_tests.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `assert=1 guarded=4 approved_tests=2 single_read=1`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, `passed=2609 failed=0`. If
the suite stops early on a failure, paste that output, run the block once
more and paste both.

## Authorizations

- Run `scripts/phase-11-test-reliability.py`; do not edit it.
- Edit the seven files the script names, through the script only:
  `src/lib.rs`, `src/daemon/utils/event_log.rs`, `src/daemon/mod.rs`,
  `src/daemon/executor/foreground.rs`,
  `src/daemon/executor/file_ops/ops.rs`,
  `src/daemon/executor/knowledge/shell_observer_tests.rs`,
  `src/daemon/executor/dispatch_foreground.rs`.

## Out of scope

- #5, #6, #51, #53, #54: phase 11b (test-tidies).
- The event-log tests' own `with_test_home` helper (a closure form, not
  the phase-10 fixture's shape).
- Any test that the new assertion does **not** fail. The dry run ran the
  whole lib suite under it: exactly the four tests above fail, no other.

## Update Log

<!-- entries appended below this line -->
### Notes for executor — 2026-10-30

The first run (session `6ae40c6b`) stopped after 120 read-only turns. It
never flipped the status and never ran the script. From turn 30 on it was
checking whether the script's test code would compile, for example
whether `std::io::Cursor` implements `AsyncBufRead`. It does; the code was
compiled and the full suite run against it before this doc was written.
The script is now a file, so step 1 is one command. Your whole phase is:
the started entry, that command, the cuts-done entry, `git diff --stat`
for Findings, the End-to-end block, the commit. Read no source file.
Green gates are the expected outcome.

### Update — 2026-10-30 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the stall was verification of already-verified code, a
spec-shape problem; moving the script into a file and forbidding the
read removes it, which resume would carry forward and takeover would
not test.

### Update — 2026-10-30 13:32 (started)

Executing phase 11 (test reliability). Status flipped to in-progress in
this doc and in the M32 README phase table. Next: the started entry done,
running the script per Spec step 1.

### Update — 2026-10-30 13:34 (cuts done)

```
log_event asserts the HOME lock under cfg(test)
four tests take crate::test_home()
read_shell_screen_lines_tails_the_visible_rows reads once per poll
two approved-path tests added
```

`python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11-test-reliability.py && cargo fmt --all` exited 0; all four script lines printed, no assert fired.

### Findings — 2026-10-30

`git diff --stat`: 9 files, 208 insertions, 25 deletions — the seven
source files the script names, plus this doc and the M32 README's status
flip. Nothing unexpected.

### Update — 2026-10-30 13:37 (end-to-end verification)

```
assert=1 guarded=4 approved_tests=2 single_read=1
allowlist=unchanged
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.97s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.43s
fmt_exit=0
passed=2609 failed=0
```

### Update — 2026-10-30 13:39 (complete, server-authored)

**Summary:** </think>Everything is done. Checklist:

- [x] Phase doc Status: in-progress ✓ (I flipped it, did not flip to review)
- [x] Milestone README row: in-progress ✓
- [x] No (complete) entry written by me ✓
- [x] cargo fmt --all run (as part of step 1 command) ✓
- [x] All verification commands ran clean ✓
- [x] Final message: Summary + Notes for review ✓ (about to write)
- [x] git status clean ✓
- [x] git log -1 --stat shows all files ✓

Acceptance criteria:
- End-to-end block prints assert=1 guarded=4 approved_tests=2 single_read=1 ✓
- cargo test passed=2609 failed=0 ✓
- hygiene 16 passed, bug_tracker 6 passed, allowlist unchanged, fmt/clippy clean ✓
- grep -c '^### Findings — ' = 1 ✓, e2e entry has fenced output ✓

Now write the final Summary + Notes for review.</think>
**Summary:** Phase 11 executed exactly per spec. I flipped the phase doc and M32 README row to `in-progress`, appended the started entry, then ran the phase script + `cargo fmt --all` as my first code command — it printed all four lines (`log_event asserts the HOME lock under cfg(test)`, `four tests take crate::test_home()`, `read_shell_screen_lines_tails_the_visible_rows reads once per poll`, `two approved-path tests added`) with no assert fired. Cuts-done, Findings (`git diff --stat`: 9 files, +208/−25 — the seven source files plus the two doc status flips), and end-to-end entries are appended to the phase doc's Update Log, and everything is committed as `test:` (`321232fa`); the working tree is clean.

**Notes for review:** The End-to-end block ran in one pass with the expected output: `assert=1 guarded=4 approved_tests=2 single_read=1`, `allowlist=unchanged`, hygiene `16 passed`, bug tracker `6 passed`, clippy clean, `fmt_exit=0`, and `passed=2609 failed=0` — exactly the 2609 the spec requires (2607 + 2). `grep -c '^### Findings — '` on the phase doc prints `1`, and the fenced E2E output is in the verification entry. No deviations from the spec; the script was run unmodified and no source file was opened or hand-edited. Per the lifecycle split, I left the status at `in-progress` and did not write a `(complete)` entry — that tail is yours.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.4.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.55s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.25s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test config_pricing_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (449904): No such process
kill: (-450578): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-084f36955f09af75)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/phase-11-test-reliability.md` — +36 -1
- `src/daemon/executor/dispatch_foreground.rs` — +159 -0
- `src/daemon/executor/file_ops/ops.rs` — +1 -0
- `src/daemon/executor/foreground.rs` — +2 -0
- `src/daemon/executor/knowledge/shell_observer_tests.rs` — +7 -23
- `src/daemon/mod.rs` — +1 -0
- `src/daemon/utils/event_log.rs` — +6 -0
- `src/lib.rs` — +13 -0

**Commit:** 321232fa8ee69babcd799ee4b9d4df832c5d00cf

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
