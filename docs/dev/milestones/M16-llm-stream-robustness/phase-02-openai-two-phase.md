# Phase 02: OpenAI backend — two-phase stream timeouts and bounded pre-first-token retry

**Milestone:** M16 — LLM Stream Robustness
**Status:** in-progress
**Depends on:** phase-01
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Convert `src/ai/backends/openai.rs` to the two-phase timeout + bounded-retry
shape using the phase-01 helpers. This is the **template backend** — phase-03
repeats the same shape for Anthropic and Gemini. After this phase a stalled
OpenAI stream errors with a phase-accurate message instead of relying on the
shared client's `.read_timeout`, and a request that dies before producing any
token is retried a bounded number of times instead of failing the turn.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1.5 — mechanism C.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phase-01 landed: `grep -c "fn stream_next_with_timeout" src/ai/mod.rs`
   must print `1`. If it prints `0`, stop and file a blocker.

## Current state

(Line numbers current as of 2026-08-16; re-derive with
`grep -n "stream_chunk\|bytes_stream" src/ai/backends/openai.rs`.)

`OpenAiClient::chat` (`src/ai/backends/openai.rs:151-298`) builds the body,
does one `send_with_retry` header exchange (~line 185), then drains the
stream through the shared client's implicit `.read_timeout`:

```rust
let mut stream = response.bytes_stream();
let mut calls: Vec<ToolCallAcc> = Vec::new();
let mut sse = crate::ai::SseBuffer::new();
let mut usage = TokenBreakdown::default();

'outer: while let Some(chunk) = stream.next().await {
    let bytes = crate::ai::stream_chunk(chunk)?;
    sse.push(&bytes)?;

    while let Some(data) = sse.next_data() {
        if data == "[DONE]" {
            break 'outer;
        }
        ...
    }
}
```

Tokens are emitted **live** (`tx.send(AiEvent::Token(...))` per delta, ~line
222) — unlike rexyMCP's buffered variant, output reaches the client as it
arrives. This is why **no retry of any kind is permitted once the first token
has been seen**: a re-issued request would duplicate text the user has
already read.

After the drain, accumulated tool calls are dispatched (~lines 258–294) and
`AiEvent::Done(usage)` is sent (~line 296). In-stream provider errors
`anyhow::bail!` out (~line 216); the daemon's spawn wrapper converts the `Err`
into `AiEvent::Error`.

Phase-01 provides in `src/ai/mod.rs`: `stream_timeouts()`,
`select_timeout(first_token_seen, t)`,
`stream_next_with_timeout(stream, timeout, first_token_seen)`,
`is_retriable_transport(&e)`, `stream_retry_backoff(attempt)`,
`record_stream_failure()`, `record_stream_success()`.

## Spec

### Task 1 — Restructure `OpenAiClient::chat` into an attempt loop

Wrap the request + drain in an outer `'attempt: loop`. Shape (adapted from
the rexyMCP template; follow this structure):

- **Before the loop** (whole-call state): `let mut first_token_seen = false;
  let mut stall_retries: u32 = 0; let mut transport_retries: u32 = 0;` and
  consts `MAX_FIRST_TOKEN_RETRIES: u32 = 2`, `MAX_STREAM_RETRIES: u32 = 3`.
- **Inside the loop, per attempt** (discarded on retry): the
  `send_with_retry` call, `stream`, `calls`, `sse`, `usage`.
- **The drain loop** replaces `stream.next().await` / `stream_chunk`:

```rust
let drain: anyhow::Result<()> = loop {
    let timeout = crate::ai::select_timeout(first_token_seen, crate::ai::stream_timeouts());
    match crate::ai::stream_next_with_timeout(&mut stream, timeout, first_token_seen).await {
        Some(Ok(bytes)) => {
            if let Err(e) = sse.push(&bytes) {
                break Err(e);
            }
            // existing sse.next_data() inner loop, unchanged except:
            //  - "[DONE]" breaks the drain loop with Ok(())
            //  - the in-stream provider-error bail becomes `break Err(...)`
            //  - set `first_token_seen = true` when a delta carries a real
            //    token (Task 2), before the Token send
        }
        Some(Err(e)) => break Err(e),
        None => break Ok(()),
    }
};
```

  (The current `'outer` label disappears; `[DONE]` and end-of-stream both
  resolve to `break <drain> Ok(())` — use a labeled loop if needed, e.g.
  `let drain = 'drain: loop { ... break 'drain Ok(()); ... };`.)

- **The decision tail**, after the drain loop — this is the worked example
  from the rexyMCP template, adapted to daemoneye's no-mid-stream-retry rule:

