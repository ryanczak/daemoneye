# Phase 07 — ghost-turn-history

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

A ghost run breaks on its second model turn, for every provider. The audit
calls this C1. The architect re-verified it on the current tree on
2026-11-24.

`persist_ghost_turn_message` (`src/daemon/ghost/turn_helpers.rs`) saves one
turn as **one** `assistant` message that carries both `tool_calls` and
`tool_results`. Each backend's `convert_messages` (`src/ai/backends/`
`anthropic.rs`, `openai.rs` and `gemini.rs`) checks `tool_results` first, in
an `if … else if` chain. When a message carries results, the converter
emits only those and drops the message's tool calls and its assistant text.
The next request then holds tool results that answer no call. The audit
reports this as HTTP 400 from Anthropic, OpenAI and Gemini.

Chat already saves the correct shape (`conversation_helpers.rs`, the end
of its tool batch): an `assistant` message with the tool calls, then a
`user` message with empty content and the results. After this phase a
ghost turn saves the same two messages.

## Statement of work

1. **Two messages per tool turn.** When a ghost turn made tool calls,
   `persist_ghost_turn_message` records two messages, in this order. Both
   carry the same `turn` number, and both are written to the session
   history **and** to the session file (`append_session_message`).
   - An `assistant` message with the turn's text and `tool_calls`, and
     `tool_results: None`.
   - A `user` message with empty `content`, `tool_calls: None` and the
     `tool_results`.
2. **A turn without tool calls is unchanged.** It still records exactly one
   `assistant` message, with no `tool_calls` and no `tool_results`.
3. **Every call has its result.** Every tool call id in the assistant
   message has exactly one result in the following user message. If a
   result is missing for a call (an outcome that yields none), record one
   that says so; the converter then sees a pair.
4. **Readers of ghost history still work.** Some code reads a ghost's
   history and may expect the old one-message shape: the uncovered-steps
   nudge, briefing generation, the mailbox's final response and the
   `/agents` view. Find each such reader. Make it work with the two-message
   shape, or record in the Update Log why it already does.

**Constraints:**

- Do not change the converters' behaviour. The fix is the shape that is
  saved. You may raise the visibility of `convert_messages` in `openai.rs`
  and `gemini.rs` to `pub(crate)` so a test can call them; Anthropic's is
  already `pub`.
- Tests that write session files take `crate::test_home_guard()`, and take
  it **once**: it is not reentrant.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.

**Scoping hints.** Start with these:
- `src/daemon/ghost/turn_helpers.rs`: `persist_ghost_turn_message` and its
  neighbours.
- `src/daemon/ghost/turn.rs`: the caller.
- `src/daemon/stream/conversation_helpers.rs`: the chat analogue. Read it;
  do not edit it.
- `convert_messages` in the three backends: read them, and change only
  their visibility.

Widen only for item 4's readers, and log each one you find.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **10** tool calls.

## First red test

**Write this before any production change.** Add
`ghost_tool_turn_persists_calls_then_results`. Persist one ghost turn with
two tool calls and two results, then assert that the session's history
holds an `assistant` message with both calls and no results, followed by a
`user` message with both results. Run it red.

## Acceptance criteria

Each item is a named test that fails on the current tree, unless marked
preservation. Paste each red run into the Update Log before its fix.

- [ ] **Shape:** `ghost_tool_turn_persists_calls_then_results` passes. It
      also checks that the session file holds the same two lines, in the
      same order.
- [ ] **Providers:** `ghost_two_turn_history_converts_for_every_provider`
      passes. It builds a two-turn ghost history through
      `persist_ghost_turn_message`: turn 1 with one tool call and its
      result, turn 2 a plain answer. It converts that history with all
      three backends and asserts that each result's call id is preceded by
      a call with the same id:
      - Anthropic: a `tool_use` block before the `tool_result` with that
        `tool_use_id`.
      - OpenAI: an assistant `tool_calls` entry before the `role: "tool"`
        message with that `tool_call_id`.
      - Gemini: a `functionCall` before the `functionResponse` for that
        call.
- [ ] **Preservation:** `ghost_final_turn_persists_one_message` passes. A
      turn with no tool calls records exactly one `assistant` message. It
      is green today.
- [ ] **Item 4:** the Update Log lists every reader of ghost history you
      found, each with "changed" or "already fine" and the reason.
- [ ] **Mutation check,** with the output pasted: put the results back on
      the assistant message, run the providers test and see it fail, then
      restore it.
- [ ] **Gates:** all four pass, and the lib suite reports **at least 3093**
      passed (3090 plus three new tests).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** work through the list, each test red before its code.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, including the
   item 4 list and the mutation check. Done when every item is checked
   with its evidence named.
4. **Capture the evidence:** run the four gates. Paste each `test result:`
   line, your red runs and the mutation check into a new Update Log entry,
   without the per-test `… ok` lines. Commit as one `fix:` commit.

## Authorizations

- You may touch `src/daemon/ghost/turn_helpers.rs`,
  `src/daemon/ghost/turn.rs` and the readers item 4 finds.
- You may change `convert_messages`' visibility in `openai.rs` and
  `gemini.rs`, and nothing else in the backends.

## Out of scope

- Truncating ghost tool results to `[limits] tool_result_chars`, as chat
  does.
- A converter that tolerates a message carrying both calls and results.
  Session files saved before this phase keep the old shape. That is
  harmless: a ghost run keeps its history in memory for the whole run and
  never reloads it from disk (`SessionEntry::from_disk` rebuilds chat
  sessions only).
- Ghost turn errors and the end of a stream without `Done`. That is
  phase 08.
- User-facing docs. The architect writes them at review.

## Update Log

<!-- entries appended below this line -->
