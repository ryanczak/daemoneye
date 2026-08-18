# Phase 07: Surface silent conditions — truncation, refusal, unknown tools, malformed frames, empty replies

**Milestone:** M16 — LLM Stream Robustness
**Status:** in-progress
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

## Gotchas — read before Task 2 and before writing any Update Log entry

**1. Gemini calls `dispatch_tool_event` TWICE per `functionCall`, and only one
of them is an emission site.** Phase-03 added `part_counts_as_token`
(`gemini.rs:14`), a first-token predicate that calls
`dispatch_tool_event("", fn_name, &args, None).is_some()` at **`gemini.rs:39`**
purely to ask "would this tool dispatch?". The real emission site is
**`gemini.rs:364`**. If you emit the unknown-tool `Notice` from wherever
`dispatch_tool_event` returns `None`, you will fire it **from the predicate
too** — once per frame, before the drain loop even decides to emit, and again
at :376. Emit the notice **only** at the `:364` emission site's `else` branch
(where `log::warn!("model called unknown tool …")` already sits at
**`:376`**). Do **not** touch `part_counts_as_token`; it is a pure predicate
and must stay silent.

**2. Anthropic's unknown-tool site is in a helper, not the drain loop.** It is
`flush_tool_call` at **`anthropic.rs:71`** — which does take
`tx: &UnboundedSender<AiEvent>`, so it can emit a notice directly. Do not go
looking for it in the drain loop; it is not there.

**3. This phase is compiler-driven by design.** Adding `AiEvent::Notice`
breaks every exhaustive `match` on `AiEvent` until each gains an arm. `cargo
build` failing right after Task 1 is the mechanism working, **not** a blocker
— add the arms the compiler names and continue. The consumer files that
reference `AiEvent` today are: `src/daemon/stream.rs`, `ghost.rs`,
`briefing.rs`, `auto_name.rs`, `digest.rs`, `scheduled.rs`, `session.rs`,
`src/webhook/process.rs`, `src/cost.rs`, plus the three backends and
`src/ai/tools/dispatch.rs` / `args.rs` (the last two construct events rather
than matching them, so they may need nothing). **Let the compiler be the
authority on which files need an arm — do not add arms speculatively.**

**4. The verification discipline this milestone runs on** — phases 04, 05 and
06 all followed it and were approved first try:

> **Run every check once in the state where it is expected to fail.** A check
> that has never produced its own negative is not evidence, however green it
> is.

If a criterion here turns out unsatisfiable or already-passing, **say so and
stop** — report it as a blocker in the Update Log rather than producing output
shaped like what it asked for. Every criterion below was measured in its
failing state during staging.

## Current state

(**Re-derived 2026-08-18 immediately before staging.** The drafted line
numbers were stale — phases 03 and 05 moved this code substantially. Every
number below is from that re-run; prefer them over anything the Spec text
still says.)

- `AiEvent` is `src/ai/types/events.rs`; its only non-tool variants are
  `Token`, `Error(String)` (terminal), `Done(TokenBreakdown)` (terminal),
  plus one struct variant per tool. Files matching on `AiEvent` (add a
  log-and-ignore arm wherever the compiler demands after Task 1 —
  candidates): `src/daemon/stream.rs`, `src/daemon/ghost.rs`,
  `src/daemon/briefing.rs`, `src/daemon/auto_name.rs`,
  `src/daemon/digest.rs`, `src/daemon/scheduled.rs`,
  `src/daemon/session.rs`, `src/webhook/process.rs`, `src/cost.rs`.
- Log-only sites (all three backends), **line numbers re-derived 2026-08-18**:
  - `src/ai/backends/anthropic.rs:321-330` — `match v["delta"]["stop_reason"]`
    with `"max_tokens"` (`:323`) / `"refusal"` (`:328`).
  - `src/ai/backends/gemini.rs:288-296` — `finishReason` `"MAX_TOKENS"`
    (`:288`) / `"SAFETY"` (`:293`).
  - `src/ai/backends/openai.rs:275-284` — `finish_reason` `"length"` (`:275`)
    / `"content_filter"` (`:280`).
  - Unknown-tool drops, all three `log::warn!("model called unknown tool
    '{...}' — call dropped")`: **`anthropic.rs:71`** (inside `flush_tool_call`,
    *not* the drain loop — see § Gotchas 2), **`openai.rs:324`**, and
    **`gemini.rs:376`** (the `else` of the `dispatch_tool_event` call at
    `:364` — see § Gotchas 1 for the predicate at `:39` you must NOT touch).
  - Malformed SSE frames: each backend parses its `data:` payload with
    `if let Ok(v) = serde_json::from_str::<Value>(&data)` — **`anthropic.rs:236`,
    `gemini.rs:266`, `openai.rs:236`** — so a malformed frame is silently
    skipped.
- The empty-reply path: `src/daemon/stream.rs` `AiEvent::Done` arm — when
  `pending_calls.is_empty()`, the daemon pushes the assistant message only
  `if !full_response.is_empty()` at **`:727`**, then sends `UsageUpdate` +
  `Response::Ok` at **`:909`**. Nothing tells the user the reply was empty.
- `Response::SystemMsg` is `src/ipc.rs:390` and already renders in the chat
  client (used for auto-name hints and catch-up briefs) — no CLI change
  needed.

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

### Update — 2026-08-18 07:46 (progress)

Started phase 07. Flipped phase doc + milestone README to `in-progress`.
Adding `AiEvent::Notice(String)` and letting the compiler drive the match-arm
additions.

### Update — 2026-08-18 08:02 (progress)

All six code tasks landed: `AiEvent::Notice(String)` with its stream.rs
SystemMsg arm and log-and-ignore arms in the remaining consumers; truncation/
refusal/unknown-tool notices in all three backends (unknown-tool notice emitted
only at the real emission sites — anthropic `flush_tool_call`, gemini :382 else
branch, openai dispatch else); malformed-frame counting with a single
end-of-stream notice; empty-reply guard in `stream.rs` Done arm. Renamed
`flush_unknown_tool_sends_nothing` → `flush_unknown_tool_sends_notice` and
asserted the Notice payload (that test had to change: the unknown-tool path now
*does* emit). Full gate run pending fmt.

### Update — 2026-08-18 08:05 (end-to-end verification)

Ran the § End-to-end verification block verbatim and unmodified into
`/tmp/e2e-07.txt`. Full transcript, mechanically captured (`cat
/tmp/e2e-07.txt`):

```text
1
1
src/ai/backends/anthropic.rs:5
src/ai/backends/gemini.rs:4
src/ai/backends/openai.rs:4
src/ai/backends/anthropic.rs:0
src/ai/backends/gemini.rs:0
src/ai/backends/openai.rs:0
1
test result: ok. 1321 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.09s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
```
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
```
exit=0
```

Paste-fidelity check (phase doc § End-to-end verification, run verbatim):
the fenced block above diffs cleanly against `/tmp/e2e-07.txt` → PASTE MATCH.
