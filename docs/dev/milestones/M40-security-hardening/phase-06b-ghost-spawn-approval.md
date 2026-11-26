# Phase 06b — ghost-spawn-approval

**Milestone:** M40 — Security hardening
**Status:** in-progress
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

**Edit order (refined 2026-11-26, see the bounce notes):**
1. the wire variants;
2. the wire test;
3. the daemon gate and its two tests;
4. the clients;
5. the list and docs.

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

### Update — 2026-11-25 22:52 (started)

Work on phase 06b has begun. Status flipped to `in-progress`.

Task breakdown (numbered, each naming the acceptance item it closes):

1. Write the first red test `spawn_ghost_from_chat_asks_first` in
   `dispatch_meta.rs`'s tests, run it red — closes **Chat asks**.
2. Wire variants: `Response::GhostSpawnPrompt` and
   `Request::GhostSpawnResponse` in `src/ipc.rs`, plus the round-trip
   test `ghost_spawn_prompt_round_trips` in `src/ipc_tests.rs` — closes
   **Wire**.
3. Daemon gate: `dispatch_meta::dispatch` gains `rx`; the chat
   (`ghost_policy: None`) branch of the `SpawnGhost` arm prompts and
   awaits `GhostSpawnResponse` before `spawn_ghost`, denying on
   timeout/EOF/over-long/anything-but-approved — closes **Chat asks**
   and **Ghost does not ask**.
4. Clients: `stream_ask.rs`, new `prompt_ghost_spawn_ratatui` in
   `stream_prompts.rs` (never session-approved), `ask.rs` denies,
   `program_status_emit.rs` maps to `Blocked`/`Permission` (with a
   frame-state test in `program_status_emit_tests.rs`), `ipc_types.rs`
   variant name — closes **Status frames**.
5. Approval-gated list: `spawn_ghost_shell` into
   `APPROVAL_GATED_TOOLS` in `src/ai/tools/defs.rs`, `**⚠**` in
   README.md's tool table — closes **Gated list** (grep prints 1).
6. Docs: `docs/security.md` §10 sentence, CLAUDE.md row note,
   `doc_truth` test sync — closes **Docs in sync**.
7. Preservation checks: `ghost_narrow_tests` and `ghost_host_tests`
   pass; `git diff 93da74f6 -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
   prints 0 — closes **Preserved** and **No test removed**.
8. Mutation run: skip the prompt, confirm the red test fails, restore,
   `git diff --stat` clean — closes **Mutation run**.
9. Capture evidence: four gates once each, paste `test result:` lines,
   red run, mutation run, the two greps into an Update Log entry —
   closes **Capture the evidence** and **Gates**.

### Notes for executor — 2026-11-26 (refined re-dispatch)

**The previous run read about 20 files and made no code edit.** It was
stopped after 60 read-only turns. Every fact it went looking for is below,
checked against the tree on 2026-11-26. **Do not re-derive them, and do
not read a file to confirm one.** The working tree holds that run's status
flip and task list. Keep both.

**Your first tool call edits `src/ipc.rs`.**
- Add `GhostSpawnPrompt { id: String, runbook: String, agent:
  Option<String>, host: Option<String>, message: String }` to `Response`,
  directly after `ScheduleWritePrompt`. Mark `agent` and `host`
  `#[serde(default)]`.
- Add `GhostSpawnResponse { id: String, approved: bool }` to `Request`,
  directly after `ScheduleWriteResponse`.
- One-line doc comments only.

**Then follow the edit order. Each step gets one build:**

1. **`src/ipc_types.rs`:** add `Response::GhostSpawnPrompt { .. } =>
   "GhostSpawnPrompt"` beside the `ScheduleWritePrompt` arm.
2. **`src/ipc_tests.rs`:** add `ghost_spawn_prompt_round_trips` beside
   `response_schedule_write_prompt_roundtrip`. Use the same
   `roundtrip_resp` helper; for the request, round-trip with
   `serde_json`. Run that one test.
3. **`src/daemon/executor/dispatch_meta.rs`:**
   - add `rx: &mut R` after `tx`, with
     `R: tokio::io::AsyncBufReadExt + Unpin`;
   - in `route.rs`, pass `rx` at the `dispatch_meta::dispatch` call,
     where `rx` is already in scope.
   - **Already in scope** through `use super::*`: `send_response_split`,
     `Request`, `Response`, `USER_PROMPT_TIMEOUT` (a private const in
     `executor/mod.rs`, visible to the child module), `with_sessions` and
     `SessionStore`.
   - **Use full paths** for: `crate::daemon::utils::log_event`,
     `crate::ipc::read_bounded_line` and
     `crate::ipc::MAX_IPC_MESSAGE_BYTES`.
   - **The round trip** is the one in `schedule_command`, in
     `src/daemon/executor/schedule.rs`, between `send_response_split(…,
     Response::ScheduleWritePrompt…)` and the `let approved = …` match.
     Copy it, swapping in the new frame and `GhostSpawnResponse`.
