# M8 — Test Suite Reliability

**Goal:** Make the test suite trustworthy. A gate that fails 5% of the time
teaches people to re-run it, and a suite that sleeps on the wall clock is slow
for no benefit. Both are M7 leftovers, and one of them is M7's single unticked
exit criterion.

**Status:** planning

**Depends on:** M7 (Memory Search & Maintenance) — closed 2026-08-02.

**Scoped:** 2026-08-02, PE decision. Deliberately small: two phases, one axis.
The PE chose "fix the class and verify empirically" over a diagnose-first phase —
and the diagnosis then arrived anyway during scoping (see Notes).

**Exit criteria:**

- [ ] **`cargo test --test isolation` passes 100 consecutive runs.** The
      measured baseline before any change is **5 failures / 100** (see Notes).
      Verified by running it, not asserted.
- [ ] **`alloc_free_port()` no longer hands out a port it has released.** The
      probe listener is held until the real consumer takes it over — the
      in-process stub receives the pre-bound listener directly; the daemon's
      webhook port is released only immediately before spawn.
- [ ] **No real-clock `sleep` in a non-`#[ignore]`d test.** This is M7's
      exit criterion 8, which closed partly-met. Four sites remain:
      `src/cli/input/tty.rs:370,374` and `src/cli/commands/stream.rs:1265,1268`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
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
| 02 | [test-sleep-removal-2](phase-02-test-sleep-removal-2.md) — the four real-clock sleeps in `tty.rs` and `stream.rs`; finishes M7's exit criterion 8 | todo |

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
