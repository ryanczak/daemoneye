# Phase 06b — ghost-spawn-approval

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-26
**Depends on:** 06a (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S9**, second half. From interactive chat, `spawn_ghost_shell`
starts an autonomous ghost **without asking the user**.

The architect re-verified this on 2026-11-26: the `PendingCall::SpawnGhost`
arm of `dispatch_meta::dispatch` (`src/daemon/executor/dispatch_meta.rs`)
checks the depth and calls `knowledge::spawn_ghost` directly. No prompt
frame is sent. A ghost then runs commands on its own under its runbook's
policy. Every other chat tool with that much reach is approval-gated:
`run_terminal_command`, `schedule_command`, `write_script` and the rest.

After this phase, a spawn from chat asks first, with its own prompt, and
that prompt is never session-approved. A spawn from a ghost stays headless.
Phase 06a already narrows a ghost caller's child to the caller's grants.

## Statement of work

1. **Wire protocol.** In `src/ipc.rs`, add:
   - `Response::GhostSpawnPrompt { id, runbook, agent: Option<String>,
     host: Option<String>, message }`;
   - `Request::GhostSpawnResponse { id, approved: bool }`.

   Model both on `ScheduleWritePrompt` and `ScheduleWriteResponse`.
   **Gotcha:** `src/ipc.rs` is near the 800-line production cap
   (`tests/hygiene.rs`), so keep the doc comments to two lines.
2. **Daemon gate.** In the `SpawnGhost` arm of `dispatch_meta::dispatch`,
   when the caller is chat (`ghost_policy` is `None`), send the prompt and
   await the reply **before** calling `spawn_ghost`. Do it after the depth
   check.
   - Follow the round trip in `schedule_command`
     (`src/daemon/executor/schedule.rs`): `send_response_split`,
     `USER_PROMPT_TIMEOUT`, `read_bounded_line` with
     `MAX_IPC_MESSAGE_BYTES`, `Err` on EOF, and `Err` on an over-long
     line.
   - Only `GhostSpawnResponse { approved: true }` approves. Anything else,
     including a timeout, is a denial.
   - A denial returns
     `ToolCallOutcome::Result("Ghost shell spawn denied by user")`, and
     logs a `ghost_spawn_approval` event with `decision: "denied"`. An
     approval logs `decision: "approved"`.
   - `dispatch_meta::dispatch` gains the `rx` reader. `route.rs` already
     holds it.
   - A ghost caller (`ghost_policy` is `Some`) sends **no** prompt.
3. **Clients.** Every place that matches `ScheduleWritePrompt` or
   `ScheduleWriteResponse` now matches the new pair too.
   `grep -rn "ScheduleWritePrompt\|ScheduleWriteResponse" src` lists them.
   - **The chat client** (`stream_ask.rs`, and a new
     `prompt_ghost_spawn_ratatui` in `stream_prompts.rs`) shows the
     runbook, the agent and the host if given, and the message. It asks
     with `prompt_yes_no_ratatui`. It is **never session-approved**: no
     `PromptKind` variant and no auto-approve check.
   - **`daemoneye ask`** (`ask.rs`) denies it, as it does the others.
   - **`program_status_emit.rs`** maps it to `Blocked` / `Permission`.
   - **`ipc_types.rs`** gives it its variant name.
4. **Approval-gated list.** Add `spawn_ghost_shell` to
   `APPROVAL_GATED_TOOLS` in `src/ai/tools/defs.rs`, and mark it `**⚠**`
   in README.md's tool table. `tests/doc_truth.rs` holds the two in sync.
   - **Consequence (milestone decision 2), stated in the docs:** that list
     also exempts `spawn_ghost_shell` from the per-turn, per-session and
     per-tool budget caps in `daemon::stream`, for chat and ghost callers
     alike. Spawns stay bounded by `max_concurrent_ghosts` and the depth
     cap. `LimitsConfig::validate` will now warn on a
     `[limits.per_tool] spawn_ghost_shell` entry.
5. **Docs.**
   - In `docs/security.md` § 10 (the delegated-ghost guard), add one
     sentence: a chat spawn is approval-gated and never session-approved,
     and the tool is exempt from the budget caps.
   - In CLAUDE.md, add "approval-gated from chat" to the
     `spawn_ghost_shell` row.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- `knowledge::spawn_ghost` and phase 06a's narrowing are **unchanged**.
- An older client that does not know the new frame is not supported.
  Daemon and CLI ship together.
- **No existing test is removed.**

**Closed file list:**
- `src/ipc.rs`, `src/ipc_types.rs`, `src/ipc_tests.rs`;
- `src/daemon/executor/dispatch_meta.rs`, `src/daemon/executor/route.rs`;
- `src/cli/commands/stream_ask.rs`, `src/cli/commands/stream_prompts.rs`,
  `src/cli/commands/ask.rs`;
- `src/cli/program_status_emit.rs`, `src/cli/program_status_emit_tests.rs`;
- `src/ai/tools/defs.rs`;
- `README.md`, `CLAUDE.md`, `docs/security.md`.

Read at most **3** files before your first edit.

**Edit order:** the first red test, then the wire variants, then the
daemon gate, then the clients, then the list and docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`spawn_ghost_from_chat_asks_first`, in `dispatch_meta.rs`'s tests.
- **Setup:** two `tokio::io::duplex` pairs, as `dispatch_open_shell.rs`'s
  tests use, and a temp `HOME` with no runbooks.
- **Call:** `dispatch` with a `PendingCall::SpawnGhost` for runbook
  `"rb06b"`, and `ghost_policy: None`.
- **Denied leg:** the client side reads a `GhostSpawnPrompt` naming
  `rb06b` and replies `approved: false`. The outcome text contains
  `denied`, and the session store is empty.
- **Approved leg:** the same call, answered `approved: true`. The outcome
  text contains `Failed to load runbook`, which proves the spawn went
  ahead once approved.

It is red today: no prompt frame is sent. Paste the red run.

## Acceptance criteria

- [ ] **Chat asks:** `spawn_ghost_from_chat_asks_first` passes. Red run
      pasted.
- [ ] **Ghost does not ask:** `spawn_ghost_from_a_ghost_sends_no_prompt`
      passes.
      - **Call:** the same as above, with `ghost_policy: Some(…)`.
      - **Asserts:** the outcome contains `Failed to load runbook`, and
        the client side reads **no** bytes once the daemon's writer is
        dropped.
- [ ] **Wire:** `ghost_spawn_prompt_round_trips` passes, in
      `src/ipc_tests.rs`. It round-trips `GhostSpawnPrompt`, with and
      without `agent` and `host`, and `GhostSpawnResponse`.
- [ ] **Status frames:** the existing frame-state tests in
      `program_status_emit_tests.rs` cover `GhostSpawnPrompt` as
      `Blocked` / `Permission`.
- [ ] **Gated list:** run this once. It prints **1** (0 today):
      `awk '/APPROVAL_GATED_TOOLS/,/\];/' src/ai/tools/defs.rs | grep -c spawn_ghost_shell`
- [ ] **Docs in sync:** `cargo test --test doc_truth` passes.
- [ ] **Mutation run, pasted as output:**
      1. Skip the prompt: make the chat branch call `spawn_ghost`
         directly.
      2. Run `cargo test --lib spawn_ghost_from_chat_asks_first` once. It
         must **fail**.
      3. Restore. Then `git diff --stat` shows no change from the
         mutation.
- [ ] **Preserved:** phase 06a's `ghost_narrow_tests` and
      `ghost_host_tests` pass unchanged.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 93da74f6 -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, run once each.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** in the edit order, writing each red test before its fix.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run and the mutation run, **as output, not summarized**;
   - the two greps.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may add the `rx` parameter to `dispatch_meta::dispatch`. It already
  carries `#[allow(clippy::too_many_arguments)]`.

## Out of scope

- **Session approval for spawns.** Each spawn asks.
- **Validating the runbook before the prompt.** A missing runbook is
  reported after approval, as today.
- **`cancel_schedule` and `delete_schedule`.** That is phase 07.
- **Budget caps for spawns.** The exemption is the stated consequence.

## Update Log

<!-- entries appended below this line -->
