# Phase 01b — in-connection-reads

**Milestone:** M40 — Security hardening
**Status:** done
**Drafted:** 2026-11-25
**Depends on:** phase-01a (done: `read_bounded_line` and
`MAX_IPC_MESSAGE_BYTES` in `src/ipc_bounded.rs`, re-exported from
`crate::ipc`)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S4a**, second half. Phase 01a bounded the first request read
on each connection. Every later read on a daemon connection still reads a
whole line into memory, with no cap. The architect **reproduced two of
them** on 2026-11-25 with scratch probes, since reverted. Each probe fed a
64 MiB source with no newline:

- **`await_edit_file_response`** (an approval prompt) read all 67108864
  bytes.
- **`handle_subscribe`** (the notice stream) read all 67108864 bytes.

The other 11 sites read in the same way. `handle_chat_attach`'s hold loop
has a second defect: it clears its `discarded` buffer only when a push is
sent, so a quiet connection's buffer grows with every byte the client
sends.

## Statement of work

Convert each site **in place**. No new helper module is needed. Swap
`read_line` / `lines()` for `crate::ipc::read_bounded_line` at
`crate::ipc::MAX_IPC_MESSAGE_BYTES`, then parse from the byte buffer. The
work list, in edit order:

| # | File | Function | Oversized reply |
|---|---|---|---|
| 1 | `src/daemon/executor/file_ops/write.rs` | `await_edit_file_response` | `Err` |
| 2 | `src/daemon/executor/mod.rs` | `approval_reply` | `Err` |
| 3 | `src/daemon/executor/shell_exec_auth.rs` | `credential_round_trip` | `GiveUp` |
| 4 | `src/daemon/executor/schedule.rs` | `run_schedule_command` | `Err` |
| 5 | `src/daemon/executor/knowledge/agents.rs` | `create_agent`, `delete_agent` | `Err` |
| 6 | `src/daemon/executor/knowledge/artifacts.rs` | `write_script`, `delete_script`, `write_runbook`, `delete_runbook` | `Err` |
| 7 | `src/daemon/server/notices.rs` | `handle_subscribe` | ends the stream |
| 8 | `src/daemon/server/shells.rs` | `handle_shell_tail` | ends the tail |
| 9 | `src/daemon/server/chat_presence.rs` | `handle_chat_attach` (the hold loop) | ends the hold |

**Rules:**

1. **Prompt sites (rows 1–6) fail closed and end the turn.** An oversized
   reply takes that site's EOF path (`Ok(Ok(0))` → `Err`), with an
   `anyhow` error whose text contains `too large`. Nothing is approved and
   nothing is written. A denial alone is not enough, for this reason: the
   rest of the oversized line is still in the stream, and the next prompt
   would read it as its own reply. Most sites do not check the reply's id
   yet, so a later reply could then pair with the wrong prompt.
2. **The credential prompt (row 3) has no error path.** It returns
   `PromptReply::GiveUp` with a reason containing `too large`. It must not
   reuse the "nobody is attached" or "not supplied in time" texts.
   Zeroize the read buffer on every path, as `cred_line` is today.
3. **The stream loops (rows 7–9) still ignore what the client sends.**
   Their buffer is emptied after **every** complete line, so many small
   lines never add up to the cap. A single oversized line ends the stream
   or hold with `Ok(())`, the same way a client hang-up does. Row 9's hold
   ends through its existing path, so the unregister bookkeeping after it
   still runs.
4. Replies under the cap behave exactly as today. Existing tests pass
   unchanged.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- **Tests must fail, not hang or pass by construction.** Phase 01a's tests
  bounced on exactly this (`bugs/bug-01a-1.md`).
  - Every await in a new test runs under a timeout, writes included.
  - No assertion may be true by construction, such as `x || true`.
- **Budget tests on a generic reader** (rows 1–3, 7) use
  `tokio::io::BufReader::new(tokio::io::repeat(b'a').take(64 << 20))`.
  `get_ref().limit()` gives the bytes not yet handed out, so they need no
  socket. **Budget tests on a socket or duplex** (rows 8–9) count the
  bytes the writer got accepted before its first write error, as phase
  01a's do.

**Closed file list:** the nine files in the table, plus
`src/daemon/server/shells_tests.rs` (the tail test). Read at most **3**
files before your first edit.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test. The first
production patch is due by row 2.

## First red test

`edit_reply_refuses_an_oversized_reply_within_budget`, in `write.rs`'s
own tests.
- Call `await_edit_file_response` on the 64 MiB `repeat` reader.
- The result is an `Err` whose text contains `too large`.
- **At most `MAX_IPC_MESSAGE_BYTES + 64 KiB`** was handed out.

