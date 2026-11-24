# Phase 11 — anthropic-usage

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

This phase fixes the Anthropic backend so cached turns are billed correctly
(audit row C4). `parse_anthropic_start_usage` (`src/ai/backends/anthropic.rs`)
treats Anthropic's `input_tokens` as the **total** prompt size. It subtracts
`cache_read_input_tokens` and `cache_creation_input_tokens` from it. But
Anthropic's `input_tokens` is **already** the uncached remainder. The
subtraction saturates to 0 on any cached turn, so a turn's uncached input
is under-reported (usually reported as 0) and under-billed.

The Anthropic API reference, which the executor cannot fetch, says:

> | `cache_creation_input_tokens` | Tokens written to cache this request |
> | `cache_read_input_tokens` | Tokens served from cache this request |
> | `input_tokens` | Tokens processed at full price (not cached) |
>
> **`input_tokens` is the uncached remainder only.** Total prompt size =
> `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.

`TokenBreakdown.input_tokens` (`src/ai/types/wire.rs`) is defined as the
uncached input billed at the full rate. So Anthropic's field maps onto it
**unchanged**. The OpenAI and Gemini backends report a total that includes
the cache, so their subtraction is correct. They are out of scope.

The architect checked this on 2026-11-24. `parse_anthropic_start_usage` is
the only place in `src/` that reads Anthropic's cache fields. It is called
from `AnthropicFrameHandler`'s `message_start` arm and from the tests.

## Statement of work

1. `parse_anthropic_start_usage` copies `input_tokens`,
   `cache_read_input_tokens` and `cache_creation_input_tokens` across as
   they are, with no subtraction. Its doc comment must state the API
   semantics.
2. Correct the existing test `anthropic_parses_cache_creation_and_read_tokens`.
   It pins the wrong semantics (`1500 - 800 - 200 = 500`). Its fixture must
   use realistic numbers, and its assertion must follow the documented
   meaning: `total()` equals all three input fields plus output.
3. Correct the doc comment on `TokenBreakdown::uncached_input_tokens`
   (`src/ai/types/wire.rs`). It says the value is uncached "after backend
   subtraction", which is now wrong for Anthropic. Say the backend reports
   it uncached, by subtracting the cache from a total where the provider
   reports one (OpenAI, Gemini), or as sent where it is already uncached
   (Anthropic). This is a comment edit only.

**Constraints:** no change to any other backend, to pricing or to cost
code. No `unwrap`, `expect` or `panic!` in production code.

**Test level.** Both new tests are unit tests in `anthropic.rs`'s test
module:
- one calls `parse_anthropic_start_usage` directly;
- one drives `AnthropicFrameHandler::on_frame` with a `message_start` frame,
  as `frame_handler_streams_tokens_flushes_tool_and_rejects_error_frame`
  already does.

**Closed file list:** `src/ai/backends/anthropic.rs` and
`src/ai/types/wire.rs` (the comment only). Read at most **3** files before
your first edit.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`anthropic_input_tokens_are_already_uncached`. Parse
`{"input_tokens": 50, "cache_read_input_tokens": 800, "cache_creation_input_tokens": 200}`.
Assert `input_tokens == 50`, `cache_read_tokens == 800` and
`cache_write_tokens == 200`. It is red today: the saturating subtraction
yields `input_tokens == 0`.

## Acceptance criteria

- [ ] **Uncached as sent:** `anthropic_input_tokens_are_already_uncached`
      passes. Paste the red run.
- [ ] **Stream path:** `anthropic_message_start_keeps_uncached_input` drives
      `on_frame` with a `message_start` frame whose usage has
      `input_tokens: 50` and both cache fields non-zero. It asserts the
      handler's `usage.input_tokens == 50`. Red today, so paste the red run.
- [ ] **Old test corrected:** `anthropic_parses_cache_creation_and_read_tokens`
      no longer asserts the subtracted value. Its `total()` assertion is
      the sum of all four fields. `grep -c '1500 - 800 - 200' src/ai/backends/anthropic.rs`
      prints **0** (it prints 1 today).
- [ ] **Preservation:** `anthropic_parses_zero_cache_when_not_present` passes
      unchanged.
- [ ] **Mutation check,** run once with the output pasted: put the
      `.saturating_sub(cache_read)` back, run
      `cargo test --lib anthropic_input_tokens_are_already_uncached` once,
      see it fail, then restore it.
- [ ] **Gates:** all four pass. The lib suite reports **at least 3106**
      passed (3104 plus two).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** fix the parser and both comments, correct the old test,
   and add the stream-path test. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste each
   `test result:` line, the two red runs, the grep and the mutation check
   into a new Update Log entry, without the per-test `… ok` lines. Commit
   as one `fix:` commit.

## Authorizations

- You may touch `src/ai/backends/anthropic.rs` and `src/ai/types/wire.rs`
  (the comment only).

## Out of scope

- The OpenAI and Gemini usage parsers.
- Pricing, cost records, and any back-fill of usage already logged.

## Update Log

<!-- entries appended below this line -->
