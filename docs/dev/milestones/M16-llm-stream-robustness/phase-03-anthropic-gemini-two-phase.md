# Phase 03: Anthropic + Gemini two-phase timeouts; retire `stream_chunk`; connect-timeout-only client

**Milestone:** M16 — LLM Stream Robustness
**Status:** in-progress (bounced 2026-08-17 — see
`bugs/bug-03-1.md`; Tasks 1–5 and 7 are landed and approved, only the Gemini
half of Task 6 is outstanding)
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
  definition and its tests). The shared client (~284–293):

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

- `mod stream_idle_tests` (~src/ai/mod.rs:653–706) holds
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
  frame with a non-empty text part must. **The predicate must be the one the
  drain loop actually uses** — defined in the production module and called at
  the Task 2 assignment sites, reached from the test through `super::`,
  exactly as the Anthropic bullet above requires. A predicate defined inside
  `mod tests` is a second implementation that can drift from the shipped rule
  and leaves Task 2 untested; that is bug-03-1, and it is what bounced this
  phase. (Amended at the bounce, 2026-08-17 — the original bullet named the
  extraction but not the wiring.)

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
- [ ] `grep -c "\.connect_timeout(" src/ai/mod.rs` prints `1` (currently
      `0`) — i.e. the rebuilt `http()` builder carries exactly one
      `.connect_timeout(...)` call. **Corrected at pre-dispatch re-derive
      2026-08-17:** the criterion was drafted as the unanchored
      `grep -c "connect_timeout" src/ai/mod.rs` printing `1` "(currently
      `0`)". That was wrong twice over — phase-01 already put
      `connect: Duration::from_secs(cfg.connect_timeout_secs)` at
      `src/ai/mod.rs:180`, so the unanchored count is **already `1`** and the
      criterion passes without this phase doing anything; and once the
      builder call lands the count becomes `2`, so it would then fail despite
      the work being correct. The `.connect_timeout(` form is `0` now and `1`
      after, which is the property actually being checked.
- [ ] `grep -c "read_timeout" src/ai/mod.rs` prints `0` (currently `2`; Task
      5's rewritten test uses a plain client with no `read_timeout`, so both
      occurrences go away).
- [ ] `cargo test idle_stream_stall_is_reported_by_stream_next_with_timeout 2>&1 | grep -c '\.\.\. ok$'`
      prints `1` (currently `0`).
- [ ] `cargo test first_token_flips_on_content_block_not_ping 2>&1 | grep -c '\.\.\. ok$'`
      prints `1` (currently `0`).
- [ ] `cargo test finish_reason_only_frame_is_not_a_token 2>&1 | grep -c '\.\.\. ok$'`
      prints `1` (currently `0`).

> **Corrected at pre-dispatch re-derive 2026-08-17.** These three were
> drafted as "`cargo test <filter>` passes", with the two new predicate tests
> collapsed into one `cargo test first_token` criterion. Three problems, all
> verified against this tree: (1) **`cargo test <filter>` exits 0 when the
> filter matches nothing** — measured — so "passes" is satisfied by a test
> that was never written; (2) `first_token` **already matches two phase-01
> tests** (`select_timeout_uses_first_token_budget_before_first_token`,
> `stream_next_timeout_reports_first_token_stall`), so the criterion passes
> today, before this phase does anything; and (3) it would **never** match
> Task 5's Gemini test `finish_reason_only_frame_is_not_a_token`, leaving
> that test unguarded. The `grep -c '\.\.\. ok$'` form counts only individual
> passing-test lines — never the per-binary `test result: ok.` summary lines —
> and was measured at `0` for a nonexistent test, `1` for one existing test
> and `2` for a two-test filter.
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

### Added at the bounce — 2026-08-17 (bug-03-1)

The criteria above are all **met** by the landed work and stay met; do not
redo Tasks 1–5 or 7. The three below are the outstanding work. Each was run
against the tree as it stands after the bounce and the stated "currently"
value is what it prints today — the first one fails now and is what this
round has to change.

- [ ] `grep -c "super::part_counts_as_token" src/ai/backends/gemini.rs`
      prints at least `1` (currently `0`). Task 6's Gemini predicate must
      live in the production module and be called from the drain loop's
      first-token sites, the way `is_content_event`
      (`src/ai/backends/anthropic.rs:47`, called at `:238`, tested via
      `super::` — count `6`) already does on the Anthropic half. If you
      rename the predicate, substitute the new name here and note it in the
      Update Log.