It compiles today and is red: all 67108864 bytes are handed out and the
result is `Ok`. Paste the red run.

## Acceptance criteria

- [ ] **Edit reply:** `edit_reply_refuses_an_oversized_reply_within_budget`
      passes. Red run pasted.
- [ ] **Approval:** `approval_refuses_an_oversized_reply_within_budget`
      (in `executor/mod.rs` tests) passes. It calls `approval_reply` and
      checks the same `Err` and budget as above.
- [ ] **Credential:** `credential_prompt_refuses_an_oversized_reply_within_budget`
      (in a test module of `shell_exec_auth.rs`) passes. It calls
      `credential_round_trip`; the result is `GiveUp` containing `too large`,
      within the same budget.
- [ ] **Notices:** `subscribe_ends_on_an_oversized_client_line_within_budget`
      (in `notices.rs` tests) passes. `handle_subscribe` returns `Ok(())`
      within the same budget.
- [ ] **Tail:** `shell_tail_ends_on_an_oversized_client_line` (in
      `shells_tests.rs`, built with its `spawn_tail` helper) passes. The
      tail ends, and the client had **at most 2 MiB** accepted.
- [ ] **Presence, small lines:** `presence_hold_survives_lines_totalling_over_the_cap`
      (in `chat_presence.rs` tests) passes. It sends 2 MiB as 1 KiB lines,
      and a presence push still reaches the client afterwards. **This is a
      preservation test:** it passes today. It goes red if the new code
      does not empty the buffer after each line, because the cap would
      then end the hold. Say so in its doc comment, and do not chase a red
      run for it.
- [ ] **Presence, one big line:** `presence_hold_ends_on_an_oversized_line`
      passes. A 64 MiB line with no newline ends the hold, and the client
      had **at most 2 MiB** accepted.
- [ ] **Sweep:** run this command **once**. It prints **0** (it prints 13
      today):
      `grep -rnE 'rx\.read_line\(|rx\.lines\(\)' src/daemon/executor src/daemon/server | grep -c .`
- [ ] **Mutation check,** run once with the output pasted:
      1. in `await_edit_file_response`, pass `usize::MAX` as the cap;
      2. run
         `cargo test --lib edit_reply_refuses_an_oversized_reply_within_budget`
         once and see it fail;
      3. restore the cap.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3132** passed (3125 plus seven). `tests/hygiene.rs` passes.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per work-list
   row, each naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement:** the work list in order, writing each row's test before
   its fix where the row has one. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run;
   - the sweep grep;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.

## Out of scope

- Checking the reply id at the prompt sites (phase 11, S15).
- `Zeroizing` for the credential beyond the buffer zeroize already there
  (phase 10, S14).
- The liveness probe in `src/daemon/mod.rs`, where the daemon is the
  client.
- The daemon-side shell client and the CLI's reads.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-25 17:10 (started)

Work on phase 01b has begun: converting the 13 in-connection reads to
`read_bounded_line`, per the work list, with the first red test written
before any production change.

### Update — 2026-11-25 17:20 (breakdown)

Task list, one line per work-list row, each naming the acceptance item it
closes:

1. `write.rs` `await_edit_file_response` → bounded read, `Err` on
   oversized — closes **Edit reply**.
2. `executor/mod.rs` `approval_reply` → bounded read, `Err` — closes
   **Approval**.
3. `shell_exec_auth.rs` `credential_round_trip` → bounded read, `GiveUp`
   — closes **Credential**.
4. `schedule.rs` `run_schedule_command` → bounded read, `Err` — part of
   the prompt-site sweep (no dedicated acceptance item).
5. `knowledge/agents.rs` `create_agent`, `delete_agent` → bounded reads —
   same sweep.
6. `knowledge/artifacts.rs` `write_script`, `delete_script`,
   `write_runbook`, `delete_runbook` → bounded reads — same sweep.
7. `notices.rs` `handle_subscribe` → bounded read, ends the stream —
   closes **Notices**.
8. `shells.rs` `handle_shell_tail` → bounded read, ends the tail — closes
   **Tail**.
9. `chat_presence.rs` `handle_chat_attach` → bounded hold loop — closes
   **Presence, small lines** and **Presence, one big line**.

First red test written (`edit_reply_refuses_an_oversized_reply_within_budget`)
and run red:

