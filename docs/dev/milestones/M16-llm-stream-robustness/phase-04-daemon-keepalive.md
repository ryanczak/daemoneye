# Phase 04: Daemon turn-wide keepalive — the client hears from the daemon at least every 15 s

**Milestone:** M16 — LLM Stream Robustness
**Status:** in-progress
**Depends on:** phase-03
**Estimated diff:** ~280 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Today `Response::KeepAlive` is emitted from exactly one place — the 30 s
`ai_rx.recv()` timeout in `run_conversation_loop` — so the client goes silent
during tool execution: `await_agent_result` can block 3600 s, foreground
command polling can run for minutes, and the auto-name AI call adds up to
20 s, all with zero traffic. The client's 120 s deadline then falsely
declares a healthy daemon dead. This phase establishes the protocol contract
**"the daemon sends something at least every ~15 s for the whole turn"**: a
`with_keepalive` wrapper for self-contained waits, inline keepalive sends for
the foreground poll loops, and a timeout on the one unbounded client read.
A failed keepalive write doubles as prompt client-disconnect detection.

## Architecture references

Read before starting:

- `CLAUDE.md` § "Request/Response lifecycle" — where tool execution sits in
  the turn.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Gotchas — read before Task 7 and before writing any Update Log entry

Phase-03 bounced three times. Not once on its stream logic, which was right
from the first run — every time on a **verification that could only return
one answer**, read as confirmation:

- A test asserting a predicate defined inside `mod tests` could not fail when
  the production call site was deleted.
- A mutation of a call site could not fail a test that calls the function
  directly.
- A sample of eight tools could not disagree, because none of the eight
  deserialized the argument under test.

Two of those three were the architect's spec defects, not the executor's. The
rule that would have caught all three, and the one this phase is held to:

> **Run every check once in the state where it is expected to fail.** A check
> that has never produced its own negative is not evidence, however green it
> is.

Concretely, for this phase: before you paste a passing test run, break the
thing the test guards and capture the failing run too. And if a criterion in
this doc turns out to be unsatisfiable or already-passing, **say so and stop**
— report it as a blocker in the Update Log. Do not produce output shaped like
what the criterion asked for. A wrong criterion is the architect's defect to
fix, and reporting it is the fastest path to a correct phase.

## Current state

(Re-derived 2026-08-17 immediately before staging — the line numbers below
are from that run, not from the 2026-08-16 drafting pass. Phases 01–03
touched only `src/ai/`, so the daemon-side facts moved by a few lines at
most. Re-run the greps if anything reads stale.)

- `Response::KeepAlive` is `src/ipc.rs:504`. Its sole emitter is
  `src/daemon/stream.rs:138-147` (re-derived 2026-08-17):

```rust
let event = match tokio::time::timeout(std::time::Duration::from_secs(30), ai_rx.recv())
    .await
{
    Ok(Some(ev)) => ev,
    Ok(None) => break,
    Err(_timeout) => {
        // No token in 30 s — send a keep-alive so the client doesn't
        // hit its per-token deadline (slow local LLMs can stall longer).
        send_response_split(tx, Response::KeepAlive).await?;
        continue;
    }
};
```

- `send_response_split` (`src/daemon/utils/response.rs:12-20`) is generic
  over `W: tokio::io::AsyncWriteExt + Unpin + ?Sized`.
- `await_agent_result` call site: `src/daemon/executor/mod.rs:843` inside
  `execute_tool_call<W, R>` (where `tx: &mut W` is in scope):

```rust
PendingCall::AwaitAgentResult {
    job_id,
    agent_name,
    timeout_secs,
    ..
} => {
    knowledge::await_agent_result(job_id, agent_name, *timeout_secs, &memory_namespaces)
        .await
}
```

  The callee (`src/daemon/executor/knowledge/agents.rs:250-257`) polls the
  mailbox every 2 s under a `tokio::time::timeout` of up to
  `MAX_TIMEOUT_SECS = 3600` and borrows neither `tx` nor `rx`.