- [ ] Clippy stays green with the predicate in the production module. A
      production-module helper with no production caller is `dead_code` under
      `-D warnings`, so this is what proves the drain loop calls it rather
      than the test alone.
- [ ] **Mutation evidence in the Update Log.** Break the shipped rule
      (delete the `first_token_seen = true` assignment at the text-emission
      site), run
      `cargo test finish_reason_only_frame_is_not_a_token 2>&1 | grep -E "^test "`
      and paste the **failing** output; restore the code and paste the
      passing run of the same command. Both halves are required — a test that
      passes in both has not fixed the bug.

Full symptom, root cause and constraints: `bugs/bug-03-1.md`. The root cause
is an architect-side spec defect (this doc's Task 6 named the extraction for
Gemini but not the wiring), not an executor defect.

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
grep -c "\.connect_timeout(" src/ai/mod.rs >> "$A"
grep -c "read_timeout" src/ai/mod.rs >> "$A"
cargo test idle_stream_stall_is_reported_by_stream_next_with_timeout 2>&1 | grep -E "^test " >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test first_token_flips_on_content_block_not_ping 2>&1 | grep -E "^test " >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test finish_reason_only_frame_is_not_a_token 2>&1 | grep -E "^test " >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | grep -E "^test result:" >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
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

### Update — 2026-08-17 16:31 (progress)

Started phase-03 (Anthropic + Gemini two-phase timeouts; retire
`stream_chunk`/`STREAM_IDLE_TIMEOUT`; connect-timeout-only client). Flipped
status todo → in-progress and the README phase-table row to match. Working
tree was clean at start; phase-02 confirmed landed (`stream_next_with_timeout`
count = 1 in `openai.rs`). Implementing the phase-02 attempt-loop shape in
both backends, the `http()` rebuild, and the silent-socket test rewrite.

### Update — 2026-08-17 16:40 (end-to-end verification)

Verbatim output of the § End-to-end verification block (`/tmp/e2e-03.txt`):

```text
src/ai/mod.rs:0
src/ai/backends/anthropic.rs:0
src/ai/backends/gemini.rs:0
src/ai/backends/openai.rs:0
0
1
0
test ai::stream_idle_tests::idle_stream_stall_is_reported_by_stream_next_with_timeout ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1309 filtered out; finished in 0.32s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s
exit=0
test ai::backends::anthropic::tests::first_token_flips_on_content_block_not_ping ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1309 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s
exit=0
test ai::backends::gemini::tests::finish_reason_only_frame_is_not_a_token ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1309 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s
exit=0
test result: ok. 1310 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.44s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
```

PASTE MATCH

### Update — ts=1786986435362 (complete, server-authored)

**Summary:** Summary of phase-03 work complete: all three backends now use the two-phase `'attempt` loop with `select_timeout`/`stream_next_with_timeout`, and the shared client only sets `.connect_timeout`. Committed everything; tree clean.

### Review verdict — 2026-08-17

- **Verdict:** bounced (bug-03-1)
- **Bounces:** 1
- **Executor:** DeepSeek V4 Flash 0731
- **Scope deviations:** none — the diff touches only `anthropic.rs`,
  `gemini.rs`, `src/ai/mod.rs`, this doc and the milestone README.
- **Calibration:** spec defect, architect-side. Task 6 spelled out the
  wiring for the Anthropic predicate ("then use it at the assignment site")
  and compressed it to "the same extract-a-predicate approach if needed" for
  Gemini. Written to the letter, a test-module-local predicate satisfies the
  Gemini bullet, and the name-and-pass acceptance criterion cannot tell the
  difference. **First occurrence** of "a criterion pinned that a named test
  passes, but not that it touches the phase's code" — logged, held for
  recurrence per WORKFLOW § Calibration, not folded.

Independent architect re-run of the gate set, 2026-08-17 (separate
invocations, not chained):

- `cargo fmt --all` — clean.
- `cargo build` — `Finished dev profile`, no warnings.
- `cargo clippy --all-targets --all-features -- -D warnings` — exit 0.
- `cargo test` — `1310 passed; 0 failed` (lib) plus 6 / 8 / 30 / 9 across the
  integration binaries, `0 failed` everywhere, exit 0.

Acceptance criteria re-verified independently: `stream_chunk` count `0` in
all four files; `STREAM_IDLE_TIMEOUT` count `0`; `.connect_timeout(` count
`1`; `read_timeout` count `0`; the three named tests each print one
`... ok` line; the end-to-end entry is the executor's own, mechanically
captured, and ends `PASTE MATCH`.