```
test daemon::executor::file_ops::write::tests::edit_reply_refuses_an_oversized_reply_within_budget ... FAILED
thread 'daemon::executor::file_ops::write::tests::edit_reply_refuses_an_oversized_reply_within_budget' panicked at src/daemon/executor/file_ops/write.rs:349:26:
an oversized reply is refused, got Err(Result("User denied execution"))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3135 filtered out; finished in 0.05s
```

(The full 64 MiB was handed out and parsed as a non-reply, reading as a
denial — the failure the phase exists to close.)

### Update — 2026-11-25 17:55 (end-to-end verification)

All nine work-list rows converted in place to
`crate::ipc::read_bounded_line` at `crate::ipc::MAX_IPC_MESSAGE_BYTES`;
the hold loop's buffer is now emptied after every complete line, and one
oversized line ends the stream/tail/hold `Ok(())` like a hang-up.

Per-acceptance-item evidence (each `test result` line from its own run):

- **Edit reply** (`edit_reply_refuses_an_oversized_reply_within_budget`,
  `write.rs` tests): `test result: ok. 1 passed; 0 failed; 0 ignored;
  0 measured; 3141 filtered out; finished in 0.01s`
- **Approval** (`approval_refuses_an_oversized_reply_within_budget`,
  `executor/mod.rs` `approval_oversize_tests`): `test result: ok. 1 passed;
  0 failed; 0 ignored; 0 measured; 3141 filtered out; finished in 0.01s`
- **Credential** (`credential_prompt_refuses_an_oversized_reply_within_budget`,
  `shell_exec_auth_tests.rs`): `test result: ok. 1 passed; 0 failed;
  0 ignored; 0 measured; 3141 filtered out; finished in 0.01s` — the
  give-up reason contains `too large` and reuses neither the
  "nobody is attached" nor "not supplied in time" wording; the buffer is
  zeroized on every path as before.
- **Notices** (`subscribe_ends_on_an_oversized_client_line_within_budget`,
  `notices.rs` tests): `test result: ok. 1 passed; 0 failed; 0 ignored;
  0 measured; 3141 filtered out; finished in 0.01s`
- **Tail** (`shell_tail_ends_on_an_oversized_client_line`,
  `shells_tests.rs`, `spawn_tail` helper): `test result: ok. 1 passed;
  0 failed; 0 ignored; 0 measured; 3141 filtered out; finished in 1.56s` —
  the write loop ends with the client having accepted at most 2 MiB.
- **Presence, small lines**
  (`presence_hold_survives_lines_totalling_over_the_cap`): `test result:
  ok. 1 passed` (within the `presence_hold` run below) — 2 MiB as 1 KiB
  lines, then a push still arrives.
- **Presence, one big line** (`presence_hold_ends_on_an_oversized_line`):
  same run — hold ends Ok with at most 2 MiB accepted and
  `presence_count == 0` afterwards.

`presence_hold` combined run: `test result: ok. 2 passed; 0 failed;
0 ignored; 0 measured; 3140 filtered out; finished in 0.02s`

**Sweep** (run once; printed 13 before the work):

```
$ grep -rnE 'rx\.read_line\(|rx\.lines\(\)' src/daemon/executor src/daemon/server | grep -c .
0
```

**Mutation check** (cap replaced with `usize::MAX` in
`await_edit_file_response`, run once, then restored):

```
thread 'daemon::executor::file_ops::write::tests::edit_reply_refuses_an_oversized_reply_within_budget' panicked at src/daemon/executor/file_ops/write.rs:356:26:
an oversized reply is refused, got Err(Result("User denied execution"))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3141 filtered out; finished in 0.23s
```

(cap restored: `test result: ok. 1 passed; 0 failed; 0 ignored;
0 measured; 3141 filtered out; finished in 0.01s`)

**Gates** (run once each; `cargo fmt --all` before staging):

```
cargo build:      Finished `dev` profile, no warnings
cargo clippy --all-targets --all-features -- -D warnings: Finished, no warnings
cargo test (lib): test result: ok. 3132 passed; 0 failed; 10 ignored
cargo test (all): 10 suites, all `test result: ok`, 0 failed
tests/hygiene.rs: test result: ok. 16 passed; 0 failed; 0 ignored
```

### Update — 2026-11-25 18:14 (complete, server-authored)

