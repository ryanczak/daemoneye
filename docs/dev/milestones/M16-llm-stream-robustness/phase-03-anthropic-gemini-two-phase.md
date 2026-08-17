# Phase 03: Anthropic + Gemini two-phase timeouts; retire `stream_chunk`; connect-timeout-only client

**Milestone:** M16 — LLM Stream Robustness
**Status:** todo
**Depends on:** phase-02
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Repeat the phase-02 conversion for the Anthropic and Gemini backends, then —
with all three backends carrying their own two-phase timeouts — delete the
now-unused `stream_chunk` / `STREAM_IDLE_TIMEOUT` pair and rebuild the shared
`http()` client with **only** `.connect_timeout`. After this phase a
generation may run arbitrarily long as long as tokens keep arriving; only a
genuine stall (no first token in 600 s / no data for 240 s mid-stream) or a
transport drop ends it, each with an accurate message.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1.5 — mechanism C.
- `docs/dev/milestones/M16-llm-stream-robustness/phase-02-openai-two-phase.md`
  § Spec — the template this phase repeats (read the Task 1 shape and the
  decision tail; the same rules apply verbatim, including **no retry of any
  kind once `first_token_seen` is true**).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phase-02 landed: `grep -c "stream_next_with_timeout" src/ai/backends/openai.rs`
   must print `1`. If `0`, stop and file a blocker. Read the landed
   `OpenAiClient::chat` — it is the authoritative worked example for this
   phase; where this doc and that code disagree on the loop shape, follow the
   code.

## Current state

(Current as of 2026-08-16; re-derive with
`grep -n "stream_chunk" src/ai/backends/*.rs src/ai/mod.rs`.)

- `src/ai/backends/anthropic.rs` — drain loop at ~lines 209–212:
  `'outer: while let Some(chunk) = stream.next().await { let bytes =
  crate::ai::stream_chunk(chunk)?; sse.push(&bytes)?; ... }` with `[DONE]`
  breaking `'outer`. Header exchange via `send_with_retry` ~line 186. Events
  worth counting as "first token": `content_block_start` and
  `content_block_delta` message types (text, thinking, and tool_use input
  deltas). `message_start` only carries usage and `ping` events are
  keepalives — neither counts.
- `src/ai/backends/gemini.rs` — same shape at ~lines 211–214 (no `'outer`
  label; the SSE data lines are JSON objects with `candidates`). "First
  token" = a candidate whose `content.parts` yields a non-empty text part or
  a `functionCall` (the places the code already emits `AiEvent::Token` /
  tool-call events). A bare `finishReason`/usage-only frame does not count.
- `src/ai/mod.rs` — `STREAM_IDLE_TIMEOUT` (line ~123) and `stream_chunk`
  (~130–147) will have **zero remaining callers** once both backends convert
  (`grep -rn "stream_chunk" src/ --include=*.rs` then shows only the
  definition and its tests). The shared client (~197–206):

```rust
pub fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .read_timeout(STREAM_IDLE_TIMEOUT)
            .build()
            // INVARIANT: default reqwest client config is always valid
            .unwrap()
    })
}
```

- `mod stream_idle_tests` (~src/ai/mod.rs:499–553) holds
  `silent_after_first_chunk()` — a hermetic TCP listener that serves one SSE
  chunk then goes silent — and a test asserting the **old** `.read_timeout` +
  `stream_chunk` mechanism. The listener helper is kept; the test is
  rewritten against the new mechanism (Task 5).

## Spec

### Task 1 — Convert `AnthropicClient::chat` to the attempt loop

Apply the phase-02 Task 1 shape to `src/ai/backends/anthropic.rs`: outer
`'attempt: loop`; per-attempt state (`stream`, `sse`, `usage`, tool
accumulators, thinking accumulators); drain loop via
`crate::ai::stream_next_with_timeout(&mut stream, timeout, first_token_seen)`
with `timeout = crate::ai::select_timeout(first_token_seen,
crate::ai::stream_timeouts())`; the identical decision tail (both retry
classes gated on `!first_token_seen`; `record_stream_success()` on natural
end; `record_stream_failure()` before returning the final `Err`). In-stream
`bail!` sites inside the drain become `break <label> Err(...)`.

Set `first_token_seen = true` when `msg_type` is `"content_block_start"` or
`"content_block_delta"` (place the assignment right after `msg_type` is
extracted, guarded on `!first_token_seen`). Do **not** flip it on
`message_start`, `ping`, or `message_delta`.

### Task 2 — Convert `GeminiClient::chat` the same way

Same restructure for `src/ai/backends/gemini.rs`. Set
`first_token_seen = true` at the two emission sites: where a non-empty text
part sends `AiEvent::Token`, and where a `functionCall` (including the
`MALFORMED_FUNCTION_CALL` recovery path) produces a tool-call event — in each
case immediately **before** the send. A frame carrying only
`finishReason`/`usageMetadata` must not flip it.

### Task 3 — Delete `stream_chunk` and `STREAM_IDLE_TIMEOUT`

