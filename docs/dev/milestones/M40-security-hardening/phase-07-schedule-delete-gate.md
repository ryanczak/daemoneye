# Phase 07 — schedule-delete-gate

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-26
**Depends on:** 06b (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S12**. `cancel_schedule` and `delete_schedule` have no ghost
guard and no approval.

**Re-verified by the architect on 2026-11-26.** In
`src/daemon/executor/dispatch_schedule.rs`:
- **The arms:** the `CancelSchedule` and `DeleteSchedule` arms call
  `schedule::cancel_schedule` and `schedule::delete_schedule` directly.
- **No ghost check:** `is_ghost` is in scope but not consulted, so a ghost
  can cancel or delete any job, including the job that watches it.
- **No prompt:** chat never asks.

Its sibling `schedule_command` refuses ghosts and asks chat with
`ScheduleWritePrompt`. `assets/prompts/sre.toml` already tells the model
that `delete_schedule` needs a confirmation. That is false today.

**After this phase:**
- **Ghosts:** both tools are refused to a ghost, as `schedule_command` is.
- **Chat:** both tools ask first, with one new prompt that names the job
  and says whether it is a cancel or a delete. The prompt is never
  session-approved.

## Statement of work

1. **Wire protocol.** In `src/ipc.rs`, add one pair, used for both tools:
   - `Response::ScheduleRemovePrompt { id, job_id, name, kind, action,
     delete: bool }`, where `kind` and `action` are the job's `describe()`
     strings;
   - `Request::ScheduleRemoveResponse { id, approved }`.

   Place each beside `ScheduleWritePrompt` / `ScheduleWriteResponse`, with
   one-line doc comments. **`src/ipc.rs` is at about 780 production lines
   and the hygiene cap is 800.** If `cargo test --test hygiene` fails on
   `src/ipc.rs`, add its line to `tests/hygiene_allowlist.txt` with a
   one-line reason, and say so in the Update Log.
2. **Daemon gate,** in `src/daemon/executor/schedule.rs`.
   `cancel_schedule` and `delete_schedule` become async and take `call_id`,
   `is_ghost`, `tx` and `rx`. `dispatch_schedule.rs` passes them; it
   already holds all four. In order:
   - **Ghost:** refuse with
     `Error: cannot cancel or delete scheduled jobs in a Ghost Shell
     (requires user approval).` Send no frame, and leave the store
     untouched.
   - **Unknown id:** if no job in `schedule_store.list()` has this exact
     `id`, return today's `Job <id> not found` text, with no prompt.
   - **Chat:** send `ScheduleRemovePrompt`, then read the reply exactly as
     `run_schedule_command` does: `USER_PROMPT_TIMEOUT`,
     `read_bounded_line` with `MAX_IPC_MESSAGE_BYTES`, `Err` on EOF, `Err`
     on an over-long line. Only `ScheduleRemoveResponse { approved: true }`
     approves; anything else is a denial.
   - **Approved:** do today's cancel or delete, with today's result text
     and today's `job_canceled` / `job_deleted` events.
   - **Denied:** return `Job removal denied by user` and log
     `command_approval` with `mode: "schedule_cancel"` or
     `"schedule_delete"`, `decision: "denied"`. The job is unchanged.
3. **Clients.** Handle `ScheduleRemovePrompt` everywhere
   `ScheduleWritePrompt` is handled. The output of
   `grep -rn "ScheduleWritePrompt" src` lists the sites:
   - **`stream_ask.rs`:** add it to the `handle_script_or_schedule_prompt`
     dispatch, plus an arm there that calls a new
     `prompt_schedule_remove_ratatui` in `stream_prompts.rs`.
     - The new function shows the name, the short id, the kind and the
       action, and says "cancel (kept in the list)" or "delete
       permanently".
     - It asks with `prompt_yes_no_ratatui`, with no session-approval
       path. `prompt_ghost_spawn_ratatui` is the model.
   - **`ask.rs`:** the one-shot client denies, as it does for
     `GhostSpawnPrompt`.
   - **`program_status_emit.rs`:** both lists, plus a row in
     `program_status_emit_tests.rs`'s frame-state table (`Kind::Permission`).
   - **`ipc_types.rs`:** the variant name.
4. **Gated list and its consequence.** Add `cancel_schedule` and
   `delete_schedule` to `APPROVAL_GATED_TOOLS` (`src/ai/tools/defs.rs`),
   and mark both rows `**⚠**` in README § AI tools.
   - **Consequence:** this also exempts both from the per-turn,
     per-session and per-tool budget caps (milestone decision 2). Say so
     in `docs/security.md`, next to the 06b sentence about
     `spawn_ghost_shell`.
   - **Test to flip:** `daemon::stream::tests::approval_gated_contains_expected_tools`
     asserts `delete_schedule` is **not** gated. Flip that assertion and
     its comment, and add the same for `cancel_schedule`.
5. **Docs.**
   - `CLAUDE.md`: add "approval-gated from chat; refused to ghosts" to the
     two tool rows.
   - `assets/prompts/sre.toml`: add `cancel_schedule` to the
     confirmation list near line 62. `delete_schedule` is already there.
6. **Constraints.**
   - `ScheduleStore` is **unchanged**.
   - The `daemoneye schedule cancel/delete` CLI subcommands are unchanged.
     They are the user acting, not the AI.
   - **No existing test is removed.**

**Closed file list:**
- `src/ipc.rs`, `src/ipc_types.rs`, `src/ipc_tests.rs`;
- `src/daemon/executor/schedule.rs`,
  `src/daemon/executor/dispatch_schedule.rs`;
- `src/cli/commands/stream_ask.rs`, `src/cli/commands/stream_prompts.rs`,
  `src/cli/commands/ask.rs`;
- `src/cli/program_status_emit.rs`, `src/cli/program_status_emit_tests.rs`;
- `src/ai/tools/defs.rs`, `src/daemon/stream.rs`;
- `README.md`, `CLAUDE.md`, `docs/security.md`, `assets/prompts/sre.toml`;
- `tests/hygiene_allowlist.txt`, only if item 1 needs it.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is an edit to `src/ipc.rs`:** the two variants.
2. The `ipc_types.rs` arm, then the wire test.
3. The daemon gate in `schedule.rs` and `dispatch_schedule.rs`, with its
   tests.
4. The clients.
5. The list, `stream.rs`, and the docs.

**Read at most 3 files before your first edit.** After **5 reads in a
row**, your next call must be an edit or a `cargo` command.

**Budget:** **1200 turns**; use them efficiently.

### Pre-verified facts (do not re-read to confirm)

- `schedule.rs` imports explicitly, not with a glob: `ToolCallOutcome`,
  `USER_PROMPT_TIMEOUT`, `log_event`, `send_response_split`, `Request`,
  `Response`, `ScheduleListItem`, `ActionOn`, `JobStatus`, `ScheduleKind`,
  `ScheduleStore`, `ScheduledJob` and `Arc` are already imported. Its test
  module has `use super::*;`.
- `run_schedule_command` in the same file is the round trip to copy.
- `ScheduledJob` has `id`, `name`, `kind` and `action`. `kind.describe()`
  and `action.describe()` return `String`. `schedule_store.list()` returns
  a `Vec<ScheduledJob>` snapshot.
- **The test harness needs no duplex pair and cannot hang:**
  - **writer:** `let mut out: Vec<u8> = Vec::new();`
  - **reader with a canned reply:**
    `tokio::io::BufReader::new(std::io::Cursor::new(format!("{}\n",
    serde_json::to_string(&Request::ScheduleRemoveResponse { … }).unwrap())
    .into_bytes()))`
  - **reader for the no-prompt legs:**
    `tokio::io::BufReader::new(tokio::io::empty())`, as
    `run_schedule_command_refuses_invalid_calls_before_prompting` does.
  - **store:** `Arc::new(ScheduleStore::new_empty())`, with a job added
    via `store.add(ScheduledJob::new("nightly".to_string(),
    ScheduleKind::Once { at: chrono::Utc::now() }, ActionOn::Alert,
    None))`. `add` returns the job id as `Result<String>`, and a new job's
    status is `JobStatus::Pending`.
  - **asserting a prompt was sent:** parse `out` up to its first `\n` as a
    `Response`, and match `ScheduleRemovePrompt`.
- `log_event` panics in a test that does not hold the HOME lock. A test
  that reaches the denied or approved leg takes `crate::test_home()` once
  and holds it.

## First red test

`delete_schedule_from_chat_asks_first`, in `schedule.rs`'s tests.
- **Denied leg:** a store holding one job. The reader holds
  `ScheduleRemoveResponse { approved: false }`.
  - **Asserts:** the outcome contains `denied`; `out` holds a
    `ScheduleRemovePrompt` with `delete: true` and the job's name; the job
    is still in `store.list()`.
- **Approved leg:** the same, with `approved: true`.
  - **Asserts:** the outcome contains `deleted permanently`, and the store
    is empty.

It is red today: no prompt is sent, and the denied leg deletes the job.
Paste the red run.

## Acceptance criteria

- [ ] **Chat asks before deleting:** `delete_schedule_from_chat_asks_first`
      passes. Paste the red run.
- [ ] **Chat asks before cancelling:** `cancel_schedule_from_chat_asks_first`
      passes.
      - **Same shape:** `delete: false`.
      - **Denied leg:** the job's status is still `Pending`.
      - **Approved leg:** its status is `Cancelled`, and it stays listed.
- [ ] **Ghost refused:** `schedule_removal_from_a_ghost_is_refused` passes.
      - **Calls:** both tools, with `is_ghost: true`.
      - **Asserts:** the outcome contains `Ghost Shell`, `out` is empty,
        and the job is unchanged.
- [ ] **Unknown id, no prompt:** `schedule_removal_of_an_unknown_id_sends_no_prompt`
      passes. The outcome contains `not found`, and `out` is empty.
- [ ] **Wire:** `schedule_remove_prompt_round_trips`, in `src/ipc_tests.rs`,
      passes. It covers both variants, with `delete` true and false.
- [ ] **Status frames:** the frame-state table in
      `program_status_emit_tests.rs` covers `ScheduleRemovePrompt` as
      `Kind::Permission`.
- [ ] **Gated list:** run this once. It prints **2** (0 today):
      `awk '/APPROVAL_GATED_TOOLS/,/\];/' src/ai/tools/defs.rs | grep -cE '"(cancel|delete)_schedule"'`
- [ ] **Docs in sync:** `cargo test --test doc_truth` passes.
- [ ] **Mutation run, pasted as output:**
      1. Make the chat path skip the prompt and treat the call as
         approved.
      2. Run `cargo test --lib delete_schedule_from_chat_asks_first` once.
         It must **fail**, not hang.
      3. Restore by **undoing that one edit**. Never copy a backup file
         over the source: that silently reverts your later edits.
      4. Run `git diff --stat` before and after the mutation. Paste both.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 640d4fea -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, each run once.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run and the mutation run, **as output, not summarized**;
   - the two greps.

   Then commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- `cancel_schedule` and `delete_schedule` in `schedule.rs` may change
  signature and become `async`.

## Out of scope

- **Session approval for removals.** Each removal asks.
- **Letting a ghost remove its own job via policy.** Ghosts are refused,
  as for `schedule_command`.
- **The `&id[..8]` slices in `ScheduleStore::cancel` / `delete`.** Real
  ids are UUIDs; not this phase.
- **`spawn_ghost_shell`.** Done in phase 06b.

## Update Log

<!-- entries appended below this line -->
