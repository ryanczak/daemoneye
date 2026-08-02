# NEXT

**Active phase: M8 phase-01 — port-lifetime** (`todo`, drafted 2026-08-02).

Doc: `docs/dev/milestones/M8-test-suite-reliability/phase-01-port-lifetime.md`

Dispatch with `/rexymcp:dispatch phase-01`.

**M8 — Test Suite Reliability** was scoped 2026-08-02 (PE): two phases, one axis.
M7 closed the same day; both M8 items are its leftovers, and phase 02 finishes
M7's single unticked exit criterion.

## Phase 01 — what it is

`cargo test --test isolation` fails about **5% of the time**. Fix: hold the probe
listener from allocation until its real consumer takes over, so two
`IsolatedEnv`s can no longer be handed the same port.

**The mechanism is confirmed, not guessed.** A 100-run baseline gave **5
failures**, at two sites:

| Site | Count | What it is |
|---|---|---|
| `tests/harness/mod.rs:88` | 4 | `bind stub server: AddrInUse` |
| `tests/isolation.rs:184` | 1 | `assert_ne!(env_a.stub_port(), env_b.stub_port())` |

The second is the proof — a test whose only job is to assert two environments get
different ports, failing. So the collision is real; the `AddrInUse` failures are
it seen one step downstream.

**Two hypotheses were disproven first** and are recorded so nobody re-runs them:
the kernel does not hand back a just-freed ephemeral port under tight sequential
allocation (0/200), and a simplified concurrent model did not reproduce it either
(0/480). The real suite differs by running nine tests in parallel while spawning
daemons that consume ephemeral ports of their own.

**The hand-off API was compiled and run before drafting** —
`tokio::net::TcpListener::from_std` adopts the already-bound `std` listener after
`set_nonblocking(true)`, port intact. And eight concurrent allocations with
listeners **held** gave 8/8 distinct ports, which is why the fix works.

**The acceptance criterion is a 100-run loop, not a unit test.** At a 5% rate a
single green run happens 95% of the time on the *unfixed* code — one run cannot
distinguish a fix from luck. A warm isolation run is ~0.33 s, so 100 runs is
about 35 seconds.

## Phase 02 — named only

The four real-clock sleeps in non-`#[ignore]`d tests:
`src/cli/input/tty.rs:370,374` and `src/cli/commands/stream.rs:1265,1268`.
Draft with `/rexymcp:architect next` when 01 is `done`.

## Explicitly not in M8

**`hooks_land_on_private_server`** — the other flake, from phase-04 review. It
binds no ports at all and did **not** fail once in the 100-run baseline, so there
is no live evidence to work from. M7's retrospective originally claimed both
flakes shared a root cause; that was an over-claim and has been corrected there.
If it recurs, it is a separate bug wanting its own investigation.

## Still carried, unscheduled

1. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()`.
2. **`tree_block_of`'s loose error contract** — an unterminated fence returns
   `Some` where the spec said `None`. No reachable consequence.
3. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
4. **`reconcile_index()` has no operator entry point** — deferred twice; a
   `reindex` subcommand or startup hook is the obvious home.

## The rule M7 earned, still in force

> **Do not assert a fact about the system in a spec unless it was executed.**
>
> **Naming a false-success mode is worthless unless the guard is checked against
> it.**

Plus: a phase that deliberately lands code for a *later* phase must say how the
deny-warnings gate is satisfied, and **a green bounce always needs a refined
re-dispatch**.
