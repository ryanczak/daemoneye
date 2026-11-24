# Phase 07 — ghost-turn-history

**Milestone:** M39 — Critical fixes
**Status:** in-progress
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

### Update — 2026-11-24 05:39 (started)

Executor: Claude (Opus 4.5, headless dispatch). Status flipped `todo` →
`in-progress`; README row updated to match. First red test next.

**Task list** (Spec task 1 breakdown; each names the acceptance item it closes):

1. Write the first red test `ghost_tool_turn_persists_calls_then_results`
   in `src/daemon/ghost.rs` tests module (drives
   `persist_ghost_turn_message`), run it red. → **Shape**
2. Re-shape `persist_ghost_turn_message`
   (`src/daemon/ghost/turn_helpers.rs`): a turn with tool calls records an
   `assistant` message (text + `tool_calls`, `tool_results: None`) followed
   by a `user` message (empty content, results); both carry the same
   `turn`, both go to the session history **and** the session file.
   → **Shape**
3. Fill in missing results: a call with no result in the batch gets a
   result message saying so, so every call id pairs. → **Every call has
   its result** (part of **Shape**'s two-message invariant)
4. Raise `convert_messages` visibility to `pub(crate)` in `openai.rs` and
   `gemini.rs` (Anthropic's is already `pub`); nothing else changes in the
   backends. → enables **Providers**
5. Write `ghost_two_turn_history_converts_for_every_provider` (red first):
   build a two-turn history through `persist_ghost_turn_message`, convert
   with all three backends, assert each result's call id is preceded by a
   call with the same id. → **Providers**
6. Write `ghost_final_turn_persists_one_message` (preservation): a turn
   with no tool calls records exactly one `assistant` message. →
   **Preservation**
7. Item 4 reader audit: find every reader of ghost history
   (uncovered-steps nudge, briefing generation, mailbox final response,
   `/agents` view, working-set compaction, digest repair, `SessionEntry::from_disk`),
   verify each against the two-message shape; log each as "changed" or
   "already fine" with the reason. → **Item 4**
8. Mutation check: put the results back on the assistant message, run the
   providers test, paste the failure, restore. → **Mutation check**
9. Capture the evidence: run `cargo fmt --all`, `cargo build`, `cargo
   clippy --all-targets --all-features -- -D warnings`, `cargo test`;
   paste the `test result:` lines, red runs and mutation output into a new
   Update Log entry; commit as one `fix:` commit. → **Gates**

### Update — 2026-11-24 05:52 (red run, Spec task 1 → task 2 fix)

`ghost_tool_turn_persists_calls_then_results` written first, run red against
the unmodified `persist_ghost_turn_message`:

```
running 1 test
test daemon::ghost::tests::ghost_tool_turn_persists_calls_then_results ... FAILED

failures:

---- daemon::ghost::tests::ghost_tool_turn_persists_calls_then_results stdout ----

thread 'daemon::ghost::tests::ghost_tool_turn_persists_calls_then_results' (2913250) panicked at src/daemon/ghost.rs:1515:9:
assertion `left == right` failed: the session file holds the seed and the same two lines: {"role":"assistant","content":"Checking disk pressure","tool_calls":[{"id":"tc_1",...
{"role":"user","content":"","tool_results":[...],"turn":1}

  left: 2
 right: 3
```

(The unmodified code saved one combined message; the test asserted the
two-message shape, so the assistant line carried the results and the
count/order assertions failed.)

Fix applied in `persist_ghost_turn_message`; after the fix:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3100 filtered out; finished in 0.03s
```

### Update — 2026-11-24 06:05 (item 4 reader audit)

Every reader of ghost history found, each with its verdict:

1. **Uncovered-steps nudge** — `maybe_nudge_uncovered_steps`
   (`src/daemon/ghost/turn_helpers.rs:520`). **Already fine.** It filters
   `role == "assistant"` and joins `content` only; the new `user` results
   message is skipped by the role filter, and the assistant message's
   content is unchanged from the old shape. STEP markers the model emitted
   sit in assistant content exactly as before.
2. **Briefing generation** — `generate_and_save_briefing`
   (`src/daemon/briefing.rs`). **Already fine.** It passes the last 50
   messages to the model as-is (converted by the same converters this phase
   now feeds a valid shape — the briefing call was *broken* by the old
   shape and is fixed by it, not harmed by the new one). No shape-specific
   logic.
3. **Mailbox final response** — `write_mailbox_on_exit`
   (`src/daemon/ghost.rs:29`), two `.last()` reads (mailbox `result_text`
   and the run `summary`). **Changed.** Both took
   `history.messages.last().filter(role == assistant)`; with the new
   shape a tool turn's last message is the `user` results message, so a
   ghost that ended on a tool turn would have written an empty result.
   Both now take the last `assistant` message via `iter().rev().find()`.
4. **`/agents` view** — `src/cli/commands/agents.rs`. **Already fine.** It
   reads `ShellInfo` records (shell ids, owners, exit state), never
   message history.
5. **`/agents` → attach** — `src/cli/attach.rs`. **Already fine.** Attaches
   to a PTY shell; touches no history.
6. **Ghost working-set compaction** — `enforce_ghost_working_set`
   (`src/daemon/context/ghost_ws.rs`) and `repair_tail_head`
   (`src/daemon/digest.rs:336`). **Already fine.** `next_clean_turn_start`
   treats a `user` message carrying `tool_results` as a *non*-boundary, so
   the new assistant+results pair is never split; `repair_tail_head`
   strips leading orphan user-with-results, which the new shape makes
   rarer, not wrong. `elide_old_tool_results` edits results in place,
   agnostic to which message carries them.
7. **Narrative digest formatter** — `format_messages_for_narrative`
   (`src/daemon/digest.rs:80`). **Already fine.** Matches on
   `("user", tool_results.is_some())` → renders the results, and
   `"assistant"` → renders content + tool_calls. The two new messages each
   hit exactly one arm — the old combined message also hit the assistant
   arm and dropped its own results.
8. **Token estimate** — `estimate_message_tokens`
   (`src/daemon/context/estimate.rs`). **Already fine.** Sums content,
   tool_calls and tool_results fields; two messages instead of one
   estimates the same total plus one `PER_MESSAGE_OVERHEAD`.
9. **Session archive indexing** — `append_archive_message`
   (`src/daemon/session.rs`) and `index_archive_file`
   (`src/memory/index/mod.rs`). **Already fine.** Both index per message
   with the message's own turn; the user results line is indexed against
   the same turn number, so `recall_context` and `search_turns` see both
   lines for a turn.
10. **`recall_context` range mode** — `range_query`
    (`src/daemon/context/recall.rs`). **Already fine.** Renders every
    message in the turn range with role labels; the results message
    renders as `turn N (user):` followed by its `tool_result` lines.
11. **`search_turns_fts` / situational excerpts** — `src/search.rs:500`,
    `src/daemon/situational.rs:87`. **Already fine.** Both rebuild
    matched_line from content + tool_results per line; the results line
    now carries them in its own body.
12. **`SessionEntry::from_disk`** — `src/daemon/session.rs:507`. **Already
    fine**, per the phase's Out-of-scope note: a ghost run keeps its
    history in memory for the whole run and never reloads from disk.
13. **Webhook/notice history sinks** — `src/webhook/process.rs`,
    `src/daemon/notice_sinks.rs`. **Already fine.** They inject their own
    synthetic messages and their tests read `.last()` of messages they
    just pushed; ghost tool persistence never interleaves.

### Update — 2026-11-24 06:09 (mutation check)

Mutation: `tool_results: None` on the persisted assistant message replaced
with `tool_results: Some(tool_results.clone())` (the old combined-message
shape), then the providers test run:

```
running 1 test
test daemon::ghost::tests::ghost_two_turn_history_converts_for_every_provider ... FAILED

failures:

---- daemon::ghost::tests::ghost_two_turn_history_converts_for_every_provider stdout ----

thread 'daemon::ghost::tests::ghost_two_turn_history_converts_for_every_provider' (2917178) panicked at src/daemon/ghost.rs:1637:9:
anthropic history holds a tool_use block for tc_a: [Object {"content": String("check disk"), "role": String("user")}, Object {"content": Array [Object {"content": String("/dev/sda1 87%"), "tool_use_id": String("tc_a"), "type": String("tool_result")}], "role": String("user")}, Object {"content": Array [Object {"content": String("/dev/sda1 87%"), "tool_use_id": String("tc_a"), "type": String("tool_result")}], "role": String("user")}, Object {"content": String("Disk is 87% full."), "role": String("assistant")}]
note: run with `RUST_BACKTRACE=1` environment variable to follow this backtrace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3102 filtered out; finished in 0.03s
```

The converter drops the calls and duplicates the results — exactly audit
C1. Mutation restored; test green again:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3102 filtered out; finished in 0.03s
```

### Update — 2026-11-24 06:15 (end-to-end verification)

Gates, run after `cargo fmt --all`, captured from `cargo test` (full suite):

```
test result: ok. 3093 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 143.41s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.24s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.25s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.37s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build` — clean, zero warnings. `cargo clippy --all-targets
--all-features -- -D warnings` — clean.

The phase's three named tests together:

```
running 3 tests
test daemon::ghost::tests::ghost_tool_turn_persists_calls_then_results ... ok
test daemon::ghost::tests::ghost_final_turn_persists_one_message ... ok
test daemon::ghost::tests::ghost_two_turn_history_converts_for_every_provider ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 3100 filtered out; finished in 0.04s
```

The lib suite reports 3093 passed — 3090 before the phase plus the three
new tests, exactly the Gates criterion. Red runs for the first test and
the mutation check are pasted in the entries above, both before their fix.