In `src/ai/mod.rs`, delete the `stream_chunk` function, the
`STREAM_IDLE_TIMEOUT` const, and their doc comments. Both are now unused
outside their own tests (verify first:
`grep -rn "stream_chunk\|STREAM_IDLE_TIMEOUT" src/ --include=*.rs` must show
only `src/ai/mod.rs` definition + test sites before you delete). Clippy is
authoritative for liveness — run the lint gate after this task.

### Task 4 — Rebuild `http()` with connect-timeout only

Replace the builder body:

```rust
pub fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // No total `.timeout` and no `.read_timeout`: a streamed
            // generation may legitimately run for many minutes, and every
            // backend now bounds its own reads via stream_next_with_timeout
            // (first-token / idle budgets from `[ai]` config). A client-level
            // total timeout would kill long streams mid-response and
            // misreport them as transport errors.
            .connect_timeout(crate::ai::stream_timeouts().connect)
            .build()
            // INVARIANT: default reqwest client config is always valid
            .unwrap()
    })
}
```

### Task 5 — Rewrite the silent-socket stall test against the new mechanism

In `mod stream_idle_tests` (`src/ai/mod.rs`): keep
`silent_after_first_chunk()` unchanged. Replace
`idle_stream_times_out_and_reports_a_stall` with
`idle_stream_stall_is_reported_by_stream_next_with_timeout`:

- Build a plain client (`reqwest::Client::new()` — no read_timeout), GET the
  silent URL, take `bytes_stream()`.
- First read via `stream_next_with_timeout(&mut stream,
  Duration::from_millis(500), false)` → must be `Some(Ok(_))`.
- Second read via `stream_next_with_timeout(&mut stream,
  Duration::from_millis(300), true)` → must be `Some(Err(e))` with the
  message containing `"idle mid-response"`.

This test uses a real socket and real (sub-second) timeouts, matching the
existing test's convention in this module — do not convert it to paused time
(paused time does not advance across real socket I/O).

### Task 6 — Unit tests for the first-token predicates

- In `anthropic.rs` tests: `first_token_flips_on_content_block_not_ping` —
  exercise whatever helper/inline predicate you introduced with the message
  types `"ping"`, `"message_start"` (no flip) and `"content_block_delta"`
  (flip). If the assignment is inline (no helper), test it by extracting a
  small pure predicate `fn is_content_event(msg_type: &str) -> bool` and
  asserting `is_content_event("content_block_delta")`,
  `is_content_event("content_block_start")` are true and
  `is_content_event("ping")`, `is_content_event("message_start")`,
  `is_content_event("message_delta")` are false — then use it at the
  assignment site.
- In `gemini.rs` tests: `finish_reason_only_frame_is_not_a_token` — with the
  same extract-a-predicate approach if needed: a frame
  `{"candidates":[{"finishReason":"STOP"}]}` must not count as a token; a
  frame with a non-empty text part must.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-03.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -rc "stream_chunk" src/ai/mod.rs src/ai/backends/anthropic.rs src/ai/backends/gemini.rs src/ai/backends/openai.rs`
      prints `0` for every file (as of drafting: `3`, `1`, `1`, and `1` for
      openai — the openai count drops to `0` when phase-02 lands, before
      this phase runs).
- [ ] `grep -c "STREAM_IDLE_TIMEOUT" src/ai/mod.rs` prints `0` (currently 4).
- [ ] `grep -c "connect_timeout" src/ai/mod.rs` prints `1` (currently `0`).
- [ ] `grep -c "read_timeout" src/ai/mod.rs` prints `0` (currently `2`; Task
      5's rewritten test uses a plain client with no `read_timeout`, so both
      occurrences go away).
- [ ] `cargo test idle_stream_stall_is_reported_by_stream_next_with_timeout`
      passes.
- [ ] `cargo test first_token` passes (the new predicate tests).
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tasks 5–6. All existing anthropic/gemini parsing tests must keep passing
unchanged.

## End-to-end verification

The silent-socket test **is** the end-to-end artifact for the stall path (a
real TCP stream through the real helper). Capture:

```sh
A=/tmp/e2e-03.txt; : > "$A"
grep -rc "stream_chunk" src/ai/mod.rs src/ai/backends/anthropic.rs src/ai/backends/gemini.rs src/ai/backends/openai.rs >> "$A"
grep -c "STREAM_IDLE_TIMEOUT" src/ai/mod.rs >> "$A"
grep -c "connect_timeout" src/ai/mod.rs >> "$A"
grep -c "read_timeout" src/ai/mod.rs >> "$A"
cargo test idle_stream_stall_is_reported_by_stream_next_with_timeout 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (append the verdict line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-03-anthropic-gemini-two-phase.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03.txt
diff /tmp/pasted-03.txt /tmp/e2e-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any daemon (`src/daemon/`) or CLI (`src/cli/`) changes.
- Retrying after `first_token_seen` — forbidden, same as phase-02.
- Changing `SseBuffer`, `send_with_retry`, or the Retry-After logic.
- The Gemini `MALFORMED_FUNCTION_CALL` recovery logic (only the
  first-token flag is added near it).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