- Auto-name call site: `src/daemon/stream.rs:824` (re-derived 2026-08-17):

```rust
if should_suggest
    && let Some((name, desc)) =
        auto_name::suggest_session_name(&messages, config).await
{
```

  (`suggest_session_name` has an internal 20 s timeout,
  `src/daemon/auto_name.rs`.)
- `PaneSelectPrompt` read: `src/daemon/executor/mod.rs:1191` —
  `rx.read_line(&mut pane_line).await?;` with **no timeout**, unlike the
  approval read at :992 which uses
  `tokio::time::timeout(APPROVAL_TIMEOUT, rx.read_line(&mut line))`.
  `APPROVAL_TIMEOUT` (60 s) and `USER_PROMPT_TIMEOUT` (120 s) are at
  `src/daemon/executor/mod.rs:67-69`.
- Foreground execution poll loops in `src/daemon/executor/foreground.rs`
  (`tx` is in scope in all of them; find with
  `grep -n "tokio::time::sleep" src/daemon/executor/foreground.rs`):
  the six poll loops whose `tokio::time::sleep` calls re-derived 2026-08-17 to
  **:451** (sudo fingerprint detect), **:679** (interactive connect,
  `select!` arm), **:703** (interactive settle), **:730** (remote
  output-stability, `select!` arm), **:799** and **:845** (local child poll).
  Each can run for minutes sending nothing. A seventh sleep at **:859** is
  `POST_CMD_CAPTURE_DELAY` — a one-shot delay, **not** a poll loop; it is
  deliberately excluded and must not get a `maybe_keepalive` call.

## Spec

### Task 1 — New module `src/daemon/utils/keepalive.rs`

Create the module (register it in `src/daemon/utils/mod.rs` following the
existing one-file-per-concern pattern) with:

```rust
use crate::ipc::Response;
use std::time::Duration;

/// Protocol liveness contract: while a turn is in flight the daemon sends
/// *something* at least this often. The CLI's dead-daemon deadlines are
/// derived from this with ≥ 6x margin — change it and they must change too
/// (src/cli/commands/stream.rs).
pub const KEEPALIVE_PERIOD_SECS: u64 = 15;

/// Drive `fut` to completion while sending `Response::KeepAlive` every
/// [`KEEPALIVE_PERIOD_SECS`]. A failed keepalive write means the client is
/// gone — the error propagates immediately, which is deliberate: it turns a
/// vanished client into a prompt turn abort instead of a much later EPIPE.
pub async fn with_keepalive<W, F, T>(tx: &mut W, fut: F) -> anyhow::Result<T>
where
    W: tokio::io::AsyncWriteExt + Unpin + ?Sized,
    F: std::future::Future<Output = T>,
{
    tokio::pin!(fut);
    let mut tick =
        tokio::time::interval(Duration::from_secs(KEEPALIVE_PERIOD_SECS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // interval fires immediately on first tick; burn it so the first
    // keepalive goes out after one full period, not at once.
    tick.tick().await;
    loop {
        tokio::select! {
            out = &mut fut => return Ok(out),
            _ = tick.tick() => {
                super::response::send_response_split(tx, Response::KeepAlive).await?;
            }
        }
    }
}

/// Inline variant for poll loops that interleave their own `tx` writes:
/// call once per iteration; sends a keepalive when the last one is older
/// than the period.
pub async fn maybe_keepalive<W>(
    tx: &mut W,
    last: &mut std::time::Instant,
) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin + ?Sized,
{
    if last.elapsed() >= Duration::from_secs(KEEPALIVE_PERIOD_SECS) {
        super::response::send_response_split(tx, Response::KeepAlive).await?;
        *last = std::time::Instant::now();
    }
    Ok(())
}
```

(Adjust the `super::response::` path to however sibling utils actually
reference each other — check an existing cross-references in
`src/daemon/utils/`; `crate::daemon::utils::response::send_response_split`
is always safe.)

