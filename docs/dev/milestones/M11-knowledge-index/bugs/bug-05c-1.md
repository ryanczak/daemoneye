# Bug 1 on phase-05c: a search over an empty corpus wipes every other corpus

**Severity:** major
**Status:** open — **PE chose Option 1 (per-corpus reconcile) on 2026-08-06**; spec written into phase-05c, awaiting dispatch
**Filed:** 2026-08-05
**Introduced by:** phase-05a (architect takeover — this is my defect, not the
executor's)
**Surfaced by:** phase-05b, whose guard test it made vacuous
**Filed against 05c** because 05a and 05b are both `done` and correct as
delivered; this is the work item for the phase that will fix it.

## What's wrong

`open_and_reconcile_if_empty(table)` (`src/memory/index.rs`) rebuilds the index
when the *named* corpus is empty:

```rust
let count: i64 = conn.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0);
if count == 0 {
    if let Err(e) = reconcile_index() { … }
```

But `reconcile_index()` is not scoped to that corpus — it clears **all seven
tables** and rebuilds every one from disk:

```rust
tx.execute("DELETE FROM memories", [])
tx.execute("DELETE FROM artifacts", [])
tx.execute("DELETE FROM epochs", [])
tx.execute("DELETE FROM turns", [])
tx.execute("DELETE FROM turns_map", [])
tx.execute("DELETE FROM events", [])
tx.execute("DELETE FROM events_map", [])
```

Phase 05a wired that helper into all four searches. So **searching a corpus that
happens to be empty destroys live rows in every other corpus** — any row whose
on-disk source the reconciler cannot reproduce is gone for good.

Reproduced mechanically (probe added to `src/search.rs` tests, run, reverted):

```
PROBE all(before)                     -> 0 hits
PROBE turns AFTER an 'all' call       -> 0 hits    ← was findable immediately before
PROBE kind=turns (after re-seeding)   -> 1 hits
PROBE kind=epochs (after re-seeding)  -> 1 hits
```

The turn row was findable, one `search_repository(…, "all", …)` call later it was
gone, and re-inserting it made it findable again. Epochs are the clearest loss:
an epoch indexed by `index_epoch` has no `.epochs.jsonl` the reconciler reads
back, so a reconcile deletes it permanently.

## Why this matters in production, not just in tests

The trigger is "some corpus is empty", which is the **normal state of a fresh or
lightly-used install**:

- A user with no memories runs `search_repository(kind="memory")` → `memories` is
  empty → full reconcile → any epochs indexed this session are destroyed.
- A user with no runbooks runs `kind="all"` → `artifacts` is empty → same.
- After any `SCHEMA_VERSION` bump drops and recreates the DB, *every* corpus is
  empty, so the first search of any kind triggers it.

It is silent — no error, no warning, and the search that caused it still returns
plausible results.

## How this surfaced

It made phase-05b's guard test `all_kind_excludes_turns_and_epochs` **pass
vacuously**: the fixture's turn and epoch rows were wiped by the reconcile that
`search_memory_fts` triggered at the head of the `"all"` chain, so "no turns in
the results" was true for the wrong reason. Adding `search_turns_fts` and
`search_epochs_fts` to the `"all"` arm did **not** make the test fail — the test
proved nothing.

The 05b test is repaired by seeding every corpus the `"all"` chain touches, which
makes the guard genuinely non-vacuous (verified: it now fails under mutation with
`Found kind=turns`). **That is a workaround in the test, not a fix for the bug.**

## What should happen

A reconcile triggered by one empty corpus must not destroy other corpora. Two
plausible shapes, and choosing between them is a design decision for the PE:

1. **Per-corpus reconcile** — `reconcile_index()` gains a scope so
   `open_and_reconcile_if_empty("memories")` rebuilds only `memories`. Cleanest,
   but the reconciler currently does one transaction over all seven tables.
2. **Drop the reconcile-on-empty trigger from the three newer searches** and rely
   on `daemoneye reindex` plus the incremental hooks (phases 03a/03b) to keep the
   index current. Simpler, but loses the self-healing property `fts5_search` has
   had since M7.

There is a third consideration either way: an empty corpus is not evidence of a
stale index. A user genuinely having zero memories triggers a full rebuild on
every single search.

## How to fix

Not fixable within 05b — it is a defect in 05a's shipped behavior and the fix
changes `reconcile_index()`'s contract, which several phases depend on. **This
needs its own phase in M11.** Recommend phase 05c, before phase 06, because
phase 06 (prompt scoring) reads the index on every turn and would be exposed to
the same wipe.

## Verification

- [ ] Indexing a turn, then calling `search_repository` with a kind whose corpus
      is empty, leaves the turn still findable.
- [ ] An epoch indexed via `index_epoch` survives a search over an empty corpus.
- [ ] `all_kind_excludes_turns_and_epochs` still fails under mutation **without**
      needing every corpus pre-seeded (i.e. the test's workaround can be removed).
- [ ] `cargo test` green; no regression against the 1111 baseline.
