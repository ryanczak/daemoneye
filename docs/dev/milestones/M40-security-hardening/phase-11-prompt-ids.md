# Phase 11 — prompt-ids

**Milestone:** M40 — Security hardening
**Status:** todo
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
