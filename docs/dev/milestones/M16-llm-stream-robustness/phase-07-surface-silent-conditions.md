# Phase 07: Surface silent conditions — truncation, refusal, unknown tools, malformed frames, empty replies

**Milestone:** M16 — LLM Stream Robustness
**Status:** todo
**Depends on:** phase-03
**Estimated diff:** ~260 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Five conditions currently vanish into `log::warn!` while the user sees a
truncated, empty, or blank reply: response truncation at max_tokens,
provider refusal/content-filter stops, unknown-tool-call drops, malformed
SSE frames, and the all-dropped case where the daemon replies with a bare
`Response::Ok` and the turn looks finished with no output at all. This
phase adds a non-terminal `AiEvent::Notice(String)` that backends emit
alongside the existing logs, forwarded to the client as a visible
`Response::SystemMsg`, plus an empty-reply guard.

## Architecture references

Read before starting:

- `CLAUDE.md` § "Request/Response lifecycle".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(Current as of 2026-08-16; re-derive with
`grep -rn "log::warn" src/ai/backends/ | grep -i "truncat\|refus\|filter\|unknown"`.)

- `AiEvent` is `src/ai/types/events.rs`; its only non-tool variants are
  `Token`, `Error(String)` (terminal), `Done(TokenBreakdown)` (terminal),
  plus one struct variant per tool. Files matching on `AiEvent` (add a
  log-and-ignore arm wherever the compiler demands after Task 1 —
  candidates): `src/daemon/stream.rs`, `src/daemon/ghost.rs`,
  `src/daemon/briefing.rs`, `src/daemon/auto_name.rs`,
  `src/daemon/digest.rs`, `src/daemon/scheduled.rs`,
  `src/daemon/session.rs`, `src/webhook/process.rs`, `src/cost.rs`.
- Log-only sites (all three backends), with their messages:
  - `src/ai/backends/anthropic.rs` ~296–309 — `stop_reason` `"max_tokens"` /
    `"refusal"` (`message_delta` handling).
  - `src/ai/backends/gemini.rs` ~231–242 — `finishReason` `"MAX_TOKENS"` /
    `"SAFETY"`.
  - `src/ai/backends/openai.rs` ~231–247 — `finish_reason` `"length"` /
    `"content_filter"`.
  - Unknown-tool drops: `anthropic.rs` ~290–296 and `openai.rs` ~277–281,
    both `log::warn!("model called unknown tool '{...}' — call dropped")`;
    gemini's equivalent is near its `dispatch_tool_event` call
    (`grep -n "unknown tool" src/ai/backends/gemini.rs`).
  - Malformed SSE frames: all three backends parse each `data:` payload with
    `if let Ok(v) = serde_json::from_str::<Value>(&data)` — a malformed
    frame is silently skipped (`grep -n "if let Ok(v) = serde_json::from_str" src/ai/backends/*.rs`).
- The empty-reply path: `src/daemon/stream.rs` `AiEvent::Done` arm — when
  `pending_calls.is_empty()`, the daemon pushes the assistant message only
  `if !full_response.is_empty()` (~line 675), then sends `UsageUpdate` +
  `Response::Ok` (~843–853). Nothing tells the user the reply was empty.
- `Response::SystemMsg` already renders in the chat client (used for
  auto-name hints and catch-up briefs) — no CLI change needed.

## Spec

### Task 1 — `AiEvent::Notice(String)`

In `src/ai/types/events.rs`, add to `AiEvent`:

```rust
/// Non-terminal advisory the user should see (truncation, refusal,
/// dropped/unknown tool call, malformed provider frames). Forwarded to the
/// chat client as a SystemMsg; log-and-ignore in non-interactive consumers.
Notice(String),
```

Build (`cargo build`) and add the missing match arms the compiler names. In
`src/daemon/stream.rs` the arm is (place it next to `AiEvent::Error`):

```rust
AiEvent::Notice(n) => {
    send_response_split(tx, Response::SystemMsg(format!("⚠ {n}"))).await?;
}
```

In every other consumer the arm is
`AiEvent::Notice(n) => log::warn!("AI notice: {n}"),` (these paths — ghost,
briefing, auto-name, digest, scheduled, session, webhook, cost — have no
interactive client).

### Task 2 — Emit notices from the backends

At each log-only site listed in Current state, keep the `log::warn!` and add
a `tx.send` of a `Notice` with user-facing wording:

