# Bug 1 on phase-02a: `daemoneye reindex` reports phantom added rows on every run after the first

**Severity:** major
**Status:** verified
**Filed:** 2026-08-03
**Fixed:** 2026-08-03 (commit `0f09157`, round 2). `rows_before` now sums the same
five tables `rows_after` does, via the shared `count_table` helper hoisted above
the transaction (`src/memory/index.rs:305-320`).

Independently reverified at review through the shipped binary, same reproduction
as the original report plus one check the bug doc did not ask for — that a *real*
delta is still reported, so the fix is not hardcoded equality:

```
=== run 1 (fresh index) ===
Index rebuilt: 0 → 12 rows (12 added).
=== run 2 (NOTHING changed) ===
Index rebuilt: 12 rows (count unchanged — the rebuild still replaced every row).
=== run 3 (NOTHING changed) ===
Index rebuilt: 12 rows (count unchanged — the rebuild still replaced every row).
=== run 4: after ADDING a runbook ===
Index rebuilt: 12 → 13 rows (1 added).
  artifacts: 4
```

The previously-unreachable `Ordering::Equal` branch fires, and run 4 confirms
genuine changes are still counted. The regression guard is real: reverting
`rows_before` to the memories-only query makes `second_reconcile_reports_no_change`
FAIL.

## What's wrong

**The index itself is correct.** All seven tables are created, `artifacts` and
`epochs` are populated properly, search works, and every new test is real —
three independent mutations were each caught (see § Already verified). The
defect is in the operator-facing report, which is the `reindex` command's only
output.

`rows_before` and `rows_after` count **different things**. `rows_before`
(`src/memory/index.rs:305-307`) is unchanged from v1 and still counts one table:

```rust
let rows_before: i64 = conn
    .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
    .unwrap_or(0);
```

while `rows_after` (`:465`) is now the sum across all five corpora:

```rust
let rows_after = memories_count + artifacts_count + epochs_count + turns_count + events_count;
```

So `format_report` subtracts a memories-only count from an all-corpora count and
reports the difference as rows *added*. On any install with at least one runbook,
script or epoch, every reindex after the first claims rows were added when
nothing changed.

Reproduced through the shipped binary (`./target/debug/daemoneye reindex`)
against a throwaway `HOME` seeded with two runbooks and one script — **the tree
was not touched between runs**:

```
=== run 1 (fresh index) ===
Index rebuilt: 0 → 12 rows (12 added).
  memories: 9
  artifacts: 3
  epochs: 0
  turns: 0
  events: 0
=== run 2 (NOTHING changed on disk) ===
Index rebuilt: 9 → 12 rows (3 added).
  ...
=== run 3 (still nothing changed) ===
Index rebuilt: 9 → 12 rows (3 added).
  ...
```

Runs 2 and 3 are false: nothing was added. The phantom count equals
`artifacts + epochs`, and it repeats forever — the report never converges to the
truth no matter how many times it runs.

A second consequence: `format_report`'s `Ordering::Equal` branch — the message
written specifically for the idempotent case, *"count unchanged — the rebuild
still replaced every row"* — becomes unreachable whenever any non-memory corpus
is non-empty. The one branch that tells the operator "this was a no-op" can no
longer fire.

This is also a direct spec miss. The phase doc § Spec task 2 said, in bold:

> Add one field; keep the existing two as **totals across all corpora** so
> `format_report`'s existing arithmetic and tests keep working.

`rows_after` was made a total; `rows_before` was left alone.

## What should happen

`rows_before` and `rows_after` must measure the same thing — the total row count
across all five corpora — so the delta is meaningful and a no-op rebuild reports
as a no-op. On an unchanged tree, the second and every later `daemoneye reindex`
must print the `Ordering::Equal` message rather than a fabricated "N added".

## How to fix

In `src/memory/index.rs`, `reconcile_index()`: replace the memories-only
`rows_before` query with a total across the same five tables that `rows_after`
sums, counted **before** the transaction opens.

Reuse the existing `count_table` helper rather than writing a second counter.
You do not need to move it: `count_table` is a nested `fn` item, and items
declared in a block are visible throughout that block in Rust — it can be called
above its textual definition without restructuring the function.

Keep `rows_after` and `per_corpus` exactly as they are; they are correct.

## Verification

- [ ] `reconcile_index()` run twice against an unchanged `HOME` returns a report whose `rows_before == rows_after`.
- [ ] A test asserts the idempotence directly: seed a runbook and a script, reconcile, reconcile again, assert the second report has `rows_before == rows_after` and that `rows_before` equals the sum of `per_corpus`. Name it something like `second_reconcile_reports_no_change`.
- [ ] Through the real binary: `daemoneye reindex` run twice against a `HOME` containing at least one runbook prints the "count unchanged" message on the second run, with the actual output pasted into the Update Log.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` all clean.

## Already verified at review — do not redo

These held independently and need no rework:

- **Schema v2 is correct.** All seven tables exist; `user_version` is 2; a v1
  database is dropped and recreated. The contentless `turns`/`events` tables
  correctly carry no UNINDEXED columns, so the NULL-readback trap was avoided.
- **`artifacts` indexing is real.** Replacing the runbook loop's iterator with an
  empty vec makes `reconcile_indexes_runbook_and_script_bodies` FAIL.
- **The stats guard works.** Adding a `load_runbook` call back into the runbook
  loop makes the same test FAIL on its runbooks-executed assertion — the
  pre-injected gotcha is genuinely defended, not just avoided.
- **`epochs` indexing is real.** Removing `failed_cmds` from the epoch body makes
  `reconcile_indexes_epoch_narrative_and_failed_cmds` FAIL.
- **Corpora do not bleed.** A term present only in an `artifacts` body returns
  zero rows from `memories`.
- **Hygiene:** no TODO/FIXME, no `dbg!`/`println!` in production, no `#[allow]`
  or `#[ignore]`, no `unsafe` or `unwrap`/`expect`/`panic!` in the production
  path of either changed file.
- **All four gates** re-run clean at review: fmt, build, clippy exit 0; test
  green at 1051 lib + 6 + 4 + 30 + 9.

## Noted, not a defect — do not "fix"

- `reconcile_indexes_epoch_narrative_and_failed_cmds` writes its `.epochs.jsonl`
  as raw JSONL rather than through `append_epoch`, deviating from the phase doc's
  § Test plan. The risk is nil — `append_epoch` serializes the same struct with
  `serde_json::to_string`, so the on-disk shape is identical, and the test
  hand-builds the same path `sessions_dir()` resolves to, so a path change would
  fail the test rather than silently pass it. The stated rationale ("to avoid
  masking") is inaccurate, since the fixture contains nothing maskable, but the
  choice itself is harmless. Left as-is.
- `count_table` interpolates a table name into SQL with `format!`. Every call
  site passes a hardcoded literal, so there is no injection surface.
