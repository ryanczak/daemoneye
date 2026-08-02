# NEXT

**Active phase: M9 phase-01 — reindex-command** (`todo`, drafted 2026-08-02).
**This is M9's only phase.**

Doc: `docs/dev/milestones/M9-operator-tooling/phase-01-reindex-command.md`

Dispatch with `/rexymcp:dispatch phase-01`.

**M9 — Operator Tooling** was scoped 2026-08-02 (PE): one phase. This was M7's
and M8's highest-weight carried item and the only one a user could actually hit —
a derived index with no repair command is one corruption away from needing a
manual `rm`.

## Phase 01 — what it is

`daemoneye reindex`, wired to the existing `reconcile_index()`. The function was
built in M7 and has exactly one caller: the reconcile-on-empty branch that fires
only when the index has **zero** rows. A *stale* index — rows present but
wrong — is unreachable by any current code path.

**Measured before drafting, against three real trees:**

| Tree | `rows_before` → `rows_after` |
|---|---|
| Bare `$HOME`, no `~/.daemoneye/` | `0 → 9` — the binary seeds the tree first |
| Freshly `setup` | `0 → 9` |
| Same tree again | `9 → 9` |

So the command needs no daemon, tolerates an empty tree, and is idempotent — all
inherited, none of it new logic. **The rebuild is also atomic**: `DELETE` plus
every re-insert share one transaction (`src/memory/index.rs:254`-`:311`), so a
running daemon serving a search sees the old index or the new one, never a
half-empty one. That is what makes it safe to run without stopping the daemon,
and the help text says so.

**One wording decision is load-bearing.** When the row count is unchanged the
output says *"count unchanged — the rebuild still replaced every row"*, and a
test pins that it must **not** say "already up to date". `ReconcileReport`
carries counts, so nine stale rows replaced by nine correct ones reads `9 → 9`.
The rebuild did repair the index; the numbers just cannot show it, and an
operator who ran the command to fix a suspected problem needs to know it
happened.

**The E2E block is the real test**, because it runs the shipped binary three
times. A subcommand that is declared but never wired into the dispatch `match`
compiles clean, passes every unit test, and fails the instant anyone types it.

## Carried forward — nothing scheduled

Ordered by weight; full detail in the M8 retrospective.

1. **`read_key` has no timeout**, so a regression makes the `cli::input::tty`
   tests hang rather than fail. In CI a hang is worse than a failure.
2. **One residual real-clock sleep** — `src/ai/mod.rs:364`, in a spawned task
   simulating an unresponsive server. Zero wall cost, cannot flake;
   `std::future::pending().await` is the clean expression.
3. **`src/daemon/context/epochs.rs:618`** hardcodes the category→directory
   mapping instead of calling `dir_name()`.
4. **`tree_block_of`'s loose error contract** — unterminated fence returns `Some`
   where the spec said `None`. No reachable consequence.
5. **The phase-04 fence toggle is a flip-flop, not a nesting parser.**
6. **`hooks_land_on_private_server`** — never reproduced; 0 failures in 300 runs.
   Only a bug if it recurs.

Also unrecorded elsewhere: `daemoneye reindex` will deserve a line in `CLAUDE.md`
and `docs/architecture.md`. Deliberately out of phase 01's scope to keep the diff
reviewable; worth folding into a later docs pass.

## The rules M7 and M8 earned

> **Do not assert a fact about the system in a spec unless it was executed.**
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.**

Corollaries, each twice-earned: naming a false-success mode is worthless unless
the guard is checked against it; a phase that lands code for a *later* phase must
say how the deny-warnings gate is satisfied; and **a green bounce always needs a
refined re-dispatch**, never a plain one.