```rust
match drain {
    Ok(()) => {
        crate::ai::record_stream_success();
        // existing post-stream tool-call dispatch, unchanged
        // existing `tx.send(AiEvent::Done(usage))`, unchanged
        return Ok(());
    }
    Err(e) => {
        if !first_token_seen {
            if crate::ai::is_retriable_transport(&e) && transport_retries < MAX_STREAM_RETRIES {
                transport_retries += 1;
                log::warn!("AI stream transport error before first token (attempt {transport_retries}/{MAX_STREAM_RETRIES}): {e}");
                tokio::time::sleep(crate::ai::stream_retry_backoff(transport_retries)).await;
                continue 'attempt;
            }
            if stall_retries < MAX_FIRST_TOKEN_RETRIES {
                stall_retries += 1;
                log::warn!("AI stream failed before first token (attempt {stall_retries}/{MAX_FIRST_TOKEN_RETRIES}): {e}");
                continue 'attempt;
            }
        }
        crate::ai::record_stream_failure();
        return Err(e);
    }
}
```

Note both retry classes are gated on `!first_token_seen` — this deliberately
differs from the rexyMCP original (which buffers output and can therefore
retry transport drops mid-stream). Do not "improve" this by retrying
mid-stream.

### Task 2 — Set `first_token_seen` from real deltas

Port `delta_carries_token` verbatim into `src/ai/backends/openai.rs` (private
fn) and call it where the delta object is already in hand (~line 218):

```rust
/// Whether a delta carries a real token (non-empty content, reasoning, or tool calls).
fn delta_carries_token(delta: &serde_json::Map<String, Value>) -> bool {
    let has_content = delta
        .get("content")
        .and_then(|c| c.as_str())
        .is_some_and(|c| !c.is_empty());
    let has_reasoning = delta
        .get("reasoning")
        .or_else(|| delta.get("reasoning_content"))
        .and_then(|r| r.as_str())
        .is_some_and(|r| !r.is_empty());
    let has_tool_calls = delta
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|t| !t.is_empty());
    has_content || has_reasoning || has_tool_calls
}
```

In the delta branch: `if !first_token_seen && delta_carries_token(delta) {
first_token_seen = true; }` — **before** the `AiEvent::Token` send. Empty
keepalive deltas must not flip the flag (that is the point of the helper).

### Task 3 — Unit tests

In the existing `mod tests` of `src/ai/backends/openai.rs`:

- `delta_carries_token_ignores_empty_keepalive` — asserts `false` for
  `{"content": ""}` and for `{}`; asserts `true` for `{"content": "x"}`,
  `{"reasoning_content": "r"}`, and `{"tool_calls": [{}]}`.
- `delta_carries_token_ignores_role_only_delta` — asserts `false` for
  `{"role": "assistant"}` (the first delta OpenAI sends).

(The attempt loop itself is exercised end-to-end by the phase-01 helper tests
plus the existing backend tests; do not build an HTTP mock server for this
phase.)

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-02.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "stream_chunk" src/ai/backends/openai.rs` prints `0`
      (currently `1`).
- [ ] `grep -c "stream_next_with_timeout" src/ai/backends/openai.rs` prints
      `1` (currently `0`).
- [ ] `grep -c "fn delta_carries_token" src/ai/backends/openai.rs` prints `1`
      (currently `0`).
- [ ] `cargo test delta_carries_token` passes (both tests).
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tests enumerated in Spec Task 3. Existing openai tests
(`apply_tool_call_delta`, `parse_openai_usage`) must keep passing unchanged.

## End-to-end verification

No daemon-visible behavior change is checkable hermetically (the shared
client still carries `.read_timeout` until phase-03; live long-stream checks
happen at milestone close). Evidence is the conversion greps plus gates:

```sh
A=/tmp/e2e-02.txt; : > "$A"
grep -c "stream_chunk" src/ai/backends/openai.rs >> "$A"
grep -c "stream_next_with_timeout" src/ai/backends/openai.rs >> "$A"
grep -c "fn delta_carries_token" src/ai/backends/openai.rs >> "$A"
cargo test delta_carries_token 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (run after pasting the entry, append the verdict
line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-02-openai-two-phase.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02.txt
diff /tmp/pasted-02.txt /tmp/e2e-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- `src/ai/backends/anthropic.rs`, `src/ai/backends/gemini.rs` (phase-03).
- `http()` / `stream_chunk()` in `src/ai/mod.rs` — both stay untouched;
  `stream_chunk` still has callers in the other two backends. Known
  transitional behavior (accepted): until phase-03 lands, the client-level
  300 s total timeout can still kill a very long OpenAI generation, and it
  now surfaces as a retriable transport error pre-first-token. Do not try to
  fix this here.
- Any retry after `first_token_seen` — explicitly forbidden (duplicated
  output).
- Changing `AiEvent` variants or the daemon/CLI.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-17 15:19 (progress)

Started phase-02 (OpenAI two-phase stream timeouts + bounded retry). Flipped status todo → in-progress. Implementing the attempt-loop restructure of `OpenAiClient::chat` per the phase-01 helper surface (`stream_next_with_timeout`, `select_timeout`, `stream_retry_backoff`, `is_retriable_transport`, `record_stream_*`), the `delta_carries_token` port, and the two unit tests. Working tree was clean at start; phase-01 helpers confirmed present (`stream_next_with_timeout` count = 1).

### Update — 2026-08-17 15:26 (end-to-end verification)

Verbatim output of the § End-to-end verification block (`/tmp/e2e-02.txt`):

```text
0
1
3

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s

exit=0

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0
```

PASTE MATCH
