# Phase 11 — prompt-ids

**Milestone:** M40 — Security hardening
**Status:** done
**Drafted:** 2026-11-26
**Depends on:** 10 (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Close audit row **S15**. The architect read the code on 2026-11-26.

Each approval-gated tool sends a `Response::*Prompt { id, … }` and then reads
one `Request` line back. The CLI echoes the prompt's `id` in its reply.

- **Two handlers check the id.**
  - The command approval, `approval_reply` in
    `src/daemon/executor/mod.rs`.
  - The file approval, in `src/daemon/executor/file_ops/write.rs`.
  - Both match `id: resp_id, …` with a guard `if resp_id == id`, and any
    other reply is a denial.
- **Nine handlers ignore the id.** Each tests the reply with
  `matches!(…, Some(Request::XResponse { approved: true, .. }))`. Any
  `approved: true` reply of the right variant therefore approves, even
  one answering a different prompt.

  | File | Handlers |
  |---|---|
  | `src/daemon/executor/knowledge/artifacts.rs` | `write_script`, `delete_script`, `write_runbook`, `delete_runbook` |
  | `src/daemon/executor/knowledge/agents.rs` | `create_agent` (uses `ScriptWriteResponse`), `delete_agent` (uses `ScriptDeleteResponse`) |
  | `src/daemon/executor/schedule.rs` | `run_schedule_command`, `remove_schedule` |
  | `src/daemon/executor/dispatch_meta.rs` | `ask_chat_approval`, the ghost spawn |

- **One more ignores it:** `credential_round_trip`
  (`src/daemon/executor/shell_exec_auth.rs`). It matches
  `Request::CredentialResponse { credential, .. }`, so it types a
  credential that was meant for another prompt into the shell.

This grep finds the nine, and must print nothing when the phase is done:

```
grep -rnE 'Response \{ approved: true, \.\. \}' src/daemon
```

`PaneSelectResponse` has no daemon-side reader, so it is out of scope.

## Statement of work

1. **A reply for another prompt is a denial.** At every one of the nine
   approve/deny handlers, a reply whose `id` is not the prompt's id
   counts as a denial. This is exactly what the two checked handlers
   already do.
   - The handler logs one `log::warn!` naming both ids.
   - **Hint:** the nine read loops are identical apart from the
     variant. One shared helper that reads the line and returns the
     `bool` would remove them all, and so would a small `fn` mapping a
     `&Request` to `Option<(&str, bool)>`. Choose what reads best.
   - The EOF, oversized-line and timeout arms each keep their current
     behaviour.
2. **A credential for another prompt is never typed.**
   - In `credential_round_trip`, a `CredentialResponse` whose `id` is not
     the prompt's id gives `PromptReply::GiveUp`.
   - The reason names a mismatched reply, and the command does not run
     elevated.
   - Its credential is still zeroized, as phase 10 left it.

**Constraint:** `src/ipc.rs` is at the 800-production-line cap. Add no
lines to it, and do not edit `tests/hygiene_allowlist.txt`.

**Gotcha: some fixtures will break, on purpose.** A test fixture that
answers with an id other than the prompt's goes red after this change,
because it now sees a denial. **Fix the fixture's id. Never loosen the
check.** The no-test-removed grep below holds you to this.

**Closed file list:**
- the files in the table above;
- `src/daemon/executor/shell_exec_auth.rs` and
  `src/daemon/executor/shell_exec_auth_tests.rs`;
- `src/daemon/executor/mod.rs`, if the helper lives there;
- one new file under `src/daemon/executor/`, if you choose a helper
  module;
- any `*_tests.rs` under `src/daemon/executor/` whose fixture id needs
  fixing.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is an edit adding the red test**
   `delete_schedule_refuses_a_reply_for_another_prompt`, in
   `src/daemon/executor/schedule.rs`'s tests. Its `canned_reader` and
   `test_store` seams are already there.
2. The approve/deny fix.
3. The remaining tests.
4. The credential fix and its test.

**Read limit:** read at most **3** files before your first edit.

**Budget:** you have **1200 turns**; use them efficiently.

## First red test

`delete_schedule_refuses_a_reply_for_another_prompt`, in `schedule.rs`.

- **Setup:**
  - call `delete_schedule` with prompt id `"c1"`;
  - feed it `ScheduleRemoveResponse { id: "other", approved: true }`.
- **Asserts:**
  - the outcome is the denial text;
  - the job is still in the store.
- **Today:** red, because the job is deleted.

Paste the red run.

## Acceptance criteria

- [ ] **First red test:** `delete_schedule_refuses_a_reply_for_another_prompt`
      passes. Paste its red run.
- [ ] **Every approve/deny variant:**
      `an_approval_for_another_prompt_is_a_denial` passes.
      - **Setup:** a table over all seven reply variants: `ScriptWrite`,
        `ScriptDelete`, `RunbookWrite`, `RunbookDelete`,
        `ScheduleWrite`, `ScheduleRemove`, `GhostSpawn`.
      - **Asserts:**
        - `approved: true` with a **mismatched** id is a denial;
        - with the **matching** id it is an approval. This is the
          positive control.
      - It tests the shared code path, if there is one. Otherwise it
        tests each handler.
- [ ] **A real handler:**
      `write_script_refuses_a_reply_for_another_prompt` passes.
      - **Setup:** `write_script` under `crate::test_home()`, fed a
        mismatched-id `ScriptWriteResponse { approved: true }`.
      - **Asserts:** no script file is written.
- [ ] **Credential:**
      `a_credential_for_another_prompt_is_not_sent` passes.
      - **Setup:** `credential_round_trip` fed a `CredentialResponse`
        whose id differs.
      - **Asserts:** the reply is `GiveUp`, not `Send`.
      - The existing
        `credential_round_trip_returns_a_zeroizing_answer` still passes.
- [ ] **No handler ignores the id:** this prints nothing. Today it
      prints 9 lines:
      `grep -rnE 'Response \{ approved: true, \.\. \}' src/daemon`
- [ ] **The credential match checks the id:** this prints nothing.
      Today it prints line 81:
      `grep -n 'CredentialResponse { credential, \.\. }' src/daemon/executor/shell_exec_auth.rs`
- [ ] **Mutation run, pasted as output:**
      1. Drop the id comparison from the approve/deny path.
      2. Run
         `cargo test --lib delete_schedule_refuses_a_reply_for_another_prompt`
         once. It must fail.
      3. Restore by undoing that one edit, never by copying a backup
         file.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff a5e158b3 -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, each run once. `tests/hygiene.rs` passes
      with the allowlist untouched.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement:** follow the edit order, writing each red test before
   its fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run and the mutation run, **as output, not summarized**;
   - the greps.

   Then commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may fix the id of any existing test fixture that answers a prompt
  with an id other than the prompt's.

## Out of scope

- `PaneSelectResponse`, which has no daemon reader.
- The CLI side, which already echoes ids.
- Reading on past a mismatched reply to wait for the matching one. A
  mismatch is a denial, as the two checked handlers already treat it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-27 01:49 (started)

Work has begun on the prompt-id check across the nine approve/deny handlers and
the credential round trip. The first red test,
`delete_schedule_refuses_a_reply_for_another_prompt`, is in place and red.

**Task list (each line names the acceptance item it closes):**

1. Write `delete_schedule_refuses_a_reply_for_another_prompt` (first red test;
   acceptance: "First red test").
2. Add the shared reply-check helper that returns `Option<(&str, bool)>`-style
   (prompt id, approved) from a parsed `&Request`, and rewire the nine
   approve/deny handlers + the edit-file handler's variant check to deny a
   mismatched id with one `log::warn!` naming both ids (acceptance:
   "Every approve/deny variant", "No handler ignores the id" grep).
3. Write `an_approval_for_another_prompt_is_a_denial` — the table over all
   seven reply variants, mismatched id denied, matching id approved as the
   positive control (acceptance: "Every approve/deny variant").
4. Write `write_script_refuses_a_reply_for_another_prompt` under
   `crate::test_home()`, asserting no script file is written (acceptance:
   "A real handler").
5. Fix `credential_round_trip` to check the id: a `CredentialResponse` whose id
   differs gives `PromptReply::GiveUp` naming the mismatch, credential still
   zeroized (acceptance: "Credential" + "The credential match checks the id"
   grep). Add `a_credential_for_another_prompt_is_not_sent`; keep
   `credential_round_trip_returns_a_zeroizing_answer` green.
6. Mutation run: drop the id comparison from the approve/deny path, run
   `cargo test --lib delete_schedule_refuses_a_reply_for_another_prompt` once
   (must fail), restore by undoing the edit (acceptance: "Mutation run").
7. Run the no-test-removed grep (prints 0), the two no-match greps, the four
   gates each once, `tests/hygiene.rs` with the allowlist untouched, capture
   all evidence in an end-to-end Update Log entry, then commit as one `fix:`
   commit (acceptance: "No test removed", "Gates", "Capture the evidence").

### Update — 2026-11-26 (escalation)

**Chosen lever:** resume.

**Rationale:** the run stalled with `NoProgressStall` after 60 read-only
turns, but the production fix is already in and is correct:
- `reply_approval` and `approval_reply_decision` in `executor/mod.rs`;
- the nine handlers rewired to them;
- the credential id guard.

Both structural greps print nothing. The architect's gate run on the partial
tree:
- build and clippy are green;
- the lib suite fails 2 of 3203: `hosted::a_correct_password_is_sent_exactly_once`
  and `hosted::a_later_command_on_the_line_still_gets_the_prompt_sentinel`.

That is the spec's fixture-id gotcha. `run_with_transport`
(`src/daemon/executor/shell_exec_tests.rs`) prompts with id `""`, while
`answered_transport` answers with id `"p"`.

The remaining work is small and enumerated, so a resume keeps the finished
edits.

### Update — 2026-11-27 03:05 (end-to-end verification)

The prior run's production fix was kept; the five remaining edits landed:
the `run_with_transport` fixture id (`""` → `"p"`), the credential mismatch
arm now zeroizes the dropped credential, the three new tests
(`a_credential_for_another_prompt_is_not_sent`,
`an_approval_for_another_prompt_is_a_denial`,
`write_script_refuses_a_reply_for_another_prompt`), and the shared
approve/deny check moved verbatim into the new
`src/daemon/executor/prompt_reply.rs` — `executor/mod.rs` had drifted over
the 800-line hygiene cap with the helper inline, so the helper (and its test)
now live in their own module, re-exported as `executor::reply_approval`.

**End-to-end verification (all against the real lib suite and real files):**

First red test (recorded before the fix, in the prior run; re-confirmed via
the mutation run below — with the id comparison dropped the same test fails):

```
thread 'daemon::executor::schedule::tests::delete_schedule_refuses_a_reply_for_another_prompt' panicked at src/daemon/executor/schedule.rs:804:9:
denial text: Job 28c67c34 deleted permanently
```

Mutation run (id comparison in `reply_approval` replaced with `if false`,
then restored by undoing that one edit — no backup file was used):

```
failures:
    daemon::executor::schedule::tests::delete_schedule_refuses_a_reply_for_another_prompt

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3215 filtered out; finished in 0.03s
```

After restore, the same test:

```
test daemon::executor::schedule::tests::delete_schedule_refuses_a_reply_for_another_prompt ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3215 filtered out; finished in 0.03s
```

Gates, each run once (final sequence, after `cargo fmt --all`):

```
cargo build:  Finished `dev` profile [unoptimized + debuginfo] target(s)
cargo clippy --all-targets --all-features -- -D warnings:
              Finished `dev` profile [unoptimized + debuginfo] target(s)
cargo test:
test result: ok. 3206 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 150.74s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.34s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.23s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The lib suite reports 3206 passed — the pre-phase 3203 plus the three new
tests. `tests/hygiene.rs` passes (16/16) with `tests/hygiene_allowlist.txt`
untouched; `src/daemon/executor/mod.rs` is back under the cap at 1768 total
lines because the helper moved out of it.

Acceptance greps (all clean):

```
$ grep -rnE 'Response \{ approved: true, \.\. \}' src/daemon
(no output, exit 1)

$ grep -n 'CredentialResponse { credential, \.\. }' src/daemon/executor/shell_exec_auth.rs
(no output, exit 1)

$ git diff a5e158b3 -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'
0
```

The three new tests each call the production code paths: `reply_approval`
(production, via the seven-variant table), `delete_schedule`, `write_script`
and `credential_round_trip` exactly as production callers do.

### Update — 2026-11-27 02:58 (complete, server-authored)

**Summary:** Resumed phase 11 (prompt-ids) and completed the five remaining edits: fixed the `run_with_transport` fixture id (`""` → `"p"`, which un-broke the two failing hosted shell tests), made the credential mismatch arm zeroize the dropped credential, and added the three required tests — `a_credential_for_another_prompt_is_not_sent`, `an_approval_for_another_prompt_is_a_denial` (the seven-variant table with matching-id positive control), and `write_script_refuses_a_reply_for_another_prompt`. During verification the hygiene gate caught `executor/mod.rs` at 826 production lines (over the 800 cap), so the shared `reply_approval`/`approval_reply_decision` helper and its test moved verbatim into the new `src/daemon/executor/prompt_reply.rs` (on the spec's closed file list as "one new file under `src/daemon/executor/`"), re-exported from `executor/mod.rs`; `mod.rs` is back to 1768 total lines and the allowlist is untouched.

**Notes for review:** The mutation run dropped the id comparison in `reply_approval` and `delete_schedule_refuses_a_reply_for_another_prompt` failed as required (`Job ... deleted permanently`), then passed after restoring the single edit. Both structural greps print nothing, and the no-test-removed grep prints 0. All four gates pass — lib suite: 3206 passed (3203 pre-phase + 3 new), hygiene 16/16 including `production_files_fit_the_cap`. All evidence is pasted as output in the end-to-end verification Update Log entry, committed as `fix:` a91008ae with a clean working tree. Phase doc status left at `in-progress` for the server's completion tail.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
t ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test attach_sends_session_counters_after_ok ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test alert_list_request_returns_the_snapshot ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.26s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test an_empty_session_starts_no_tmux_server ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (863379): No such process
kill: (-864068): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-20f7a263eb3505d4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-76388b7c8bf90a79)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-4990f807dc1b822c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-2b9c25f5dc5fb2cd)
     Running tests/integration.rs (target/debug/deps/integration-5b69b39d41a6d634)
     Running tests/isolation.rs (target/debug/deps/isolation-1e4cc445234900f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-cbce62edf1c01d9f)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-6c3b6672991501e2)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M40-security-hardening/phase-11-prompt-ids.md` — +81 -0
- `src/daemon/executor/knowledge/artifacts.rs` — +37 -0
- `src/daemon/executor/mod.rs` — +4 -37
- `src/daemon/executor/prompt_reply.rs` — +126 -0
- `src/daemon/executor/shell_exec_auth.rs` — +5 -1
- `src/daemon/executor/shell_exec_auth_tests.rs` — +22 -0
- `src/daemon/executor/shell_exec_tests.rs` — +1 -1

**Commit:** a91008ae06651006216144955b689af8a722f4ad

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-26

- **Verdict:** approved_first_try. There was no review bounce. The
  `hard_fail` and the resume are recorded on their own runs.
- **Bounces:** none
- **Executor:** GLM-5.3-Flash
  - The first run (315 turns) ended in a `NoProgressStall` after 60
    turns in a row of only reading. It had the production fix written
    and was stuck on the fixture-id gotcha.
  - It was resumed with five edits enumerated and landed in a single
    run.
- **Scope deviations:** the helper lives in a new file,
  `executor/prompt_reply.rs`, which the closed list allowed. The
  executor also dropped one blank line between two tests in
  `executor/mod.rs`, which is harmless.
- **Reviewer checks:**
  - **Gates:** all four pass. The lib suite reports 3206, which is
    3203 + 3, as pinned. Every integration suite passes.
  - **Greps:**
    - both structural greps print nothing;
    - the no-test-removed grep prints 0.
  - **Mutation evidence:** the `reply_approval` id check replaced with
    `if false` fails the first red test, and the pasted output shows it
    (`Job … deleted permanently`).
  - **Code health:** there are no production `unwrap`/`expect`/`panic`
    calls. The wrong-id credential arm zeroizes the credential it
    drops.
- **Calibration:**
  - The spec's "fix the fixture's id" gotcha named the class of failure
    but not the fixture. GLM spent its stall hunting for
    `run_with_transport`'s `""` prompt id.
  - When a change will break existing fixtures, run the suite against a
    probe of the change at drafting, and name the failing fixtures. This
    is the fourth occurrence of the pending "probe current-behaviour
    claims at drafting" fold, after 08 and 09a×2.