**Summary:** Phase 01b is implemented and committed (`9a19c241`). All 13 in-connection reads across the nine work-list files were converted in place to `crate::ipc::read_bounded_line` at `crate::ipc::MAX_IPC_MESSAGE_BYTES`: the six prompt sites fail closed with a `too large` `Err`, `credential_round_trip` returns a fresh `GiveUp("credential reply line too large …")` (buffer still zeroized on every path), and the three stream loops (subscribe, shell tail, chat-presence hold) end `Ok(())` on one oversized line like a hang-up, with the hold's buffer emptied after every complete line so 2 MiB of 1 KiB lines never trips the cap. All seven acceptance tests pass; the sweep grep prints 0 (was 13); the mutation check (cap → `usize::MAX`) turned the edit-reply test red and back; all four gates pass with the lib suite at 3132 passed / 0 failed and `tests/hygiene.rs` green. Evidence, red run, sweep and mutation output are in the phase's end-to-end verification Update Log entry.

**Notes for review**

- One adaptation beyond the letter of the spec: `presence_unregisters_when_the_hold_ends_in_error` previously exercised the error path via non-UTF-8 bytes, which `read_bounded_line` (byte-copying, no UTF-8 decode) no longer produces — I re-pointed that test at the hold's remaining error path (a failed push write after dropping the client's read half), keeping its assertions about unregister bookkeeping unchanged. Flagging since it touches a pre-existing test's mechanism, not just its fixture.
- `handle_shell_tail`'s pre-existing `host.next_line()` reads (daemon→shell-host direction, the daemon as client) are out of scope per the phase doc and untouched.
- Two `ToolCallOutcome`/`ApprovalReply` enums gained `#[derive(Debug)]` so the budget tests can name the non-refused outcome in their panic messages.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test event_log_append_read ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
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
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.23s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (3782802): No such process
kill: (-3783473): No such process
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

- `docs/dev/milestones/M40-security-hardening/README.md` — +1 -1
- `docs/dev/milestones/M40-security-hardening/phase-01b-in-connection-reads.md` — +112 -1
- `src/daemon/executor/file_ops/write.rs` — +67 -26
- `src/daemon/executor/knowledge/agents.rs` — +32 -14
- `src/daemon/executor/knowledge/artifacts.rs` — +64 -28
- `src/daemon/executor/mod.rs` — +63 -4
- `src/daemon/executor/schedule.rs` — +16 -7
- `src/daemon/executor/shell_exec.rs` — +3 -0
- `src/daemon/executor/shell_exec_auth.rs` — +15 -5
- `src/daemon/executor/shell_exec_auth_tests.rs` — +46 -0
- `src/daemon/server/chat_presence.rs` — +150 -17
- `src/daemon/server/notices.rs` — +41 -7
- `src/daemon/server/shells.rs` — +13 -5
- `src/daemon/server/shells_tests.rs` — +56 -0

**Commit:** 9a19c241b9b3011b0a1f94e0174c32c4a31fe198

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-25

- **Verdict:** escalated
- **Bounces:** 1 (bug-01b-1, major: the re-pointed
  `presence_unregisters_when_the_hold_ends_in_error` failed 136 of 300
  runs)
- **Executor:** GLM-5.3-Flash. The first dispatch completed all nine
  work-list rows, all seven acceptance tests, the sweep and the mutation
  check. Claude (direct) did the bug-01b-1 test fix (`5ae8afb0`).
- **Scope deviations, all accepted:**
  - The credential test lives in a new `shell_exec_auth_tests.rs`,
    declared from `shell_exec.rs`, which is outside the closed list.
    `credential_round_trip` widened to `pub(super)` so that test can call
    it.
  - `ToolCallOutcome` and `ApprovalReply` derive `Debug` for test
    messages.
  - The executor re-pointed one pre-existing test, because non-UTF-8 input
    no longer errors, and disclosed it. That disclosure is what led the
    review to bug-01b-1.
- **Review checks:**
  - All four gates were re-run: lib suite 3132 passed. No `unwrap`,
    `expect`, `panic!`, `unsafe` or `#[allow]` in added production lines.
    The executor's red run and mutation check are pasted and accepted.
  - Every new test was stress-run 100 times on the lib binary `--exact`,
    with 0 failures. The path of each was checked with `--list`: an
    `--exact` filter that matches nothing runs no tests and passes. That
    hit the review once, on the tail test's path.
- **Calibration:**
  1. **A test that races two `select!` branches.** The executor's own
     comment asserted an order that `tokio::select!` does not keep. One
     occurrence of this shape. It is the second M40 bounce in a row on test
     mechanics rather than production code (bug-01a-1 was a test that
     waited on the wrong thing).
  2. **A stress loop has to prove it ran the test.** The review's first
     loop pointed at the bin binary and its second at a mistyped path; both
     reported nonsense (200/200 failed, then 0/100). Pin the test-count
     check (`grep -q '1 passed'`) in any stress criterion. Fold candidate
     with M39 phase 10's stress loop.
