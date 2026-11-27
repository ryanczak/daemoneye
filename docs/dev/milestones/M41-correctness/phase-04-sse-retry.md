# Phase 04 — sse-retry

**Milestone:** M41 — Correctness & reliability
**Status:** todo
**Drafted:** 2026-11-27
**Depends on:** 03 (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Close audit rows **C12** and **C24**. The architect reproduced each defect
on 2026-11-27 with a scratch test, since reverted. Each scratch test drove
`drive_sse_stream` (`src/ai/backends/mod.rs`) against a local server that
counted requests:

1. **A transport error falls through into the stall retries.** Before the
   first token, a `reqwest` transport error gets up to 3 transport retries.
   When those run out, the same error falls into the stall branch and gets
   2 more retries, which have no backoff.
   - **Probed:** a server that sends `200` headers and a partial chunk,
     then drops the socket, received **6** requests (1 + 3 + 2).
2. **A provider error frame is retried.** An in-stream error frame such as
   `{"error":{"message":"quota exceeded"}}`, which a retry cannot fix, is
   handled as a stall and retried twice.
   - **Probed:** **3** requests, then `Err("AI stream returned an error:
     quota exceeded")`.
3. **A Gemini malformed function call is reported three times.** An
   unrecoverable `MALFORMED_FUNCTION_CALL` sends an `AiEvent::Error`, then
   bails. The bail is retried like a stall, so the error is sent three
   times.
   - **Probed:** **3** requests and **3** `AiEvent::Error` events.
4. **C24: `"error": null` aborts the stream.** The OpenAI and Gemini
   handlers check `v.get("error")`, which is `Some(Null)` for a proxy that
   always sends the key.
   - **Probed:** a frame `{"error":null,"choices":[{"delta":{"content":"hi"}}]}`
     failed through `OpenAiClient::chat`. A Gemini frame
     `{"error":null,"candidates":[…"text":"hi"…]}` failed through
     `drive_sse_stream`. Each made 3 requests.

## Required behaviour

Before the first token, an error from a drained stream falls into exactly
one class, and each class has its own retry budget. Classify by the
error's type or a marker attached where it is raised, **never by matching
message text**.

| Class | What it is | Retries before the first token |
|---|---|---|
| transport | a `reqwest` error, for example a dropped connection or a body decode failure (`is_retriable_transport` today) | up to 3, with today's `stream_retry_backoff` |
| stall | the synthetic first-token timeout from `stream_next_with_timeout` | up to 2, as today |
| fatal | anything else: a provider error frame, an unrecoverable malformed call, the SSE buffer overflow, any other handler error | none |

1. **No fall-through.** A transport error that has used its 3 retries
   fails the call. So the call makes at most 4 requests. It never moves on
   to the stall budget.
2. **A fatal error fails the call after one request.**
3. **The Anthropic exception.** An Anthropic in-stream `error` frame whose
   `error.type` is `overloaded_error` or `api_error` is transient. It is
   classed as stall, which keeps today's 2 retries. Any other Anthropic
   error type is fatal, for example `invalid_request_error`.
4. **A Gemini malformed call is reported once.** An unrecoverable
   `MALFORMED_FUNCTION_CALL` sends no `AiEvent::Error` itself. It fails
   the call once, with an `Err` whose text contains
   `malformed function call` and the raw `finishMessage`. The caller
   (`conversation_helpers.rs`, `ghost/turn_helpers.rs`) already turns that
   `Err` into the one `AiEvent::Error`. A *recoverable* malformed call is
   unchanged: it still becomes a `ToolCall`.
5. **C24: `"error": null` is not an error.** In the OpenAI and Gemini
   handlers, a frame whose `error` is JSON `null` is processed as if the
   key were absent. A non-null `error` still fails the call; the existing
   tests at `openai.rs` / `gemini.rs` that send `{"error":{"message":…}}`
   keep passing.
6. **Unchanged:**
   - there are no retries after the first token;
   - stall retries keep their budget of 2;
   - the phase 03 breaker accounting: one permit per call, retries under
     it through `send_under_permit`, any final drain error recorded as a
     failure;
   - `send_with_retry`'s HTTP-status retries.

## Statement of work

- Classify drain errors in `src/ai/backends/mod.rs`, and use the class
  in `drive_sse_stream`'s retry branch.
- Raise fatal errors with their class in the handlers:
  - `anthropic.rs` decides overloaded/api vs other;
  - `gemini.rs` handles the malformed call;
  - `openai.rs` and `gemini.rs` handle `"error": null`.
- The mechanism is yours. One fitting shape is a small error type or
  marker in `backends/mod.rs` that a handler wraps its error in and the
  retry branch downcasts. `anyhow::Error::downcast_ref` already
  distinguishes `reqwest::Error`.

**Closed file list:**
- `src/ai/backends/mod.rs`;
- `src/ai/backends/openai.rs`;
- `src/ai/backends/gemini.rs`;
- `src/ai/backends/anthropic.rs`;
- `src/ai/mod.rs`, only if `is_retriable_transport` or the test helpers
  need to move;
- this doc.

**Gotchas:**
- **Tests drive the real path.** Each request-path test runs
  `drive_sse_stream` with the provider's real frame handler, or the public
  `OpenAiClient::chat`, against a local `TcpListener` server that counts
  requests.
  - The handlers are private to their files, so put each provider's tests
    in that file's test module.
  - `OpenAiClient::new(key, model, "http://<addr>/v1", 16)` reaches a
    local server.
  - Anthropic and Gemini hard-code their URLs, so call `drive_sse_stream`
    directly with a local URL.
  - Copy the server pattern from `circuit_request_path_tests::local_server`
    in `src/ai/mod.rs`. The server must read the whole request first,
    including the body by `Content-Length`; otherwise the client can see
    a reset.
  - A shared `#[cfg(test)]` helper in `backends/mod.rs` that serves a raw
    response string is fine.
- **Breakers are process-global and keyed by origin.** Every test server
  binds its own port, so each test has its own breaker. Never trip a
  breaker shared with another test.
- **A transport-error fixture:** answer
  `HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n`
  and drop the socket. The architect's probe got a `reqwest` body error
  from this, and `is_retriable_transport` is true for it.
- **An SSE fixture:** answer `200` with
  `Content-Type: text/event-stream`, an exact `Content-Length` and
  `Connection: close`. The body is `data: <json>\n\n` frames.
- **Do not match error text** to classify. Matching text in a test's
  assertion is fine.
- There is no seam to shorten the 600 s first-token timeout, so the stall
  class is tested at the classifier, not through a server.

**Edit order:** each step ends with `cargo test --lib --no-run`.
1. **Your first tool call is an edit adding the red test**
   `a_provider_error_frame_is_not_retried` to `openai.rs`'s test module.
   Run it once; it is red today, because the server sees 3 requests.
2. The classification in `backends/mod.rs` and the retry branch.
3. The handler changes: Anthropic, Gemini malformed, and `"error": null`.
4. The remaining tests, each run red before its fix where the table
   below says "red today".

**Read limit:** before your first edit, read at most
`src/ai/backends/mod.rs`, `src/ai/backends/openai.rs` and the
`circuit_request_path_tests` module of `src/ai/mod.rs`.

**Budget:** you have **1200 turns**; use them efficiently.

## Tests

| Test (file) | Asserts | Today |
|---|---|---|
| `stream_errors_are_classified_apart` (`backends/mod.rs`) | a scrubbed `reqwest` error → transport; the first-token stall error from `stream_next_with_timeout` → stall; a provider error frame from a handler → fatal; the SSE buffer overflow from `SseBuffer::push` → fatal | does not compile |
| `a_transport_error_is_retried_three_times_then_fails` (`openai.rs`) | the transport fixture gets exactly **4** requests, and the call returns `Err` | red: 6 |
| `a_provider_error_frame_is_not_retried` (`openai.rs`) | an error frame gets exactly **1** request; the `Err` text contains its message | red: 3 |
| `anthropic_overloaded_frame_is_retried` (`anthropic.rs`) | a `{"type":"error","error":{"type":"overloaded_error",…}}` frame gets exactly **3** requests | preservation: 3 today |
| `anthropic_invalid_request_frame_is_not_retried` (`anthropic.rs`) | an `invalid_request_error` frame gets exactly **1** request | red: 3 |
| `gemini_malformed_call_is_reported_once` (`gemini.rs`) | an unrecoverable `MALFORMED_FUNCTION_CALL` (`finishMessage` `"garbage"`) gets **1** request and **0** `AiEvent::Error` on `tx`; the `Err` contains `malformed function call` and `garbage` | red: 3 and 3 |
| `openai_error_null_is_not_an_error` (`openai.rs`) | the null-error frame, then `[DONE]`: the call returns `Ok`, `tx` carries `Token("hi")` and a `Done` | red |
| `gemini_error_null_is_not_an_error` (`gemini.rs`) | the Gemini null-error frame: `Ok`, `Token("hi")`, `Done` | red |

## Acceptance criteria

- [ ] All eight tests above pass. Each one marked red was run red once
      against the tree before its fix, and the run is pasted **as
      output**, copied from that `cargo test` tool result. The reviewer
      greps the session log's tool results for every pasted failure. A
      failure that appears only in the Update Log write is treated as
      fabricated (bug-03-1).
- [ ] **Two mutation runs, pasted as output from tool results.** Apply
      each mutation with an edit, undo it with an edit (never a `cp` from
      a backup), and run `git diff --stat` after each undo.
      1. Class provider error frames as stall again. Run
         `cargo test --lib a_provider_error_frame_is_not_retried` once.
         It must fail.
      2. Let an exhausted transport error fall through to the stall
         budget again. Run
         `cargo test --lib a_transport_error_is_retried_three_times_then_fails`
         once. It must fail.
- [ ] **No text-matching classifier.** This prints **0**, which it
      also prints today. It counts the production part of the file, above
      the first `#[cfg(test)]`:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/ai/backends/mod.rs | grep -cE '\.contains\("|starts_with\("'`
- [ ] **No test removed:** this prints nothing:
      `for f in src/ai/backends/mod.rs src/ai/backends/openai.rs src/ai/backends/gemini.rs src/ai/backends/anthropic.rs src/ai/mod.rs; do LC_ALL=C comm -23 <(git show c7917573:$f | grep -A2 '#\[\(tokio::\)\?test' | grep -oE 'fn [a-z_0-9]+' | LC_ALL=C sort -u) <(grep -A2 '#\[\(tokio::\)\?test' $f | grep -oE 'fn [a-z_0-9]+' | LC_ALL=C sort -u); done`
- [ ] **Counts:** the lib suite reports exactly **3227 passed**
      (3219 + 8).
- [ ] **Closed file list:** `git diff --stat c7917573` names only files
      from the closed list.
- [ ] All four gates pass, each run once, separately.

## Spec

1. **Break down** — write the first red test and run it red. Then write
   a numbered task list into an Update Log entry, one line per task,
   each naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement** — follow the edit order, writing each red test before
   its fix. Done when `cargo test --lib --no-run` is green.
3. **Meet the acceptance criteria** — check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence** — run the four gates, once each. Paste into
   a new Update Log entry:
   - each `test result:` line;
   - the red runs and the two mutation runs, **as output copied from tool
     results, not summarized**;
   - the comm output.

   Done when the entry is written and the tree is committed.

## Authorizations

- Edits to the four backend files and to `src/ai/mod.rs`'s retry helpers
  and test helpers.
- A new `#[cfg(test)]` helper in `src/ai/backends/mod.rs`.

## Out of scope

- **Breaker accounting.** For example, whether a fatal malformed call
  should count as an endpoint failure. Phase 03's rules stand.
- **Retrying OpenAI or Gemini error frames by code.** Gemini's
  `error.code` 500/503 is an example. This phase treats them as fatal,
  deliberately; a later phase can add a transient list if needed.
- A seam for the first-token timeout.
- Retries after the first token.

## Update Log

<!-- entries appended below this line -->
