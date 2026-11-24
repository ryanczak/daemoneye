# Phase 08 — ghost-turn-errors

**Milestone:** M39 — Critical fixes
**Status:** in-progress
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

A ghost run whose model call fails is reported as a success. The audit
calls this C2; the architect re-verified it on the current tree on
2026-11-24.

- In `do_ghost_turn` (`src/daemon/ghost/turn.rs`), the spawned chat task
  logs a `chat()` error and drops its sender.
  `collect_ghost_stream_turn` (`src/daemon/ghost/turn_helpers.rs`) treats
  the closed channel (`Ok(None)`) as a normal end of turn. It returns an
  empty turn with no tool calls, the loop ends, and the run is recorded as
  `Completed`. Its mailbox holds an empty answer.
- The same path catches a stream that closes without an `AiEvent::Done`
  for any other reason.
- The spawned task's handle is discarded. When the turn ends early (the
  300 s timeout, or an `AiEvent::Error`), the request keeps running.
- A run cancelled while the model is streaming isn't noticed until the
  stream ends or times out. Only `prepare_ghost_turn_step` checks the
  cancel signal, and it runs between turns.

After this phase each of those cases has a definite outcome. An error, or
a stream that ends without `Done`, makes the run end `Failed`, and the
error is carried through to that result. A cancel ends the run
`Cancelled` promptly. The chat task never outlives its turn.

## Statement of work

1. **A `chat()` error reaches the turn as `AiEvent::Error`.** The task
   sends the error's text as an `AiEvent::Error` before its sender drops.
   The collector's existing `AiEvent::Error` arm then fails the turn.
   Move the spawn into its own function so a test can drive it with a
   test double of `AiClient`. You choose the name.
2. **A stream that ends without `Done` fails the turn.** `Ok(None)` before
   any `AiEvent::Done` returns an error saying the stream ended without
   completing. It also logs a `ghost_error` event and calls
   `metrics.record_backend_error()`, as the `AiEvent::Error` arm does.
3. **The chat task ends with its turn.** Every way the turn ends (`Done`,
   error, end of stream, timeout, cancel) aborts the spawned task if it
   is still running. A drop guard holding the `JoinHandle` (or its
   `AbortHandle`) fits.
4. **A cancel during streaming ends the run `Cancelled`.** While waiting
   for stream events, the collector also waits on the run's cancel
   signal. When the signal fires, the turn stops, the chat task is
   aborted, `GhostLoopOutcome.cancelled` is set and the loop ends.
   `do_ghost_turn` must return `Ok` in that case: `ghost_terminal_state`
   puts an `Err` ahead of `cancelled`, so returning an error would record
   `Failed`.

**Constraints:**

- A healthy turn behaves exactly as before. That means `Token`, the tool
  events, then `Done`: the same content, the same pending calls, and its
  cost recorded once.
- The 300 s turn timeout still fails the turn, as it does today.
- `RunCancelSignal::cancelled` takes `&mut self`. Clone the signal if you
  need a mutable one.
- **Gotcha, paused-time tests:** under `#[tokio::test(start_paused = true)]`
  tokio advances the clock whenever every task is idle. A test whose
  stream never sends anything therefore hits the 300 s timeout at once.
  Fire the cancel, or drop the sender, before that wait can win, or don't
  pause time in that test.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.

**Scoping hints.** Start with these:
- `src/daemon/ghost/turn.rs`: `do_ghost_turn`, the spawn.
- `src/daemon/ghost/turn_helpers.rs`: `collect_ghost_stream_turn` and
  `prepare_ghost_turn_step`, whose cancel handling is the analogue.
- `src/daemon/ghost.rs`: `GhostLoopOutcome` and `ghost_terminal_state`.
  Read them; change them only if item 4 needs it.
- `src/daemon/run/cancel.rs`: `RunCancelSignal`. Read only.
- `src/ai/mod.rs`: the `AiClient` trait, for the test double.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **10** tool calls.

## First red test

**Write this before any production change.** Add
`ghost_stream_closed_without_done_fails_the_turn`. Feed
`collect_ghost_stream_turn` a channel that sends one `Token` and then
drops its sender with no `Done`. Assert that it returns an error. Run it
red: today it returns `Ok`.

## Acceptance criteria

Each item is a named test that fails on the current tree, unless marked
preservation. Paste each red run into the Update Log before its fix.

- [ ] **No `Done`:** `ghost_stream_closed_without_done_fails_the_turn`
      passes.
- [ ] **Chat error:** `ghost_chat_error_reaches_the_turn_as_an_error`
      passes. A test double's `chat()` returns `Err("boom")`. The turn
      returns an error whose text contains `boom`.
- [ ] **Abort:** `ghost_chat_task_is_aborted_when_the_turn_ends` passes. A
      double whose `chat()` never finishes holds a value that signals when
      it is dropped. After the turn ends, by timeout or by cancel, the
      signal has fired.
- [ ] **Cancel:** `ghost_cancel_during_streaming_ends_the_turn_cancelled`
      passes. Cancel the run while the stream is open. The collector
      returns well before the 300 s timeout, with a value that says the
      turn was cancelled and no error. Test the collector only; the
      mapping onto `GhostLoopOutcome.cancelled` in `do_ghost_turn` is
      checked at review by reading it.
- [ ] **Preservation:** `ghost_healthy_stream_turn_is_unchanged` passes.
      Tokens, one tool event and then `Done` give the same content, one
      pending call and `Ok`. It is green today.
- [ ] **Mutation check,** with the output pasted: put `Ok(None) => break`
      back, run the no-`Done` test and see it fail, then restore it.
