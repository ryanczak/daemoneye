# NEXT

**Active phase: none.** M7 — Memory Search & Maintenance is **closed**
(2026-08-02). Ten phases, all `done`; retrospective in
`docs/dev/milestones/M7-memory-search-and-maintenance/README.md`.

**The next milestone is a human decision and has not been scoped.** Run
`/rexymcp:architect` when you want to design it.

## Where the tree stands

- **1032 lib + 30 integration (2 ignored) + 8 isolation (1 ignored) + 6
  bug_tracker + 1 doc_truth**; clippy clean; `cargo fmt --all --check` clean.
- Working tree clean. No bug doc `open` — `bug-04-1`, `bug-06-1` and `bug-08-1`
  are all `verified`.
- Memory search is live: BM25-ranked FTS5 over `var/index/memory.db`, maintained
  on every write, with `reconcile_index()` covering the fresh-install case.

## Carried forward into whatever comes next

None of these are scheduled. Full detail in the M7 retrospective.

1. **`tests/isolation.rs` flakiness — a trend.** Two occurrences, two different
   port-binding tests, both `AddrInUse`-shaped. Wants ephemeral ports or
   serialised port-binding tests. **The oldest unscheduled item.**
2. **Four real-clock sleeps remain in non-`#[ignore]`d tests** —
   `src/cli/input/tty.rs:370,374`, `src/cli/commands/stream.rs:1265,1268`. This
   is why M7's exit criterion 8 is marked partly-met rather than ticked.
3. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()`.
4. **`tree_block_of`'s loose error contract** — an unterminated fence returns
   `Some` where the spec said `None`. No reachable consequence.
5. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
6. **`reconcile_index()` has no operator entry point** — deferred twice; a
   `reindex` subcommand or startup hook is the obvious home.

## The lesson M7 earned, for whoever writes the next specs

Every spec fact **executed** against the real system before drafting was
implemented correctly and needed no correction. Every defect in phases 06–08
came from the parts written from assumption instead.

> **Do not assert a fact about the system in a spec unless it was executed.**
>
> **Naming a false-success mode is worthless unless the guard is checked against
> it** — state the fixture property that makes the mutation detectable.

Two procedural corollaries, both twice-earned: a phase that deliberately lands
code for a *later* phase must say in the spec how the deny-warnings gate is to
be satisfied; and **a green bounce always needs a refined re-dispatch**, never a
plain one.
