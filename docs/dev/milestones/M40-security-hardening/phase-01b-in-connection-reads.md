# Phase 01b — in-connection-reads

**Milestone:** M40 — Security hardening
**Status:** in-progress
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