- [ ] **Gates:** all four pass, and the lib suite reports **at least 3098**
      passed (3093 plus five new tests).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** work through the list, each test red before its code.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, including the
   mutation check. Done when every item is checked with its evidence named.
4. **Capture the evidence:** run the four gates. Paste each `test result:`
   line, your red runs and the mutation check into a new Update Log entry,
   without the per-test `… ok` lines. Commit as one `fix:` commit.

## Authorizations

- You may touch `src/daemon/ghost/turn.rs`,
  `src/daemon/ghost/turn_helpers.rs` and `src/daemon/ghost.rs`, including
  its test module.
- You may not change the `AiClient` trait or any backend.

## Out of scope

- The interactive chat turn (`src/daemon/stream/`), which already handles
  a closed stream (`end_stream_closed`).
- Retrying a failed model call.
- User-facing docs. The architect writes them at review.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-11-24 (refined re-dispatch)

**Read this first. The first run stalled after 241 turns with no code
edits. It spent turns 44–241 reading `SessionCache`, `ScheduleStore`,
sandboxes, shells and `Briefing`, to build fixtures for driving
`do_ghost_turn` end to end. No test in this phase drives `do_ghost_turn`.**
Your break-down entry and task list below are kept; don't rewrite them.

**Every test drives `collect_ghost_stream_turn`, the new spawn function,
or both.** Their fixtures are all one line each:

- `SessionStore::new()`, plus a ghost entry inserted as
  `ghost_tool_turn_persists_calls_then_results` in `src/daemon/ghost.rs`
  does it.
- `crate::ipc::GhostConfig::default()`.
- `crate::config::ModelEntry { provider: "anthropic".into(), model:
  "test".into(), ..Default::default() }`.
- `RunId::new()` and `RunMetrics::default()`.
- `let (handle, signal) = RunCancelSignal::new();` then
  `handle.cancel(CancelReason::UserStop)` to fire it.
- Take `crate::test_home_guard()` and point `HOME` at a tempdir. `Done`
  records cost, which writes under `HOME`.
- Build `AiEvent`s and send them on an `unbounded_channel` yourself. Only
  the chat-error and abort tests need an `AiClient` double, a small
  struct in the test module.

**The analogue for item 3** is `ChatTaskGuard` in `src/daemon/stream.rs`.
It is a drop guard over the chat task's `JoinHandle` that aborts on drop.
Mirror it for the ghost; don't import the private one.

**Closed file list:**
- `src/daemon/ghost/turn_helpers.rs`
- `src/daemon/ghost/turn.rs`
- the `#[cfg(test)]` module of `src/daemon/ghost.rs`, if you put the tests
  there

You need no other file.

**Edit order:**
1. Your **first tool call** writes
   `ghost_stream_closed_without_done_fails_the_turn`, then run it red.
2. Fix the `Ok(None)` arm.
3. Write the spawn function and the guard, and add the cancel parameter
   to the collector.
4. Write the other four tests.
5. Run the mutation check, then the gates.

Read at most **5** files before your first edit, and only from the closed
list.

### Update — 2026-11-24 06:47 (started)

**Executor:** executor (headless run). Status flipped `todo` → `in-progress`.

**Task list** (from the Spec; each names the acceptance item it closes):

1. Write the first red test `ghost_stream_closed_without_done_fails_the_turn` in `turn_helpers.rs`, run it red (returns `Ok` today) — closes **No `Done`**.
2. Make `collect_ghost_stream_turn` treat `Ok(None)` before `Done` as an error: bail with a "stream ended without completing" message, log a `ghost_error` event and `metrics.record_backend_error()`, same as the `AiEvent::Error` arm — closes **No `Done`**.
3. Move the chat spawn in `do_ghost_turn` into its own function taking the `AiClient` as a parameter (test seam), and send the `chat()` error's text as `AiEvent::Error` before the sender drops — closes **Chat error**.
4. Test `ghost_chat_error_reaches_the_turn_as_an_error`: a double whose `chat()` returns `Err("boom")`; the turn errors with text containing `boom` — closes **Chat error**.
5. Add a drop guard holding the spawned task's `JoinHandle` so every turn end (Done, error, stream close, timeout, cancel) aborts a still-running chat task — closes **Abort**.
6. Test `ghost_chat_task_is_aborted_when_the_turn_ends`: a double whose `chat()` never finishes holds a drop signal; after the turn ends the signal has fired — closes **Abort**.
7. While waiting for stream events, the collector also selects on the run's cancel signal; on cancel, stop the turn, abort the task, set `GhostLoopOutcome.cancelled`, end the loop with `do_ghost_turn` returning `Ok` — closes **Cancel**.
8. Test `ghost_cancel_during_streaming_ends_the_turn_cancelled`: cancel while the double streams; the collector returns well before the 300 s timeout and the outcome has `cancelled` set with no error — closes **Cancel**.
9. Preservation test `ghost_healthy_stream_turn_is_unchanged`: `Token`, one tool event, `Done` — same content, one pending call, `Ok`, cost recorded once — closes **Preservation**.
10. Mutation check: restore `Ok(None) => break`, run the no-`Done` test red, restore — closes **Mutation check**.
11. Run the four gates, paste outputs plus red runs and mutation check into the end-to-end Update Log entry, commit as one `fix:` commit — closes **Gates**.

### Update — 2026-11-24 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** the stall came from a spec gap. The Cancel criterion's
"loop's outcome" sent the executor looking for `do_ghost_turn` fixtures it
never needed, so naming the collector-level fixtures, a closed file list
and an edit order fixes the cause. This is the first failure, so a
takeover isn't warranted.