- Truncation (all three): `let _ = tx.send(AiEvent::Notice(format!("response truncated: the model hit its max_tokens limit — the answer may be incomplete")));`
- Refusal / safety / content filter (all three): `"the provider stopped this response ({reason})"` with the provider's reason word.
- Unknown tool (all three): `"the model called unknown tool '{name}' — the call was dropped"`.

### Task 3 — Count malformed frames, notice at threshold

In each backend's drain loop, replace the bare
`if let Ok(v) = serde_json::from_str::<Value>(&data)` with a match that on
`Err` increments a per-attempt `malformed_frames: u32`, logs the first
occurrence at `warn` with a truncated payload
(`&data[..data.len().min(120)]`), and continues. After the drain completes
normally (in the same place `record_stream_success()` is called), if
`malformed_frames > 0`, send
`AiEvent::Notice(format!("provider sent {malformed_frames} malformed stream frame(s) — the response may be incomplete"))`.
(No threshold-of-3: any nonzero count is worth one notice at end-of-stream —
a single notice per response, never per frame.)

### Task 4 — Empty-reply guard

In `src/daemon/stream.rs`, in the `Done` arm's `pending_calls.is_empty()`
branch, before the `UsageUpdate`/`Ok` sends: if `full_response.is_empty()`,
send
`Response::SystemMsg("The model returned an empty reply.".to_string())`.
(Any Task 2/3 notice has already been forwarded by this point, so the user
sees cause + effect together, e.g. the dropped-unknown-tool notice followed
by the empty-reply line.)

### Task 5 — Unit tests

- In `src/ai/types/events.rs` or an existing backend test module: not
  needed for the enum itself (plumbing).
- In each backend's tests, one test per backend asserting the notice text
  for its truncation reason-word mapping **if** the mapping is a testable
  helper; if it is inline string formatting at the emit site, skip
  per-backend tests (plumbing) — instead:
- `src/daemon/stream.rs` gains no new testable seam from Task 4 (control
  flow inside `run_conversation_loop`); pin it by grep criterion below.
- Add `malformed_frame_counting` as a small pure-helper test **only if** you
  extract a helper; otherwise the criterion greps suffice. Do not build an
  HTTP mock server for this phase.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-07.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "Notice(String)" src/ai/types/events.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "AiEvent::Notice" src/daemon/stream.rs` prints `1`
      (currently `0`).
- [ ] Each backend emits notices:
      `grep -c "AiEvent::Notice" src/ai/backends/anthropic.rs src/ai/backends/gemini.rs src/ai/backends/openai.rs`
      prints ≥ `3` for each file (truncation + refusal + unknown-tool; the
      malformed-frame notice adds one more).
- [ ] `grep -rc "if let Ok(v) = serde_json::from_str" src/ai/backends/anthropic.rs src/ai/backends/gemini.rs src/ai/backends/openai.rs`
      prints `0` for every file (currently `1` each — replaced by the
      counting match).
- [ ] `grep -c "The model returned an empty reply" src/daemon/stream.rs`
      prints `1` (currently `0`).
- [ ] All four gates green (the new variant will fail the build until every
      exhaustive `AiEvent` match has its arm — that is the mechanism, not a
      problem).
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

See Spec Task 5 — this phase is deliberately light on new unit tests: the
changes are event plumbing pinned by grep criteria, and the user-visible
behavior (a SystemMsg reaching the client) is exercised live at milestone
close with an unknown-tool prompt against a compat server. Existing backend
tests must keep passing.

## End-to-end verification

```sh
A=/tmp/e2e-07.txt; : > "$A"
grep -c "Notice(String)" src/ai/types/events.rs >> "$A"
grep -c "AiEvent::Notice" src/daemon/stream.rs >> "$A"
grep -c "AiEvent::Notice" src/ai/backends/anthropic.rs src/ai/backends/gemini.rs src/ai/backends/openai.rs >> "$A"
grep -rc "if let Ok(v) = serde_json::from_str" src/ai/backends/anthropic.rs src/ai/backends/gemini.rs src/ai/backends/openai.rs >> "$A"
grep -c "The model returned an empty reply" src/daemon/stream.rs >> "$A"
cargo test 2>&1 | grep -E "^test result:" >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (append the verdict line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-07-surface-silent-conditions.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07.txt
diff /tmp/pasted-07.txt /tmp/e2e-07.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Changing what `AiEvent::Error` does, or making any Notice terminal.
- The Gemini `MALFORMED_FUNCTION_CALL` recovery path (already surfaces its
  own error).
- Persisting notices into session history (decided against: notices are
  advisory UI, not conversation content).
- New AI tools (no `TOOLS` change → no CLAUDE.md/README tool-table edits).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
