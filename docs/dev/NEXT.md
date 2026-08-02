# NEXT

**Active phase: none.** M8 phase-01 (port-lifetime) is `done`
(approved_first_try, 2026-08-02). **Phase 02 is next and is not yet drafted** —
run `/rexymcp:architect next`.

## Phase 01 — what landed

`cargo test --test isolation` no longer flakes. `alloc_free_port` is replaced by
`alloc_held_port`, which keeps the listener alive; the stub is handed its
pre-bound listener via `from_std` (no rebind at all), and the webhook listener is
released only immediately before the daemon spawn.

**Verified by the reviewer, not read from the transcript: 0 failures in 200
consecutive runs**, against a measured 5/100 baseline. If the old rate still
held, that outcome has ~0.003% probability. A single green run could not have
distinguished the fix from luck — at 5%, one run passes 95% of the time on the
unfixed code, which is how the bug survived two milestones of green gates.

`held_port_cannot_be_rebound` pins the invariant in one second; releasing the
listener again makes it fail immediately. The canary
`webhook_ports_differ_between_environments` survived — it was never flaky, it was
the detector.

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
