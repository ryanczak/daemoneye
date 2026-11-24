# Phase 08 — ghost-turn-errors

**Milestone:** M39 — Critical fixes
**Status:** todo
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
      passes. Cancel the run while the double is streaming. The collector
      returns well before the 300 s timeout, and the loop's outcome has
      `cancelled` set with no error.
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
