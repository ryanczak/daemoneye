# Bug 2 on phase-11: `stream_phase` drops `daemon_recv` on `Warn`/`Tick` returns, still losing partial bytes

**Severity:** major
**Status:** fixed (architect takeover, 2026-06-27)
**Filed:** 2026-06-27

## What's wrong

The re-dispatch correctly moved `daemon_recv` outside the `select!` and polls it by `&mut`
reference — so the future is NOT dropped when another branch wins within a single `select!`
call. That part is right.

**The bug survives because `stream_phase` *returns* for `Warn` and `Tick`.**

```rust
// src/cli/commands/stream.rs  (stream_phase, ~lines 648–695)
async fn stream_phase(...) -> StreamOutcome {
    let mut daemon_recv = Box::pin(recv(rx));   // created once, pinned ✓
    loop {
        tokio::select! {
            key = read_key(stdin) => {
                match action {
                    InterruptAction::Warn => return StreamOutcome::Warn,  // ← drops daemon_recv
                    …
                }
            }
            res = &mut daemon_recv => { … }   // polled by &mut — correct within the select!
            _ = tokio::time::sleep(tick_interval) … => {
                return StreamOutcome::Tick;    // ← drops daemon_recv
            }
        }
    }
}
```

When `stream_phase` returns (any path), its local variable `daemon_recv` is dropped.
`recv` is implemented as:

```rust
// src/cli/commands/ipc_client.rs:53
pub async fn recv(rx: &mut BufReader<OwnedReadHalf>) -> Result<Response> {
    let mut line = String::new();
    let n = rx.read_line(&mut line).await?;   // read_line → read_until(b'\n', &mut line)
    …
}
```

`tokio::io::AsyncBufReadExt::read_line` calls `read_until(b'\n', …)` which transfers bytes
from the `BufReader`'s internal fill buffer into `line` before calling `fill_buf()` for more.
When `fill_buf()` returns `Pending` (socket has no more data yet), the already-transferred
bytes live in `line` — inside the `recv` future's stack frame. **Dropping the future drops
`line` and those bytes are gone.** The `BufReader`'s fill buffer is now empty; the bytes it
already transferred are unrecoverable.

**Consequence:** when `stream_phase` returns `StreamOutcome::Warn` (first interrupt press)
and the BufReader had partially consumed a `Response` JSON line:

1. The caller shows the warning and re-enters the outer loop.
2. The outer loop calls `stream_phase` again with the same `rx`.
3. `recv(rx)` starts fresh — `line` is empty.
4. `read_line` reads only the *tail* of the original JSON line from the socket.
5. `serde_json::from_str(tail)` fails → `StreamOutcome::Error("Connection error: …")`.
6. The streaming turn ends with an error instead of continuing.

This violates **AC: "first ESC/Ctrl+C shows a warning in the live region and streaming
continues."** The same failure can occur on every `StreamOutcome::Tick` return (phase 1,
every 80 ms if a partial response straddles the tick boundary).

**The executor's "Notes for review" make the same incorrect API claim as bounce 1:**
> "The next call to stream_phase recreates the recv from the same rx (which is fine since
> no bytes were consumed by the key branch)."

Within the `select!` call, no bytes are lost (correct — `&mut` polling). At `stream_phase`
return, bytes are lost if `line` was non-empty. The claim is wrong for the same reason.

Additionally: the 4 new tests in `stream_phase_tests` all call `InterruptState::feed()`
directly — they are re-tests of the `interrupt.rs` module tests. **None drives the
`stream_phase` function or the `recv`/BufReader seam.** The integration test required
by bug-phase-11-1's verification ("a hermetic regression test proves a keypress during
streaming does not drop a daemon message") is still absent.

## What should happen

`stream_phase` must keep `daemon_recv` alive for the **entire duration** of one call — it
must handle `Warn` and `Tick` side effects internally without returning. Only return for
`Msg`, `Interrupted`, or `Error`.

## How to fix

**Change `stream_phase` to accept callbacks for the side effects that today cause returns:**

```rust
// Signature change: add on_tick and on_warn callbacks; remove Warn and Tick from StreamOutcome
async fn stream_phase(
    stdin: &AsyncStdin,
    rx: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    interrupt_state: &mut InterruptState,
    tick_interval: std::time::Duration,
    overall_timeout: Option<std::time::Duration>,
    on_tick: &mut impl FnMut(),    // NEW: called on each spinner tick, no return
    on_warn: &mut impl FnMut(),    // NEW: called on first interrupt press, no return
) -> StreamOutcome /* only: Msg | Interrupted | Error */
```

Inside `stream_phase`, on each `select!` iteration:
- **`Warn` case:** call `on_warn()`, then **`continue`** (do NOT return — `daemon_recv` stays alive).
- **`Tick` case:** call `on_tick()`, then **`continue`** (do NOT return — `daemon_recv` stays alive).
- **`Abort` case:** return `StreamOutcome::Interrupted` (dropping `daemon_recv` is fine — we're abandoning the stream).
- **`Msg` case:** return `StreamOutcome::Msg(response)` (future completed — no partial bytes remain).
- **`Error` case:** return `StreamOutcome::Error(e)`.

Remove `StreamOutcome::Warn` and `StreamOutcome::Tick` variants (or keep them if the
caller genuinely needs to know after the fact, but neither should trigger a return that
drops `daemon_recv`).

**Callers pass lambdas:**

```rust
// Phase 1 caller (spinner + interrupt):
let outcome = stream_phase(
    stdin, &mut rx, &mut interrupt_state,
    Duration::from_millis(80),
    None,
    &mut || { /* spin animation */ spin = spin.wrapping_add(1); draw_spinner(…); },
    &mut || { draw_spinner("⚡", "interrupt?", 0, &sb); },
).await;

// Phase 2 caller (streaming + interrupt):
let outcome = stream_phase(
    stdin, &mut rx, &mut interrupt_state,
    Duration::MAX,   // or Option::None — no tick
    Some(Duration::from_secs(120)),
    &mut || {},      // no tick in phase 2
    &mut || { draw_spinner("⚡", "interrupt?", 0, &sb); },
).await;
```

This keeps `daemon_recv` pinned for the full `stream_phase` call. The `BufReader`'s fill
buffer is never discarded mid-line.

**Also add the integration test required by bug-phase-11-1:**

A hermetic test that proves a keypress does not corrupt the stream. The test should:
1. Construct a `BufReader` over a pipe or cursor whose data is a partial JSON line (first
   half), then a key event arrives on the stdin side, then the second half of the line.
2. Call `stream_phase` (or its logical equivalent after the refactor).
3. Assert the full `Response` is received intact (no `Error`) and no message was dropped.

This is the test that would have caught both bounce 1 and bounce 2.

## Verification

- [ ] By inspection: `stream_phase` never returns `StreamOutcome::Warn` or
  `StreamOutcome::Tick` (or equivalent names) — those paths `continue` the internal loop.
- [ ] The hermetic seam integration test (item above) passes and would fail if `daemon_recv`
  were recreated on a warn/tick.
- [ ] Existing `InterruptState` tests still pass; `commit_panel` color test still passes.
- [ ] `cargo build` (0 warnings), `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo fmt --all`, `cargo test` all pass.
