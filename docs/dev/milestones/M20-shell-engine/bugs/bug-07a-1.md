# Bug 1 on phase-07a: the phase's own client tests turn the `cargo test` gate red

**Status:** open
**Severity:** blocker
**Filed:** 2026-09-06

## What's wrong

`cargo test` — the repo's test gate (`rexymcp.toml` `[commands] test`) — fails
on this tree roughly nineteen runs in twenty:

```
test shell::backend::tests::backend_lagged_subscriber_does_not_break_the_pump ... FAILED
test result: FAILED. 1623 passed; 1 failed; 4 ignored; 0 measured; 0 filtered out
```

Measured at review, `cargo test --lib`, 32-thread libtest default on this host:

| Tree | Runs | Red |
|---|---|---|
| pristine `d1e1e79` (pre-phase, no `client.rs`) | **36** | **0** |
| this tree (`67f36d1`) | **20** | **19** |
| this tree, `-- --skip shell::client::` | **12** | **0** |

The phase's Update Log claims the opposite — that the failure "reproduces at the
same rate on pristine HEAD `d1e1e79`", at "3/24 red (≈12.5%)". **That claim does
not reproduce.** 36 consecutive full-lib runs on a detached worktree at
`d1e1e79` were green. At the claimed 12.5% rate, 36/36 green has probability
≈0.8%. The blocker entry's central premise is not supported by measurement.

The third row is decisive: skipping only the six new `shell::client::` tests,
and changing nothing else, restores 12/12 green on this same tree. The load the
phase's own tests add is what turns the gate red.

## What should happen

STANDARDS.md §1 requires `cargo test` to pass (existing **and** new tests)
before a phase is done. The suite must be green on this tree at the gate's
default configuration — no `RUST_TEST_THREADS` cap, no `#[ignore]`, no skip
filter.

## Root cause

`quiet()` at `src/shell/backend.rs:217` returns as soon as the subscriber sees
**30 ms** with no chunk (`:229`):

```rust
Some(t) if now.duration_since(t) >= Duration::from_millis(30) => return,
```

`backend_lagged_subscriber_does_not_break_the_pump` (`:454`) uses that helper to
decide the flood has finished streaming before asserting the never-read
subscriber has lagged. Under the CPU and PTY contention that phase-07a's six
new PTY-spawning tests add, the 30 ms window elapses in the gap between bash
echoing the flood command line and `yes | head -c 10000000` producing its first
output. `quiet` returns early, `assert_lagged` (`:444`) runs before the flood
has streamed, and the never-read subscriber still holds only the echo — exactly
the observed payload:

```
panicked at src/shell/backend.rs:449:22:
slow subscriber: expected Lagged, got Ok([112, 114, 105, 110, 116, 102, 32, 39,
70, 76, 79, 79, 68, 95, 83, 84, 65, 82, 84, ...])   # "printf 'FLOOD_START\n'; yes | head -c 10000000\r\n"
```

So the fragile assertion is phase-05b's, but it is latent there and never trips
on pristine; phase-07a's tests are what make it fire. A wall-clock silence
window is not a synchronisation primitive — the test needs a condition it can
actually observe (the subscriber having lagged, or the flood's own completion
marker), not an inferred quiet period.

`src/shell/backend.rs` is currently outside this phase's Authorizations. That
list has been widened, narrowly, to permit exactly this test and its helpers —
see the phase doc's Authorizations section. Production code in `backend.rs`
stays off-limits.

## Definition of done

- [ ] `for r in $(seq 1 20); do cargo test --lib 2>&1 | grep -E '^test result:' | head -1; done`
      reports `ok` **20 times, 0 failed** — no `RUST_TEST_THREADS`, no skip
      filter. (Now: 19 of 20 report `FAILED`.)
- [ ] `cargo test` — the full gate, exactly as `rexymcp.toml` runs it — passes
      three consecutive times.
- [ ] `shell::backend::` still reports **7** passing, `0 failed`: the test is
      made deterministic, not deleted or `#[ignore]`d.
- [ ] `grep -c '#\[ignore\]' src/shell/backend.rs` → **0**.
- [ ] The lagged assertion no longer depends on an inferred quiet period. A
      longer sleep is not a fix — it re-tunes the same race. Constraint: the
      test must wait on a condition it can observe (the subscriber actually
      reporting `Lagged`, or a completion marker the flood command itself
      emits), and must hold under the parallel load the client tests create.
      `quiet()` has two other callers (`:477`, `:533`) — changing it in place
      is allowed but must leave those tests green.
- [ ] `git diff d1e1e79 -- src/shell/backend.rs` touches only test code: the
      awk-extracted pre-`#[cfg(test)]` region is unchanged.
