# M9 — Operator Tooling

**Goal:** Give the operator a way to repair the memory index. It is a derived
cache with no entry point of its own: if it goes stale or corrupt, the only
remedy today is deleting the file by hand and hoping something rebuilds it.

**Status:** planning

**Depends on:** M8 (Test Suite Reliability) — closed 2026-08-02.

**Scoped:** 2026-08-02, PE decision. One phase. This was M7's and M8's
highest-weight carried item and the only one a user could actually hit.

**Exit criteria:**

- [ ] **`daemoneye reindex` exists and rebuilds the memory index**, reporting
      the row count before and after.
- [ ] **It works with no daemon running**, on a bare `$HOME` with no
      `~/.daemoneye/` at all, and on a seeded one — none of which is an error.
- [ ] **It is idempotent**: a second run on an unchanged tree reports the same
      count and exits 0.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
      fmt --all --check` clean; `cargo test` green with no regression against
      the **1032 lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
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
| 01 | [reindex-command](phase-01-reindex-command.md) — `daemoneye reindex`, wired to `reconcile_index()`, with a before/after report | todo |

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

### Measured before scoping

`reconcile_index()` was run directly against three trees:

| Tree | `rows_before` → `rows_after` |
|---|---|
| Bare `$HOME`, no `~/.daemoneye/` at all | `0 → 0`, returns `Ok` |
| Freshly `setup` `$HOME` | `0 → 9` (7 knowledge + 2 session) |
| Same tree, second pass | `9 → 9` |

So the command needs no daemon, tolerates a completely empty tree, and is
idempotent — all three verified rather than assumed.

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
