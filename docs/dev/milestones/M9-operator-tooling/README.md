# M9 — Operator Tooling

**Goal:** Give the operator a way to repair the memory index. It is a derived
cache with no entry point of its own: if it goes stale or corrupt, the only
remedy today is deleting the file by hand and hoping something rebuilds it.

**Status:** closed 2026-08-02 (all four exit criteria met)

**Depends on:** M8 (Test Suite Reliability) — closed 2026-08-02.

**Scoped:** 2026-08-02, PE decision. One phase. This was M7's and M8's
highest-weight carried item and the only one a user could actually hit.

**Exit criteria:**

- [x] **`daemoneye reindex` exists and rebuilds the memory index**, reporting
      the row count before and after.
- [x] **It works with no daemon running**, on a bare `$HOME` with no
      `~/.daemoneye/` at all, and on a seeded one — none of which is an error.
- [x] **It is idempotent**: a second run on an unchanged tree reports the same
      count and exits 0.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
      fmt --all --check` clean; `cargo test` green with no regression against
      the **1032 lib (now 1035, +3 from this milestone) + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
      bug_tracker + 1 doc_truth** baseline M8 closed at.

## Architecture references

- `src/memory/index.rs` — `reconcile_index()` and `ReconcileReport`, built in
  M7 and given their first production caller (reconcile-on-empty) in M7 phase 08.
- `src/cli/commands/audit_prompts.rs:173` `run_audit_prompts()` — the closest
  existing analogue: an operator-facing, no-args, report-producing subcommand.
- `src/main.rs:131` `AuditPrompts` — the variant declaration and dispatch arm to
  copy.

## Phases

| #  | Phase | Status |
|----|-------|--------|
| 01 | [reindex-command](phase-01-reindex-command.md) — `daemoneye reindex`, wired to `reconcile_index()`, with a before/after report | done        |

## Notes

### Why a command and not a startup hook

M7 phase 08 already reconciles **when the index is empty**, which covers the
fresh-install case where seeded memories bypass the write hooks. What nothing
covers is a *stale* index — one with rows in it that no longer match the files
on disk. Reconcile-on-empty will never fire for that, because the row count is
not zero.

A full reconcile at every daemon start would cover it, at the cost of walking
every memory file on every boot. That cost is why M7 phases 07 and 08 both
deferred it. An operator command has none of that cost and is the honest
shape: rebuild is a thing you ask for when you suspect a problem.

### Measured before scoping — and re-measured at review

`reconcile_index()` was called **in-process** against three trees while scoping:

| Tree | `rows_before` → `rows_after` |
|---|---|
| Bare `$HOME`, no `~/.daemoneye/` at all | `0 → 0`, returns `Ok` |
| Freshly `setup` `$HOME` | `0 → 9` (7 knowledge + 2 session) |
| Same tree, second pass | `9 → 9` |

**The bare-`$HOME` row does not describe the shipped command.** Running the real
binary there reports `0 → 9` and creates `~/.daemoneye/` on the way, because the
binary's first-run seeding happens before the command is reached. The in-process
probe skipped that seeding, so it measured a state a user can never observe. The
executor's own end-to-end run reported `0 → 9`; that was correct and the scoping
number was not.

What the three trees *do* establish holds either way: the command needs no
daemon, tolerates a completely empty tree, and is idempotent.

**The rebuild is atomic.** `reconcile_index()` wraps its `DELETE FROM memories`
and every re-insert in a single transaction (`src/memory/index.rs:254` through
`:311`), so a concurrent reader — a running daemon serving a search — sees
either the old index or the new one, never a half-empty one. That is what makes
it safe to run without stopping the daemon.

### What the report cannot tell you

`ReconcileReport` carries row counts only. If a rebuild replaces nine stale rows
with nine correct ones, both numbers read `9` and the output will say the count
did not change. The command still **did** repair the index; the report just
cannot prove content equality. The phase spec requires the output to be worded
so it never claims more than that.

## M9 retrospective — closed 2026-08-02

One phase, `done`, `approved_first_try`, no bugs filed, 41 executor turns. Final
gates: **1035 lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
bug_tracker + 1 doc_truth**, clippy clean, `fmt --check` clean, tree clean.

`daemoneye reindex` closes the last item that had been deferred through two
milestones — the one carried entry with user-facing weight. `reconcile_index()`
had exactly one caller, firing only when the index was empty, so a *stale* index
was unreachable by any code path and unfixable short of deleting
`var/index/memory.db` by hand.

### The executor was right and the architect was wrong — twice

Both defects found at review were **in the spec**, not in the executor's work.
The diff was exactly the three source files the phase named, with
`src/memory/index.rs` untouched.

**1. The false-success mode I named cannot happen.** The spec justified its
end-to-end block by asserting that a subcommand declared but never wired into the
dispatch `match` "compiles clean and passes every unit test." Deleting the arm and
building gives:

```
error[E0004]: non-exhaustive patterns: `Commands::Reindex` not covered
```

`main.rs` has no wildcard arm, so exhaustiveness checking catches it at compile
time. The E2E still earns its keep — it verifies wording, exit codes, and the
bare-`$HOME` path against the shipped binary — but it is not the guard against
unwired dispatch.

This is M7's rule applied to a **negative** claim. M7 earned *"naming a
false-success mode is worthless unless the guard is checked against it."* Here the
check would have shown the mode was imaginary. The generalisation:

> **A claimed failure mode is a fact about the system, so it is covered by "do
> not assert a fact in a spec unless it was executed."** Justifying a test by an
> unexecuted failure story produces tests aimed at the wrong thing.

**2. The bare-`$HOME` measurement was taken through the wrong door.** The scoping
probe called `reconcile_index()` in-process and recorded `0 → 0`. The shipped
binary reports `0 → 9`, because first-run seeding precedes the command. The
executor reported `0 → 9`; I read the discrepancy as executor error and re-ran it
myself before concluding the spec was wrong. Corrected in this README and in
`NEXT.md`.

> **Measure through the same door the user will use.** An in-process probe of the
> function a command wraps is not a measurement of the command. Both numbers were
> honestly obtained; only one describes something a user can observe.

Both of these were caught *because* review re-runs everything independently rather
than reading the executor's report. Neither would have surfaced from green gates —
all four were green, and both facts were wrong.

### Calibration held where it has before

Every acceptance criterion was run against the tree before the spec was committed:
`reindex` absent from `--help` (had to become ≥1), lib 1032 → 1035, no existing
`Reindex` variant. All three were satisfiable and all three landed exactly.

Both load-bearing tests were mutation-checked at review, and each was killed by
exactly the right test and no other — the unchanged-count test by rewording the
Equal arm to "already up to date", the growth test by breaking the delta
arithmetic.

### One wording decision worth keeping

When the row count is unchanged the command says *"count unchanged — the rebuild
still replaced every row"*, and a test pins that it must **not** say "already up to
date". `ReconcileReport` carries counts only, so nine stale rows replaced by nine
correct ones reads `9 → 9`. The rebuild did repair the index and the numbers
cannot show it; an operator who ran the command to fix a suspected problem needs to
know it happened. The reassuring phrasing would have been a small lie.

### Carried forward — nothing scheduled

Unchanged from M8 except that its highest-weight item is now closed. Ordered by
weight:

1. **`read_key` has no timeout**, so a regression makes the `cli::input::tty`
   tests hang rather than fail. In CI a hang is worse than a failure. This is now
   the heaviest carried item.
2. **One residual real-clock sleep** — `src/ai/mod.rs:364`, in a spawned task
   simulating an unresponsive server. Zero wall cost, cannot flake;
   `std::future::pending().await` is the clean expression.
3. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory mapping
   instead of calling `dir_name()`.
4. **`tree_block_of`'s loose error contract** — unterminated fence returns `Some`
   where the spec said `None`. No reachable consequence.
5. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
6. **`hooks_land_on_private_server`** — never reproduced; 0 failures in 300 runs.
   Only a bug if it recurs.
7. **`daemoneye reindex` is undocumented in `CLAUDE.md` and `docs/architecture.md`.**
   New, and deliberate: kept out of phase 01 to keep the diff reviewable. The
   command works and `--help` describes it; the project-level docs do not mention
   it yet.

