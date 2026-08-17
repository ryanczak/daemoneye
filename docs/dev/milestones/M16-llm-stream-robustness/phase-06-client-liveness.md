# Phase 06: Client liveness contract — no infinite spinner, phase-accurate timeout errors

**Milestone:** M16 — LLM Stream Robustness
**Status:** todo
**Depends on:** phase-04
**Estimated diff:** ~130 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Give the chat client's phase 1 (before the first content) a deadline so a
wedged daemon can never produce an infinite "scrying…" spinner, and make both
timeout errors name what actually happened. Phase-04 established the daemon
contract (something arrives at least every 15 s for the whole turn), so a
90 s silence bound — 6× the keepalive period — only fires on a genuinely
dead or wedged daemon.

## Architecture references

Read before starting:

- `src/daemon/utils/keepalive.rs` — `KEEPALIVE_PERIOD_SECS` (the constant
  the client deadlines are derived from; landed in phase-04).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phase-04 landed:
   `grep -c "KEEPALIVE_PERIOD_SECS" src/daemon/utils/keepalive.rs` must be
   ≥ 1; if the file is absent, stop and file a blocker.

## Current state

(Current as of 2026-08-16; re-derive with
`grep -n "overall_timeout\|last_msg_at\|Daemon stopped responding" src/cli/commands/stream.rs`.)

`src/cli/commands/stream.rs`, `ask_with_session_ratatui` — the two-phase
timeout selection (~lines 172–183):

```rust
// Both phases animate a spinner on an 80 ms tick so a mid-stream pause
// (e.g. a tool round-trip or a slow model) never looks frozen. Phase 1
// (before the first content) has no overall timeout; phase 2 keeps a
// 120 s deadline measured from the last message via `last_msg_at`.
let (tick_interval, overall_timeout) = if !response_started {
    (std::time::Duration::from_millis(80), None)
} else {
    let remaining =
        std::time::Duration::from_secs(120).saturating_sub(last_msg_at.elapsed());
    (std::time::Duration::from_millis(80), Some(remaining))
};
```

`last_msg_at` is reset on **every** daemon message including `KeepAlive`
(~line 259). The deadline expiry lives inside `select_stream` (~line 729):

```rust
return StreamOutcome::Error("Daemon stopped responding (120 s timeout)".to_string());
```

so the message cannot distinguish phase 1 from phase 2 — today it never
fires in phase 1 at all (`overall_timeout = None`), which is the infinite
spinner. `StreamOutcome::Error` is otherwise used for genuine connection
errors (EOF → `"Daemon closed connection unexpectedly."`, ~line 675 region).

`src/cli/commands/ask.rs` (~lines 95–99) already bounds every recv at 120 s
(KeepAlives reset it via `continue`) — it needs only the reworded error.

## Spec

### Task 1 — Named deadline constants

Near the top of `src/cli/commands/stream.rs` add:

```rust
/// Client-side silence bounds, derived from the daemon's
/// `KEEPALIVE_PERIOD_SECS` (15 s) with >= 6x margin: while a turn is in
/// flight the daemon sends *something* at least every 15 s, so 90 s of
/// total silence before the first content means the daemon is hung, not
/// slow. Phase 2 keeps the pre-existing 120 s.
const PHASE1_SILENCE_TIMEOUT_SECS: u64 = 90;
const PHASE2_SILENCE_TIMEOUT_SECS: u64 = 120;
```

### Task 2 — Give phase 1 a deadline

Replace the selection quoted above so both phases carry a deadline measured
from `last_msg_at`:

```rust
let (tick_interval, overall_timeout) = {
    let budget = if !response_started {
        std::time::Duration::from_secs(PHASE1_SILENCE_TIMEOUT_SECS)
    } else {
        std::time::Duration::from_secs(PHASE2_SILENCE_TIMEOUT_SECS)
    };
    (
        std::time::Duration::from_millis(80),
        Some(budget.saturating_sub(last_msg_at.elapsed())),
    )
};
```