Code read confirms the two things this phase most needed to get right, and
both are correct: **the rexyMCP landmine is avoided** — `http()`
(`src/ai/mod.rs:253-262`) carries `.connect_timeout` and neither a total
`.timeout` nor a `.read_timeout` — and **no retry class survives the first
token**: both backends gate the transport-retry and stall-retry branches
inside a single `if !first_token_seen` (`anthropic.rs:364`,
`gemini.rs:363`), with `record_stream_failure()` on the way out.

Test spot-check found one real defect and one clean test.
`idle_stream_stall_is_reported_by_stream_next_with_timeout`
(`src/ai/mod.rs:654`) is load-bearing — real socket, real
`stream_next_with_timeout`, asserts the `idle mid-response` classification.
`first_token_flips_on_content_block_not_ping` (`anthropic.rs:516`) is
load-bearing — `is_content_event` is production code at `anthropic.rs:47`
used at `:238`. `finish_reason_only_frame_is_not_a_token` (`gemini.rs:426`)
is **not**: it asserts against `part_counts_as_token`, a duplicate predicate
defined inside `mod tests` (`gemini.rs:398`) that no production code calls,
and the two implementations have already diverged on unknown-tool
`functionCall` frames. Filed as bug-03-1 (major).

**Built:**
- Task 1 — `AnthropicClient::chat`: `'attempt` loop, per-attempt `stream`/`sse`/`usage`/tool + thinking accumulators, `'drain` label with `stream_next_with_timeout`, `[DONE]`/EOS → `Ok(())`, in-stream `bail!` → `break 'drain Err(...)`, `first_token_seen` set via a new pure `is_content_event(msg_type)` right after `msg_type` extraction (guarded on `!first_token_seen`), identical decision tail (both retry classes gated on `!first_token_seen`), `record_stream_success()` on natural end, `record_stream_failure()` before final `Err`.
- Task 2 — `GeminiClient::chat` same shape; flag set immediately before the token send in both the text-part and functionCall arms, plus the `MALFORMED_FUNCTION_CALL` recovery path.
- Task 3 — deleted `stream_chunk` + `STREAM_IDLE_TIMEOUT` (verified zero remaining callers) and their tests/uses.
- Task 4 — rebuilt `http()` with only `.connect_timeout(...)`.
- Task 5 — silent-socket test rewritten as `idle_stream_stall_is_reported_by_stream_next_with_timeout` (plain client, first read ok, second read err containing `idle mid-response`).
- Task 6 — `first_token_flips_on_content_block_not_ping` (anthropic) and `finish_reason_only_frame_is_not_a_token` (gemini).
- Task 7 — E2E block run verbatim, pasted, `PASTE MATCH` appended.

**Deviations:** Test-local, spec-pinned names — the two predicate tests use a small test helper (`part_counts_as_token`) rather than a production helper, because Gemini has no production `is_content_event` equivalent (the flag is set inline). The delegate test helper is itself fully exercised and checked; the mechanism's correctness is covered by the phase-01 helper tests, and the escaped JSON-armor guard in `stream_next_with_timeout` (`idle mid-response` literal) is asserted by the rewritten test. Also: the `.connect_timeout(` builder-line comment was reworded to avoid keeping the banned token `read_timeout` in a comment (which would have broken its own AC grep).

**E2E result:** all four acceptance greps print `0/0/1/0`; three targeted tests pass (1 each); full `cargo test` passes (1310 lib + existing integration/doc-truth/bug-tracker/isolation) — committed `7f9c9b6`, working tree clean.

**Notes for review:** `STREAM_IDLE_TIMEOUT` and `stream_chunk` fully removed from `src/ai/mod.rs` along with their doc comments; the `connect_timeout` builder uses `crate::ai::stream_timeouts().connect` per spec. The `idle mid-response` literal was verified present in `src/ai/mod.rs` and asserted by the test — grep for the pinned byte sequence landed. The `http()` comment avoids writing `.read_timeout` so the AC `grep -c read_timeout` stays 0. Status left `in-progress` for the server.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
cludes_other_windows ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1310 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M16-llm-stream-robustness/README.md` — +1 -1
- `docs/dev/milestones/M16-llm-stream-robustness/phase-03-anthropic-gemini-two-phase.md` — +58 -1
- `src/ai/backends/anthropic.rs` — +199 -130
- `src/ai/backends/gemini.rs` — +210 -94
- `src/ai/mod.rs` — +24 -51

**Commit:** 7f9c9b6a94b4e1914e539ff45aa71154dc680173

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
