# Phase 03 — circuit-breaker

**Milestone:** M41 — Correctness & reliability
**Status:** todo
**Drafted:** 2026-11-27
**Depends on:** 02 (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Close audit row **C7**. The AI circuit breaker (`CircuitBreaker`,
`send_with_retry` and `record_stream_success` / `record_stream_failure`
in `src/ai/mod.rs`, called from `drive_sse_stream` in
`src/ai/backends/mod.rs`) has four defects. The architect reproduced all
four on 2026-11-27 with a scratch test, since reverted:

1. **Success is recorded on HTTP 200 headers.** `send_with_retry` calls
   `record_success()` as soon as the headers arrive. A stream that then
   fails mid-response records one failure, and the next request's 200
   resets the count. So an endpoint that always fails mid-stream never
   trips the breaker.
   - **Probed:** ten rounds of header-OK followed by stream-fail, in
     `drive_sse_stream`'s order, left the breaker `closed`.
2. **A 4xx trips the breaker.** `send_with_retry_inner` bails on any
   non-success status, and the failure is counted. A 400 (for example a
   context that is too long), a 401 or a 404 is a request or config
   error, not an outage, but five of them open the breaker.
   - **Probed:** five HTTP 400s from a local server opened the breaker.
3. **The breaker is global.** A single static breaker serves every
   provider and endpoint, so one dead endpoint refuses all the others.
   - **Probed:** after the 400s, a request to a *different* local
     endpoint that returns 200 was refused with "AI backend circuit
     breaker is open".
4. **Half-open has no probe gate.** `allow()` returns `true` for every
   caller once the cooldown passes, so a recovering endpoint gets the
   whole backlog at once.
   - **Probed:** in half-open, `allow()` returned `true`, `true`, `true`.

## Required behaviour

1. **One outcome per call.** One call of `drive_sse_stream` records at
   most one outcome against its breaker. Its internal retries do not each
   count.
   - **Success:** the stream reached its natural end. A successful header
     exchange alone records nothing.
   - **Failure:**
     - a transport error;
     - a 5xx;
     - a 408 or 429 that is still failing after `send_with_retry`'s own
       retries;
     - a stream that fails after its headers, which includes stalls and
       idle timeouts.
   - **Neutral:** any other 4xx (400, 401, 403, 404, 413, 422, …). It
     leaves the failure count unchanged.
2. **Per endpoint.** Breakers are keyed by the request URL's **origin**
   (scheme, host and port).
   - Anthropic and Gemini each get one breaker.
   - Two OpenAI-compatible `base_url`s get two breakers.
   - One endpoint's open breaker never refuses a request to another.
3. **Half-open admits exactly one probe.** After the cooldown, the first
   call through takes the probe; every other call to that endpoint is
   refused until the probe ends.
   - **Probe succeeds:** the breaker closes and the count resets to 0.
   - **Probe fails:** the breaker re-opens for a full cooldown.
   - **Probe ends neutral (a 4xx):** the breaker closes, because the
     endpoint answered.
   - **Probe is cancelled** (its future dropped, as when a turn is
     cancelled or its task aborted): it records nothing and frees the
     probe slot, so the next call may probe.
4. **Unchanged:**
   - the threshold, `CB_FAILURE_THRESHOLD` = 5;
   - the cooldown, `CB_COOLDOWN` = 60 s;
   - `send_with_retry`'s retry and `Retry-After` behaviour;
   - the "circuit breaker is open" refusal, an `Err` before any request
     is sent. Its wording may name the endpoint's origin.

## Statement of work

- Rework the breaker in `src/ai/mod.rs`. You may move it into a new
  child module, `src/ai/circuit.rs`.
- Rework its accounting in `drive_sse_stream`.
- The mechanism is yours. A permit that `drive_sse_stream` acquires once,
  holds across its retries and resolves once, released on drop, fits
  behaviour 3's cancel rule.
- The origin may come from a built request; `build_request` already
  builds one per attempt.

**Closed file list:**
- `src/ai/mod.rs`;
- `src/ai/backends/mod.rs`;
- optionally the new `src/ai/circuit.rs`;
- this doc.

Do not touch the three backend files or phase 04's error classification.
In `drive_sse_stream`, change only the breaker accounting.

**Gotchas:**
- **Never hold a `std::sync::Mutex` guard across an `.await`.** A permit
  carries its key, not a guard.
- Lock sites use `.unwrap_or_log()`. `tests/lock_invariant.rs` now also
  catches `.read()`, `.write()` and the `try_*` forms.
- **Log and refuse with the origin only, never the full URL.** The Gemini
  URL once carried the API key (M36 S4).
- The breaker is process-global. Tests that go through
  `send_with_retry` / `drive_sse_stream` stay isolated only because each
  local server has its own port, and therefore its own origin. Never
  assert on a shared key.
- The local-server pattern to copy is `stream_idle_tests` in
  `src/ai/mod.rs`. Read the whole request before answering.

**Edit order:** each step ends with `cargo test --lib --no-run`.
1. **Your first tool call is an edit adding the red test**
   `half_open_admits_exactly_one_probe`. It is written against today's
   `CircuitBreaker`:
   - trip the breaker;
   - expire the cooldown, as `circuit_breaker_closes_on_success` does;
   - assert that a second caller is refused.

   It is red today, because `allow()` returns `true` again.
2. The per-origin breaker state and the probe gate.
3. The accounting in `send_with_retry` / `drive_sse_stream`.
4. The remaining tests.

**Read limit:** read at most `src/ai/mod.rs` and `src/ai/backends/mod.rs`
before your first edit.

**Budget:** you have **1200 turns**; use them efficiently.

## Tests

Put the tests next to the breaker. The three existing tests keep their
names, though their bodies may adapt:
- `circuit_breaker_closed_initially`
- `circuit_breaker_opens_after_threshold`
- `circuit_breaker_closes_on_success`

New tests:

- `half_open_admits_exactly_one_probe`
  - The first caller after the cooldown is admitted and the second is
    refused.
  - Probe success closes the breaker, and probe failure re-opens it.
  - **Today:** red.
- `a_cancelled_probe_frees_the_slot`
  - A probe that is dropped without an outcome records nothing.
  - The next caller is then admitted as the probe.
- `headers_alone_do_not_record_success`
  - Record threshold − 1 failures on a local `200 OK` server's origin.
  - Then make one header exchange to that server through the real
    request path.
  - The count must still be threshold − 1, and one more failure must
    open the breaker.
  - **Today:** red. The 200 resets the count.
- `a_client_error_does_not_trip_the_breaker`
  - At least `CB_FAILURE_THRESHOLD` calls through the real request path
    to a local server answering `400 Bad Request`.
  - The breaker for that origin stays closed.
  - **Today:** red.
- `breakers_are_per_endpoint`
  - Trip endpoint A's breaker.
  - A request to endpoint B, a second local server answering `200 OK`,
    is still sent and succeeds.
  - **Today:** red.

## Acceptance criteria

- [ ] All eight tests above pass. Paste the red runs of
      `half_open_admits_exactly_one_probe`,
      `headers_alone_do_not_record_success`,
      `a_client_error_does_not_trip_the_breaker` and
      `breakers_are_per_endpoint`, **as output**.
- [ ] **Mutation runs, pasted as output.** Do each one separately, and
      undo each by hand, never from a backup copy:
      1. Let a second caller through in half-open. Run
         `cargo test --lib half_open_admits_exactly_one_probe` once. It
         must fail.
      2. Record success on a successful header exchange again. Run
         `cargo test --lib headers_alone_do_not_record_success` once. It
         must fail.
- [ ] **No full URL in a breaker log or refusal:** the breaker code
      formats only the origin. Name the expression you use in the
      Update Log.
- [ ] **No test removed (preservation).** Run this once; it prints
      nothing:
      `LC_ALL=C comm -23 <(git show 57691b5a:src/ai/mod.rs | grep -A2 '#\[\(tokio::\)\?test' | grep -oE 'fn [a-z_0-9]+' | sort -u) <(cat src/ai/mod.rs src/ai/circuit.rs 2>/dev/null | grep -A2 '#\[\(tokio::\)\?test' | grep -oE 'fn [a-z_0-9]+' | sort -u)`
- [ ] **Counts:** the lib suite reports **3211 + N passed**, where N is
      the number of new tests you added (at least 5). Name N.
- [ ] **Gates:** all four pass, each run once.
      - `ai::filter::reinit_tests::masking_reinit_applies_a_new_extra_pattern`
        is a known flake. If it alone fails, re-run `cargo test` once and
        paste both results.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when `cargo test --lib --no-run` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line;
   - the four red runs and the two mutation runs, **as output, not
     summarized**;
   - the comm output.

   Then commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.

## Out of scope

- Classifying stream errors as transport, stall or provider errors, and
  Gemini's malformed-call report: phase 04.
- Showing the breaker state in `daemoneye status`.
- Changing the threshold or the cooldown.
- The backend files `anthropic.rs`, `openai.rs` and `gemini.rs`.

## Update Log

<!-- entries appended below this line -->
