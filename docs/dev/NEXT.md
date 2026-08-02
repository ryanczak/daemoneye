# NEXT

**Active phase: none.**

**M9 — Operator Tooling closed 2026-08-02** (one phase, `approved_first_try`,
no bugs). `daemoneye reindex` ships. Retrospective:
`docs/dev/milestones/M9-operator-tooling/README.md`.

M7, M8 and M9 are all closed. There is no scoped milestone. **Starting one is a
human decision** — the architect does not cross a milestone boundary on its own.

## What the last three milestones left behind

Ordered by weight. Nothing here is scheduled.

1. **`read_key` has no timeout** — now the heaviest item. A regression that stops
   bytes reaching it makes the `cli::input::tty` tests **hang** rather than fail;
   confirmed by mutation at M8 phase-02 review. In CI a hang is worse than a
   failure. A bounded `timeout()` wrapper is the fix.
2. **One residual real-clock sleep** — `src/ai/mod.rs:364`, a 30 s sleep in a
   *spawned* task simulating a server that stops responding. Zero wall cost and
   cannot flake; `std::future::pending().await` says the same thing without a
   clock.
3. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()` — the same class of drift M7 phase-10
   fixed elsewhere.
4. **`daemoneye reindex` is undocumented** in `CLAUDE.md` and
   `docs/architecture.md`. Deliberately out of M9 phase-01's scope to keep the
   diff reviewable. The command works and `--help` describes it.
5. **`tree_block_of`'s loose error contract** — an unterminated fence returns
   `Some` where the spec said `None`. No reachable consequence today.
6. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
7. **`hooks_land_on_private_server`** — the old phase-04-review flake. Never
   reproduced; 0 failures in 300 runs across M8 and M9. Only a bug if it recurs.

Items 1–3 are small and independent; a single "residual hygiene" milestone would
hold all three plus item 4 comfortably. That is a suggestion, not a plan.

## The rules these milestones earned

> **Do not assert a fact about the system in a spec unless it was executed.**
> A *claimed failure mode* is such a fact — M9 justified a test with a
> compile-time impossibility that one `cargo build` would have disproven.
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.** A single green run is not evidence.
>
> **Measure through the same door the user will use.** M9's in-process probe of
> `reconcile_index()` recorded a bare-`$HOME` result the shipped binary never
> produces.

Corollaries, each earned more than once: naming a false-success mode is worthless
unless the guard is checked against it; a phase that lands code for a *later*
phase must say how the deny-warnings gate is satisfied; and **a green bounce
always needs a refined re-dispatch**, never a plain one.
