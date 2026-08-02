# M8 — Test Suite Reliability

**Goal:** Make the test suite trustworthy. A gate that fails 5% of the time
teaches people to re-run it, and a suite that sleeps on the wall clock is slow
for no benefit. Both are M7 leftovers, and one of them is M7's single unticked
exit criterion.

**Status:** closed 2026-08-02 (all four exit criteria met)

**Depends on:** M7 (Memory Search & Maintenance) — closed 2026-08-02.

**Scoped:** 2026-08-02, PE decision. Deliberately small: two phases, one axis.
The PE chose "fix the class and verify empirically" over a diagnose-first phase —
and the diagnosis then arrived anyway during scoping (see Notes).

**Exit criteria:**

- [x] **`cargo test --test isolation` passes 100 consecutive runs.** The
      measured baseline before any change is **5 failures / 100** (see Notes).
      Verified by running it, not asserted.
- [x] **`alloc_free_port()` no longer hands out a port it has released.** The
      probe listener is held until the real consumer takes it over — the
      in-process stub receives the pre-bound listener directly; the daemon's
      webhook port is released only immediately before spawn.
- [x] **No real-clock `sleep` in a non-`#[ignore]`d test.** This is M7's
      exit criterion 8, which closed partly-met. All four sites are gone
      (`tty.rs:370,374`, `stream.rs:1265,1268`). **One residual is named in the
      retrospective**: `src/ai/mod.rs:364`, a 30 s sleep in a *spawned* task
      simulating an unresponsive server — zero wall cost, cannot flake, and not
      one of the four this criterion was written about.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
      fmt --all --check` clean; `cargo test` green with no regression against the
      **1032 lib + 30 integration (2 ignored) + 8 isolation (1 ignored) + 6
      bug_tracker + 1 doc_truth** baseline M7 closed at.

## Architecture references

- `tests/harness/mod.rs` — `IsolatedEnv`, `alloc_free_port()` at line 325, and
  the stub bind at line 88 that fails.
- `tests/isolation.rs:176` `webhook_ports_differ_between_environments` — already
  a canary for the collision; it is one of the two observed failure sites.
- `docs/dev/milestones/M7-memory-search-and-maintenance/README.md` §
  retrospective, carried item 1 — where this was first recorded and twice
  corrected.

## Phases

| #  | Phase | Status |
|----|-------|--------|
| 01 | [port-lifetime](phase-01-port-lifetime.md) — hold the probe listener until its consumer takes over; verify 0/100 against a measured 5/100 baseline | done |
| 02 | [test-sleep-removal-2](phase-02-test-sleep-removal-2.md) — the four real-clock sleeps in `tty.rs` and `stream.rs`; finishes M7's exit criterion 8 | done |

**Both phases are drafted.** Phase 01 is `done`; phase 02 is the last in-scope
phase of the milestone.

## Notes

### The mechanism, confirmed during scoping

The PE chose to fix the class without waiting on a diagnosis. The diagnosis
arrived anyway, and it is worth recording because it makes the fix precise
rather than speculative.

`alloc_free_port()` (`tests/harness/mod.rs:325`):

```rust
fn alloc_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to port 0");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);   // <- the port is free again from here
    port              // <- the caller binds it much later
}
```

A 100-run baseline gave **5 failures**, at two sites:

| Site | Count | What it is |
|---|---|---|
| `tests/harness/mod.rs:88` | 4 | `bind stub server: Os { code: 98, AddrInUse }` |
| `tests/isolation.rs:184` | 1 | `assert_ne!(env_a.stub_port(), env_b.stub_port())` |

**The second site is the proof.** It is a test whose entire purpose is to assert
that two environments get different ports, and it failed — so two `IsolatedEnv`s
really were handed the identical port. The `AddrInUse` failures are the same bug
observed one step downstream.

Two earlier hypotheses were **disproven** before this, and both are recorded so
nobody re-runs them: the kernel does not hand back a just-freed ephemeral port on
tight sequential allocation (0/200), and a simplified concurrent model of
alloc-close-rebind did not reproduce it either (0/480). The real suite differs
from both models by running nine tests in parallel while spawning real daemons
that consume ephemeral ports of their own.

### Why holding the listener is the fix

Two live listeners cannot hold the same port, so holding the probe listener from
allocation until hand-off makes the collision impossible rather than unlikely.
Measured during scoping: eight concurrent allocations with the listeners **held**
produced 8/8 distinct ports.

The hand-off is clean for the in-process stub —
`tokio::net::TcpListener::from_std` accepts the already-bound `std` listener after
`set_nonblocking(true)`. Verified by compiling and running it: the port survived
the hand-off intact.

The daemon's webhook port is harder, because the daemon is a **separate process**
that binds the port itself from `config.toml`. There the listener is held for the
whole of setup and dropped immediately before `Command::spawn`, which shrinks the
window from "all of `IsolatedEnv` construction plus config writing" to the spawn
call itself — and, more importantly, guarantees no *other* `IsolatedEnv` in the
same process can have been given that port in the meantime.

### The one thing this milestone cannot promise

`hooks_land_on_private_server` — the phase-04-review flake — **binds no ports at
all**. It starts a daemon and asserts tmux hook values. It did not fail once in
the 100-run baseline, so there is no live evidence to work from, and phase 01
will not claim to fix it. If it recurs after M8, it is a separate bug and wants
its own investigation. M7's retrospective originally asserted a shared root cause
for both flakes; that was an over-claim and has been corrected there.

## M8 retrospective — closed 2026-08-02

Two phases, both `done`, both `approved_first_try`, no bugs filed. Final gates:
**1032 lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
bug_tracker + 1 doc_truth**, clippy clean, `fmt --check` clean, tree clean.

### The headline number

| | before | after |
|---|---|---|
| `cargo test --test isolation` | **5 failures / 100** | **0 / 300** |

300 runs total: 200 at phase-01 review, 100 more at close against the
post-phase-02 tree. If the original 5% rate still held, zero failures in 300 runs
has probability `0.95^300 ≈ 0.000002%`.

### What made this milestone work

**Measuring the baseline before writing the spec.** The acceptance criterion was
a repeat count derived from a measured rate, not "the suite passes". That
mattered because *one run of a 5%-flaky suite passes 95% of the time* — the
automatic gate capture every phase receives could never have distinguished the
fix from luck, and that is exactly how the bug survived two milestones of green
gates and cost review attention twice before anyone measured it.

**The generalisable rule, now earned rather than assumed:**

> An acceptance criterion for an **intermittent** failure must be a repeat count
> derived from a **measured** rate. A single green run is not evidence.

### The mechanism, and how nearly it was mis-diagnosed

`alloc_free_port()` released its probe listener before the consumer bound the
port, so two `IsolatedEnv`s could be handed the same one. The fix holds the
listener until hand-off: the in-process stub receives the pre-bound listener via
`TcpListener::from_std`, and the daemon's webhook port is released only
immediately before spawn.

Worth recording that **the mechanism was hypothesised twice and disproven both
times** during scoping — the kernel does not hand back a just-freed ephemeral
port under tight sequential allocation (0/200), and a simplified concurrent
model did not reproduce it (0/480). Only running the real suite 100 times
produced evidence, and the decisive datum was not the `AddrInUse` panic everyone
had been looking at but a single failure of
`assert_ne!(env_a.stub_port(), env_b.stub_port())` — the collision itself.

The fix aimed at the *plausible* mechanism would have worked anyway. That is
luck. The method was measuring.

### Calibration held; the automation did not

Every acceptance-criterion command was run against the tree before each spec was
committed, and that caught two would-be-unsatisfiable criteria in phase 02 alone:
`stream.rs` has four `tokio::time::sleep` calls of which **three are production**
(the streaming loop's timeout and tick), and `tty.rs` has six `from_millis(10)`
of which **five are production** escape-sequence timeouts. A criterion demanding
either reach zero would have been impossible, and a blanket deletion would have
broken streaming with every test still green.

Against that, **the automated sleep audit over-flagged on all three attempts** —
at M7 close and twice here. It cannot see `#[tokio::test(start_paused = true)]`,
it mis-attributes enclosing functions across closures, and it treats "after a
`#[cfg(test)]`" as "inside the test module". Every conclusion in this milestone
about which sleeps matter came from reading the code, not from the script. That
is the honest reason no sleep-forbidding gate was built: **a scanner this
architect could not get right in three attempts is not one to enforce in CI.**