Cancel-safety note (this is why the shape is a pinned future in a `select!`
loop): `fut` is polled continuously and never dropped until it completes;
only `tick.tick()` is cancelled, which is explicitly cancel-safe.

### Task 2 — Wrap `await_agent_result`

At `src/daemon/executor/mod.rs:843`, the arm becomes:

```rust
PendingCall::AwaitAgentResult {
    job_id,
    agent_name,
    timeout_secs,
    ..
} => {
    crate::daemon::utils::keepalive::with_keepalive(
        tx,
        knowledge::await_agent_result(job_id, agent_name, *timeout_secs, &memory_namespaces),
    )
    .await?
}
```

(`with_keepalive` returns `anyhow::Result<anyhow::Result<ToolCallOutcome>>`
here; the `.await?` unwraps the keepalive layer and the arm yields the inner
`Result`, matching the surrounding `match ... }?;`.)

### Task 3 — Wrap the auto-name call

In `src/daemon/stream.rs:824`, restructure the `&& let` chain so the
call is wrapped (the suggestion future borrows `messages`/`config`, not
`tx`):

```rust
if should_suggest {
    let suggestion = crate::daemon::utils::keepalive::with_keepalive(
        tx,
        auto_name::suggest_session_name(&messages, config),
    )
    .await?;
    if let Some((name, desc)) = suggestion {
        // existing hint construction + send, unchanged
    }
}
```

### Task 4 — Bound the pane-select read

At `src/daemon/executor/mod.rs:1191`, replace the bare read with the
established timeout pattern from :992:

```rust
let read_result =
    tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut pane_line)).await;
```

On `Err(_elapsed)`: `send_response_split(tx, Response::Error("Pane selection
timed out after 120s".to_string())).await?;` then return an `Err` the same
way the surrounding aborted-response arm does. On `Ok(r)` proceed with `r?`
as today. (No keepalive here: the client is inside its own interactive
dialog during this wait, not in the streaming loop.)

### Task 5 — Keepalive the foreground poll loops

In `src/daemon/executor/foreground.rs`, in each polling loop listed in
Current state (sleeps at :451, :679, :703, :730, :799, :845 — **not** :859): declare
`let mut last_ka = std::time::Instant::now();` before the loop and call
`crate::daemon::utils::keepalive::maybe_keepalive(tx, &mut last_ka).await?;`
once per iteration, next to the existing sleep. Where a loop is inside a
`tokio::select!` arm structure, place the call at the top of the loop body
instead. Do not restructure the loops otherwise.

### Task 6 — Unify the streaming keepalive period

In `src/daemon/stream.rs:138`, replace the literal
`Duration::from_secs(30)` with
`Duration::from_secs(crate::daemon::utils::keepalive::KEEPALIVE_PERIOD_SECS)`
and update the comment (the 30 s value predates the turn-wide contract).

### Task 7 — Unit tests

New tests in `src/daemon/utils/keepalive.rs` (`mod tests`), all
`#[tokio::test(start_paused = true)]`. Use `tokio::io::duplex(1024)` for the
socket: the write half is `tx`, the read half is inspected after (`Vec<u8>`
does **not** implement tokio's `AsyncWrite` — don't try it):

- `keepalive_ticks_while_future_pends` — wrap a future gated on
  `tokio::sync::Notify` (never notified inside the test window); advance time
  ~46 s via `tokio::time::advance`; then notify, await the wrapper, and
  assert the read half contains ≥ 3 serialized `KeepAlive` lines (parse each
  line with `serde_json` and count `Response::KeepAlive`).
- `keepalive_returns_future_output` — wrap `async { 42 }`, assert
  `Ok(42)` and that **zero** keepalives were written (completes before the
  first period).
- `keepalive_write_failure_aborts` — drop the read half of the duplex first;
  advance past one period; assert the wrapper returns `Err`.
- `maybe_keepalive_respects_period` — call twice back-to-back: first send
  goes out only after `last` is aged (construct
  `Instant::now() - Duration::from_secs(16)` via checked_sub… `Instant`
  arithmetic on paused time: simpler to `advance(16s)` between calls);
  assert exactly one KeepAlive line total.

