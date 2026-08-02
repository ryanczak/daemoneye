# NEXT

**Active phase: none.** M8 — Test Suite Reliability is **closed** (2026-08-02).
Two phases, both `done`, both `approved_first_try`, no bugs filed. Retrospective
in `docs/dev/milestones/M8-test-suite-reliability/README.md`.

**The next milestone is a human decision and has not been scoped.** Run
`/rexymcp:architect` when you want to design it.

## Where the tree stands

- **1032 lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
  bug_tracker + 1 doc_truth**; clippy clean; `cargo fmt --all --check` clean.
- Working tree clean. No bug doc `open` anywhere.
- **`cargo test --test isolation`: 0 failures in 300 runs**, against a measured
  5/100 baseline before M8. The suite is trustworthy again.
- Memory search is live (M7): BM25-ranked FTS5 over `var/index/memory.db`.

## Carried forward — nothing scheduled

Full detail in the M8 retrospective. Ordered by weight:

1. **`reconcile_index()` has no operator entry point.** Deferred twice. A
   `reindex` subcommand or a startup hook. **The only item here with user-facing
   weight** — a derived index with no repair command is one corruption away from
   needing a manual `rm`.
2. **`read_key` has no timeout**, so a regression makes the `cli::input::tty`
   tests hang rather than fail. In CI a hang is worse than a failure.
3. **One residual real-clock sleep** — `src/ai/mod.rs:364`, in a spawned task
   simulating an unresponsive server. Zero wall cost, cannot flake;
   `std::future::pending().await` is the clean expression.
4. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()`.
5. **`tree_block_of`'s loose error contract** — unterminated fence returns `Some`
   where the spec said `None`. No reachable consequence.
6. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
7. **`hooks_land_on_private_server`** — the one flake never reproduced. Did not
   fail once in 300 runs. Only a bug if it recurs.

## The two rules M7 and M8 earned

> **Do not assert a fact about the system in a spec unless it was executed.**
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.** A single green run of a 5%-flaky suite passes
> 95% of the time.

Corollaries, each twice-earned: naming a false-success mode is worthless unless
the guard is checked against it; a phase that deliberately lands code for a
*later* phase must say how the deny-warnings gate is satisfied; and **a green
bounce always needs a refined re-dispatch**, never a plain one.

And one caution from M8 specifically: **the automated sleep audit over-flagged on
all three attempts.** It cannot see `start_paused`, mis-attributes enclosing
functions, and confuses "after a `#[cfg(test)]`" with "inside it". Conclusions
about which sleeps matter came from reading the code. That is why no
sleep-forbidding gate was built — a scanner that took three tries to get right by
hand is not one to enforce in CI.