### Carried forward — none scheduled

1. **One residual real-clock sleep** — `src/ai/mod.rs:364`, a 30 s
   `tokio::time::sleep` inside a **spawned** task that simulates a server which
   stops responding, so the client's 300 ms `read_timeout` fires. It costs no
   wall time (the whole `ai::tests` module runs in 0.107 s) and cannot flake,
   since it is an upper bound on silence rather than a wait for something.
   `std::future::pending().await` expresses the same intent without a clock and
   is the clean follow-up.
2. **`read_key` has no timeout**, so a regression that stops bytes reaching it
   makes the `cli::input::tty` tests **hang** rather than fail — confirmed by
   mutation at phase-02 review. In CI a hang is worse than a failure. A bounded
   `timeout()` wrapper is the fix.
3. **`hooks_land_on_private_server`** — the phase-04-review flake, which binds no
   ports. It did not fail once in 300 runs across this milestone. If it recurs it
   is a separate bug.
4. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()`.
5. **`tree_block_of`'s loose error contract** — an unterminated fence returns
   `Some` where the spec said `None`. No reachable consequence.
6. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
7. **`reconcile_index()` has no operator entry point** — deferred twice; a
   `reindex` subcommand or startup hook is the obvious home. **The only item on
   this list with user-facing weight.**

### One process note

Phase 02 did not paste its end-to-end transcript into the Update Log — the first
phase across M7 and M8 to skip that step. It was documented in the verdict rather
than bounced, because the change was byte-for-byte what the spec prescribed and
every criterion was independently re-verified. Recorded here so the requirement
is not quietly eroded.