Note on the duplex buffer: keepalive lines are ~20 bytes; a 1024-byte duplex
holds ~50 of them, ample for these tests. Do not read concurrently.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-04.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "pub const KEEPALIVE_PERIOD_SECS" src/daemon/utils/keepalive.rs`
      prints `1` (file currently absent).
- [ ] `grep -c "with_keepalive" src/daemon/executor/mod.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "with_keepalive" src/daemon/stream.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "maybe_keepalive" src/daemon/executor/foreground.rs` prints
      `6` (currently `0`; one per loop listed in Task 5 — if a loop turns
      out to be unreachable for `tx` or structurally exempt, document why in
      Notes for review and re-pin this count to the shipped number **before**
      running the E2E block).
- [ ] `grep -c "from_secs(30)" src/daemon/stream.rs` prints `0`
      (currently `1`).
- [ ] `grep -c "USER_PROMPT_TIMEOUT, rx.read_line(&mut pane_line)" src/daemon/executor/mod.rs`
      prints `1` (currently `0`).
- [ ] `cargo test keepalive_ 2>&1 | grep -c '\.\.\. ok$'` prints `4` — one
      per Task 7 test (currently `0`).

      **Corrected at pre-dispatch re-derive 2026-08-17.** This was drafted as
      "`cargo test keepalive` passes (all Task 7 tests)". Measured: that
      command **passes today**, before this phase does anything, because the
      filter matches phase-02's
      `delta_carries_token_ignores_empty_keepalive` — and `cargo test <filter>`
      exits 0 even when the filter matches nothing, so "passes" was never
      going to discriminate. The trailing underscore in `keepalive_` excludes
      that test (measured `0` today), and `grep -c '\.\.\. ok$'` counts
      individual passing-test lines, never the per-binary `test result: ok.`
      summaries. Each of the four names was also measured individually at `0`.
- [ ] Each Task 7 test passes by name — `keepalive_ticks_while_future_pends`,
      `keepalive_returns_future_output`, `keepalive_write_failure_aborts`,
      `maybe_keepalive_respects_period` (each `grep -c '\.\.\. ok$'` = `1`;
      all four currently `0`).
- [ ] **At least one Task 7 test was run in a state where it fails.** Before
      pasting the passing run, break the thing the test guards — e.g. change
      `KEEPALIVE_PERIOD_SECS` to a value larger than the window
      `keepalive_ticks_while_future_pends` advances through — capture the
      `FAILED` line, restore, and capture the passing line. Paste both.
      **A test that has never produced its own negative is not evidence** (see
      Gotchas above). If a test cannot be made to fail, say so and stop — that
      is a blocker worth reporting, not a step to improvise past.
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tests enumerated in Spec Task 7. Deterministic: paused time, no real sleeps,
duplex streams (STANDARDS § 3.3).

## End-to-end verification

Live long-tool keepalive observation happens at milestone close (session
JSONL + attached client). Hermetic evidence:

```sh
A=/tmp/e2e-04.txt; : > "$A"
grep -c "pub const KEEPALIVE_PERIOD_SECS" src/daemon/utils/keepalive.rs >> "$A"
grep -c "with_keepalive" src/daemon/executor/mod.rs >> "$A"
grep -c "with_keepalive" src/daemon/stream.rs >> "$A"
grep -c "maybe_keepalive" src/daemon/executor/foreground.rs >> "$A"
grep -c "from_secs(30)" src/daemon/stream.rs >> "$A"
cargo test keepalive 2>&1 | grep -E "^test " >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | grep -E "^test result:" >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (append the verdict line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-04-daemon-keepalive.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04.txt
diff /tmp/pasted-04.txt /tmp/e2e-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Changing the CLI's deadlines (phase-06).
- The `Ok(None)` retry hole and the chat-task `JoinHandle` (phase-05).
- Keepalives during ghost-shell turns (ghost sessions have no attached
  client socket).
- Restructuring the foreground poll loops beyond the one-line
  `maybe_keepalive` insertion.
- Making `KEEPALIVE_PERIOD_SECS` configurable — it is a protocol constant.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-17 23:50 (progress)

Started phase-04. Flipped status to in-progress (phase doc + milestone README
row). Reading the daemon stream/executor sources and the existing
`send_response_split` plumbing before implementing the keepalive module and
wrappers.

### Update — 2026-08-18 00:20 (progress)

All Spec tasks 1–7 implemented and passing individually. Notable adaptation:
`with_keepalive`/`maybe_keepalive`/`KEEPALIVE_PERIOD_SECS` are re-exported
from `crate::daemon::utils` (module-private `keepalive`), so call sites use
`crate::daemon::utils::with_keepalive` etc., not
`crate::daemon::utils::keepalive::…`. Paused-clock test detail: a single
`tokio::time::advance(46s)` only fires the first re-armed timer, so
`keepalive_ticks_while_future_pends` steps the clock in three 16 s
increments; the duplex read half needs `drop(tx)` before `read_to_end` to
hit EOF. The full `cargo test` suite shows one pre-existing failure
(`event_log::tests::sweeping_a_segment_leaves_other_segments_indexed`)
reproduced on a clean stashed tree — unrelated to this phase (see Notes).

### Update — 2026-08-18 00:25 (end-to-end verification)

The phase doc's E2E block, verbatim and unmodified, produced
`/tmp/e2e-04.txt`; its 19 lines are captured below, verbatim:

```text
1
1
1
6
0
test ai::backends::openai::tests::delta_carries_token_ignores_empty_keepalive ... ok
test daemon::utils::keepalive::tests::keepalive_returns_future_output ... ok
test daemon::utils::keepalive::tests::maybe_keepalive_respects_period ... ok
test daemon::utils::keepalive::tests::keepalive_write_failure_aborts ... ok
test daemon::utils::keepalive::tests::keepalive_ticks_while_future_pends ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1310 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s
exit=0
test result: FAILED. 1314 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.22s
exit=101
```

Reading of the five greps against the real artifacts, and what the two test
runs mean:

- `KEEPALIVE_PERIOD_SECS` grep → `1`; `with_keepalive` in executor → `1`;
  `with_keepalive` in stream → `1`; `maybe_keepalive` in foreground → `6`;
  `from_secs(30)` in stream → `0`; pane read → `1` (all six criteria tick).
- `cargo test keepalive` prints all four Task-7 tests `ok` (plus the
  phase-02 `delta_carries_token_ignores_empty_keepalive`, which the phase's
  `keepalive_` filter excludes).
- The full-suite `cargo test` shows `1314 passed; 1 failed`. The single
  failure (`event_log::tests::sweeping_a_segment_leaves_other_segments_indexed`)
  reproduces on a clean tree at `8557a75` with this phase's changes stashed —
  it is pre-existing and unrelated to this phase (the four M16 phase gates are
  the standard commands; this failure predates phase-04).

Repro evidence — the required negative run, captured before the passing run:

```text
=== NEGATIVE 1: break the keepalive contract (KEEPALIVE_PERIOD_SECS = 999s) ===
test daemon::utils::keepalive::tests::keepalive_ticks_while_future_pends ... FAILED
---- keepalive_ticks_while_future_pends stdout ----
expected >= 3 keepalives over 46 s, got 0: ""
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1314 filtered out
```

Passing run restored `KEEPALIVE_PERIOD_SECS = 15`:

```text
test daemon::utils::keepalive::tests::keepalive_ticks_while_future_pends ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1311 filtered out
```

Paste-fidelity self-check (verbatim from the phase doc, run from the repo
root):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-04-daemon-keepalive.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04.txt
diff /tmp/pasted-04.txt /tmp/e2e-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

(The `diff` in that block compares the extraction of this entry against the
live capture. It was run from the repo root; result:

```text
PASTE MATCH
```
)