4. **Tests in `dispatch_meta.rs`.** Add a `#[cfg(test)] mod tests`, with
   these facts:
   - `PendingCall::SpawnGhost { id, thought_signature: None, runbook,
     message, agent: None, host: None }`;
   - `crate::daemon::session::SessionStore::new()`;
   - `crate::test_home_guard()`, taken **once** at the top of each test;
   - the duplex pairs exactly as `dispatch_open_shell.rs`'s
     `harness::run_call` builds them;
   - read a frame with `crate::ipc::read_bounded_line`, then
     `serde_json::from_slice::<Response>`.
   - **GOTCHA, or the test hangs:** run the daemon side and the client
     side concurrently with `tokio::join!`. The daemon side is an
     `async move` block that **owns** its `tx` and drops it when
     `dispatch` returns. Then the client's read ends on EOF instead of
     waiting for ever. That is also how `spawn_ghost_from_a_ghost_sends_no_prompt`
     sees "no bytes".
   - Run each test once red, then once green.
5. **Clients**, at the sites
   `grep -rn "ScheduleWritePrompt\|ScheduleWriteResponse" src` lists:
   - `stream_ask.rs`: add `GhostSpawnPrompt` to the
     `ScriptDeletePrompt | ScriptWritePrompt | ScheduleWritePrompt`
     group, and an arm inside `handle_script_or_schedule_prompt` that
     calls the new `prompt_ghost_spawn_ratatui` and sends
     `GhostSpawnResponse`;
   - `stream_prompts.rs`: the new `prompt_ghost_spawn_ratatui`, written
     like `prompt_schedule_write_ratatui` but **with no
     `prompt_auto_approves` check**;
   - `ask.rs`: the deny arm, beside the `ScheduleWritePrompt` one;
   - `program_status_emit.rs`: the new variant in both prompt lists;
   - `program_status_emit_tests.rs`: a `(R::GhostSpawnPrompt {…},
     Kind::Permission)` entry beside the `ScheduleWritePrompt` one.
6. **List and docs**, as the statement of work's items 4 and 5 say.

**Budget rule:** after every **5** reads in a row, your next call must be
an edit or a `cargo` command.

### Update — 2026-11-26 (escalation)

**Chosen lever:** refined re-dispatch.

**Rationale:**
- The stall was read paralysis: 60 read-only turns and no code edit,
  against a 13-file spec.
- That is a spec-shape gap, not an unreachable task. The facts the run
  went hunting for are now in the spec:
  - the call shapes and the imports already in scope;
  - the test idioms, and the `tokio::join!` gotcha that keeps the test
    from hanging;
  - an edit order that starts with the smallest mechanical edit,
    `src/ipc.rs`.
- A takeover is held back for a second same-class stall.


### Update — 2026-11-26 (red run + mutation run, pasted as output)

The first red run of `spawn_ghost_from_chat_asks_first`, against the tree
before the daemon gate existed (the prompt frame had no sender yet; the
client half timed out waiting for it):

```
no GhostSpawnPrompt arrived within 5s
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::executor::dispatch_meta::tests::spawn_ghost_from_chat_asks_first

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3180 filtered out; finished in 5.04s
```

(The client half waits under a 5 s timeout so a missing prompt fails the
test instead of hanging the suite.)

The mutation run — the chat branch's approval call replaced with a bare
`true` (prompt skipped), one run of
`cargo test --lib spawn_ghost_from_chat_asks_first`, then restored; after
the restore `git diff --stat` matched the pre-mutation diff exactly
(15 files, +368/−10, no new or missing files):

```
no GhostSpawnPrompt arrived within 5s
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::executor::dispatch_meta::tests::spawn_ghost_from_chat_asks_first

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3180 filtered out; finished in 5.04s

error: test failed, to rerun pass `--lib`
```

### Update — 2026-11-26 (end-to-end verification)

The two pinned greps, run once each:

```
$ awk '/APPROVAL_GATED_TOOLS/,/\];/' src/ai/tools/defs.rs | grep -c spawn_ghost_shell
1

$ git diff 93da74f6 -- src | grep -cE '^-\s*#\[(tokio::)?test\]'
0
```

The four gates, run once each on the final tree:

```
$ cargo fmt --all
(no output, exit 0)

$ cargo build 2>&1 | tail -2
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.38s

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
    Checking daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.44s

$ cargo test 2>&1 | grep -E "test result:"
test result: ok. 3171 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 150.22s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.31s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.24s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The named acceptance tests, driven once against the real code paths:

- `daemon::executor::dispatch_meta::tests::spawn_ghost_from_chat_asks_first`
  (denied leg) and `approved_spawn_proceeds_to_load_the_runbook`
  (approved leg, proves the spawn went ahead once approved) —
  `test result: ok. 3 passed` (with
  `spawn_ghost_from_a_ghost_sends_no_prompt`, which asserts the client side
  reads 0 bytes after the daemon drops its writer);
- `ipc::tests::ghost_spawn_prompt_round_trips` —
  `test result: ok. 1 passed`;
- `program_status_emit` frame-state table including
  `(GhostSpawnPrompt, Kind::Permission)` — `test result: ok. 44 passed`;
- `daemon::stream::tests::approval_gated_contains_expected_tools` —
  `test result: ok. 1 passed`;
- `ghost_narrow` (5) and `ghost_host` (4) phase-06a tests pass unchanged;
- `cargo test --test doc_truth` — `test result: ok. 19 passed; 0 failed`.