Update the comment block above it (the "Phase 1 … has no overall timeout"
sentence is no longer true).

### Task 3 — Phase-accurate expiry outcome

Add a `Deadline` variant to `StreamOutcome` (no payload) and change the
expiry site inside `select_stream` (~line 729) to
`return StreamOutcome::Deadline;`. In the caller's outcome `match`, add:

```rust
StreamOutcome::Deadline => {
    let msg = if !response_started {
        format!(
            "No response from the daemon for {PHASE1_SILENCE_TIMEOUT_SECS}s — \
             it appears hung (a healthy daemon signals liveness every 15s \
             even while the AI is thinking). Try `daemoneye status`, or check \
             ~/.daemoneye/var/log/daemon.log."
        )
    } else {
        format!(
            "Daemon went silent mid-response (no data or keep-alive for \
             {PHASE2_SILENCE_TIMEOUT_SECS}s). Abandoning the connection; the \
             daemon may still be running — check `daemoneye status`."
        )
    };
    return Err(anyhow::anyhow!("Connection error: {}", msg));
}
```

(Match the surrounding arms' style for how errors are surfaced/returned —
follow the existing `StreamOutcome::Error` arm at ~line 201.)

### Task 4 — Reword the `ask.rs` timeout

In `src/cli/commands/ask.rs` (~line 98), replace the message
`"Daemon stopped responding (120 s timeout)"` with
`"Daemon went silent for 120s (no data or keep-alive) — it appears hung. Try `daemoneye status`."`
(keep the flat per-recv timeout shape unchanged).

### Task 5 — Unit test for the deadline selection

Extract the phase-budget choice into a pure helper so it is testable:

```rust
fn silence_budget(response_started: bool) -> std::time::Duration {
    std::time::Duration::from_secs(if response_started {
        PHASE2_SILENCE_TIMEOUT_SECS
    } else {
        PHASE1_SILENCE_TIMEOUT_SECS
    })
}
```

(use it in Task 2), and add tests in this file's test module (create one if
absent):

- `silence_budget_phase1_is_90s` — asserts
  `silence_budget(false) == Duration::from_secs(90)`.
- `silence_budget_phase2_is_120s` — asserts
  `silence_budget(true) == Duration::from_secs(120)`.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-06.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "PHASE1_SILENCE_TIMEOUT_SECS" src/cli/commands/stream.rs`
      is ≥ `2` (const + at least one use; currently `0`).
- [ ] `grep -c "Deadline" src/cli/commands/stream.rs` is ≥ `2` (variant +
      arm(s); currently `0`).
- [ ] `grep -c "Daemon stopped responding" src/cli/commands/stream.rs src/cli/commands/ask.rs`
      prints `0` for both files (currently `1` each).
- [ ] `cargo test silence` reports both Task 5 tests passing (the shared
      `silence` substring in their names is what the filter matches).
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tests in Spec Task 5. Both names carry the `silence` substring so one
`cargo test silence` filter runs them; keep that substring if you rename.

## End-to-end verification

The live wedged-daemon check (`kill -STOP` mid-turn → client errors ≤ 90 s)
is run by the architect at milestone close. Hermetic evidence:

```sh
A=/tmp/e2e-06.txt; : > "$A"
grep -c "PHASE1_SILENCE_TIMEOUT_SECS" src/cli/commands/stream.rs >> "$A"
grep -c "Daemon stopped responding" src/cli/commands/stream.rs src/cli/commands/ask.rs >> "$A"
cargo test silence 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (append the verdict line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-06-client-liveness.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06.txt
diff /tmp/pasted-06.txt /tmp/e2e-06.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any daemon-side change.
- The interrupt/Esc flow (`interrupt.rs`) — phase-08.
- Changing the 80 ms spinner tick or the renderer.
- Making the deadlines configurable.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
