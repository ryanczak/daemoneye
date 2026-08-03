# Phase 02a: Index schema v2 — all tables, plus the stored-content corpora

**Milestone:** M11 — Unified Knowledge Index
**Status:** in-progress
**Depends on:** phase-01 (done — write-time masking, so epoch text is safe to index)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Take `var/index/memory.db` from one FTS5 table to the full M11 schema: five FTS
tables plus two sidecar maps at `SCHEMA_VERSION = 2`, and extend
`reconcile_index()` to populate the two **new stored-content** corpora
(`artifacts`, `epochs`). The two contentless corpora (`turns`, `events`) get
their tables created here but are populated in phase 02b.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Schema (SCHEMA_VERSION 2)" — the table
  inventory and why corpora are separate tables rather than one.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`src/memory/index.rs` today holds exactly one table. This is the code you are
extending — **modify it, do not rewrite the file**:

```rust
pub const SCHEMA_VERSION: i64 = 1;

pub fn ensure_schema(conn: &rusqlite::Connection) -> Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .with_context(|| "reading user_version")?;

    if current != 0 && current != SCHEMA_VERSION {
        conn.execute_batch("DROP TABLE IF EXISTS memories")
            .with_context(|| "dropping stale memories table")?;
    }

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS memories USING fts5(
            key, namespace UNINDEXED, category UNINDEXED,
            tags, summary, body,
            tokenize = 'porter unicode61 remove_diacritics 2'
        );",
    )
    .with_context(|| "creating memories FTS5 table")?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .with_context(|| "setting user_version")?;
    Ok(())
}
```

`ReconcileReport` (`src/memory/index.rs:222`) is currently:

```rust
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub rows_before: usize,
    pub rows_after: usize,
}
```

`reconcile_index()` (`:231`) opens the index, counts `memories`, opens a
transaction, `DELETE FROM memories`, walks namespaces × categories inserting
rows, commits, re-counts. **Its memory-walking logic is correct and must not
change** — you are adding two more corpora inside the same transaction.

The environment is verified: bundled SQLite is **3.53.2** (libsqlite3-sys
0.38.1), which supports `contentless_delete=1` (added in 3.43).

## Spec

### 1. Bump the schema version and create every table — `src/memory/index.rs`

Set `pub const SCHEMA_VERSION: i64 = 2;`.

In `ensure_schema`, the drop-on-mismatch branch must drop **all** index tables,
not just `memories`:

```rust
if current != 0 && current != SCHEMA_VERSION {
    conn.execute_batch(
        "DROP TABLE IF EXISTS memories;
         DROP TABLE IF EXISTS artifacts;
         DROP TABLE IF EXISTS epochs;
         DROP TABLE IF EXISTS turns;
         DROP TABLE IF EXISTS turns_map;
         DROP TABLE IF EXISTS events;
         DROP TABLE IF EXISTS events_map;",
    )
    .with_context(|| "dropping stale index tables")?;
}
```

Then create all seven. Keep the existing `memories` statement exactly as it is
and add these. This DDL was executed against the bundled SQLite and accepted
verbatim — use it as written:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS artifacts USING fts5(
    kind UNINDEXED,
    name,
    tags,
    body,
    tokenize = 'porter unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE IF NOT EXISTS epochs USING fts5(
    session_id UNINDEXED,
    seq UNINDEXED,
    kind UNINDEXED,
    body,
    tokenize = 'porter unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS turns_map (
    id         INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn       INTEGER NOT NULL,
    offset     INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS turns USING fts5(
    body,
    content='', contentless_delete=1,
    tokenize = 'porter unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS events_map (
    id      INTEGER PRIMARY KEY,
    segment TEXT NOT NULL,
    offset  INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS events USING fts5(
    event,
    body,
    content='', contentless_delete=1,
    tokenize = 'porter unicode61 remove_diacritics 2'
);
```

**Gotcha — do NOT add UNINDEXED columns to `turns` or `events`.** It is
tempting: putting `session_id UNINDEXED` on `turns` looks simpler than a join.
SQLite **accepts** the DDL, so it compiles and runs — and then every read is
silently wrong. Measured against the bundled 3.53.2 with
`fts5(body, sid UNINDEXED, content='', contentless_delete=1)`:

```
SELECT sid FROM t WHERE rowid=1            -> Ok(None)     (NULL, not the value)
SELECT sid FROM t WHERE t MATCH 'hello'    -> Ok(None)     (NULL)
SELECT count(*) FROM t
  WHERE t MATCH 'hello' AND sid='sess-a'   -> Ok(0)        (silently zero rows)
```

A contentless table stores **no column values at all** — only the rowid and the
inverted index. All metadata for `turns`/`events` lives in the map tables and is
reached by joining `map.id = fts.rowid`. That join was also executed and works:

```sql
SELECT m.session_id, m.turn, m.offset, bm25(turns)
  FROM turns t JOIN turns_map m ON m.id = t.rowid
 WHERE turns MATCH ?1
 ORDER BY bm25(turns) LIMIT 10
```

**No migration path is needed.** A v1 database has `user_version = 1`, hits the
drop branch, and is rebuilt from disk. `fts5_search` already reconciles when the
index is empty, so memory search self-heals on the next query. Do not write
upgrade/backfill code.

### 2. Per-corpus counts in `ReconcileReport` — `src/memory/index.rs`

Add one field; keep the existing two as **totals across all corpora** so
`format_report`'s existing arithmetic and tests keep working:

```rust
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub rows_before: usize,
    pub rows_after: usize,
    /// Per-corpus row counts after the rebuild, in a stable order:
    /// memories, artifacts, epochs, turns, events.
    pub per_corpus: Vec<(String, usize)>,
}
```

`Vec<(String, usize)>` keeps `PartialEq, Eq` derivable — do not use a `HashMap`.

### 3. Index the `artifacts` corpus in `reconcile_index()` — `src/memory/index.rs`

Inside the existing transaction, after the memory loop, clear and repopulate
`artifacts` from runbooks and scripts.

**Gotcha — do NOT call `crate::runbook::load_runbook()`.** It looks like the
right API and it is not: `src/runbook.rs:190` calls
`crate::daemon::stats::inc_runbooks_executed()`, so a reconcile over N runbooks
would report N phantom runbook executions in the operator's stats. Read the file
directly instead.

The safe API set, verified to have no stats side effects:

| Need | Use | Notes |
|---|---|---|
| runbook names + tags | `crate::runbook::list_runbooks()` → `Vec<RunbookInfo>` | `RunbookInfo { name, tags, ghost_config }` |
| runbook body | `std::fs::read_to_string(crate::config::runbooks_dir().join(format!("{name}.md")))` | raw file text, frontmatter included — that is intentional, index it as-is |
| script names + tags | `crate::scripts::list_scripts_with_tags()` → `Vec<(ScriptInfo, Vec<String>)>` | `ScriptInfo { name, size }` |
| script body | `crate::scripts::read_script(name)` | no stats side effect |

Do **not** try to call `runbook::parse_frontmatter` or
`scripts::read_script_tags` — both are private (`fn`, not `pub fn`) and making
them public is out of scope.

Rows: `kind` is the literal `"runbook"` or `"script"`; `name` is the artifact
name; `tags` is the tag list joined with `" "`; `body` is the file text. A file
that fails to read is skipped, not fatal — mirror the memory loop, which uses
`let Ok(raw) = std::fs::read_to_string(&path) else { continue; };`.

### 4. Index the `epochs` corpus in `reconcile_index()` — `src/memory/index.rs`

Enumerate session ids by scanning `crate::config::sessions_dir()` for files
whose name ends in `.epochs.jsonl`; the session id is the filename with that
suffix removed. For each id call `crate::daemon::context::epochs::read_epochs(id)`
(a pure file read, no side effects) and insert one row per `EpochRecord`:

- `session_id` — the id from the filename
- `seq` — `rec.seq`
- `kind` — `rec.kind` (`"epoch"` or `"chapter"`)
- `body` — the record's searchable text: `rec.narrative` (when `Some`), each
  `rec.tally.failed_cmds` command string, and each `rec.artifacts` entry, joined
  with a space or newline

Fields to skip: timestamps, counts, `covers`. They are not text worth matching.

Epoch text is already masked at write time (phase-01), so no masking call
belongs here.

### 5. Report per-corpus counts — `src/cli/commands/reindex.rs`

`format_report` keeps its existing three-branch total line unchanged, then
appends a per-corpus breakdown built from `report.per_corpus` — one line, or one
line per corpus, your choice of layout. Keep the function pure so its unit tests
stay simple. Update the existing `format_report` tests only as far as the new
field forces (they construct `ReconcileReport` literals).

## Acceptance criteria

- [ ] A fresh `HOME` with no index: `open_index()` creates all seven tables
      (`memories`, `artifacts`, `epochs`, `turns`, `turns_map`, `events`,
      `events_map`) and `PRAGMA user_version` reads `2`.
- [ ] A database written at `user_version = 1` with a row in `memories` is
      dropped and recreated on the next `ensure_schema`: the seven tables exist,
      `user_version` is `2`, and the stale row is gone.
- [ ] `reconcile_index()` over a seeded `HOME` containing one runbook and one
      script produces two `artifacts` rows, and an FTS query matching text in a
      runbook **body** returns it (the case a name-only index would miss).
- [ ] `reconcile_index()` over a `HOME` containing an `<id>.epochs.jsonl` with
      two records produces two `epochs` rows, and a query matching text that
      appears only in `tally.failed_cmds` returns the row.
- [ ] `ReconcileReport.per_corpus` reports all five corpus names after a
      reconcile, and `rows_after` equals the sum of the per-corpus counts.
- [ ] `turns` and `events` remain **empty** after `reconcile_index()` — they are
      populated in phase 02b. Assert this explicitly so the split stays honest.
- [ ] Existing memory search is unaffected: the pre-existing `index.rs` tests
      pass unchanged, including `fresh_index_is_reconciled_on_first_search`.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green with no existing test removed or `#[ignore]`d.

## Test plan

Add to `src/memory/index.rs`'s existing `mod tests`. Follow the home-guard
convention **already used in that module** — do not import a different guard:

```rust
let _guard = crate::test_home_guard();
let tmp = tempfile::tempdir().unwrap();
unsafe { std::env::set_var("HOME", tmp.path()) };
```

- `schema_v2_creates_every_table` — asserts all seven names exist in
  `sqlite_master` and `user_version` is 2.
- `stale_v1_database_is_dropped_and_recreated` — open a connection, create a v1
  `memories` table, `pragma_update user_version = 1`, insert a row, call
  `ensure_schema`, assert version 2 and zero rows.
- `reconcile_indexes_runbook_and_script_bodies` — seed one runbook and one
  script; assert two `artifacts` rows and that a MATCH on body text hits.
- `reconcile_indexes_epoch_narrative_and_failed_cmds` — write an
  `<id>.epochs.jsonl` with two records via `append_epoch`; assert two `epochs`
  rows and that a MATCH on a `failed_cmds` string hits.
- `reconcile_leaves_contentless_corpora_empty` — asserts `turns` and `events`
  have zero rows after a reconcile (the phase-02b boundary).
- `reconcile_report_per_corpus_sums_to_total` — asserts the invariant.

**Negative cases to pin** (each must NOT happen):

- A runbook indexed into `artifacts` must NOT increment the runbooks-executed
  stat. Assert via `crate::daemon::stats` before/after if a getter exists;
  otherwise assert `load_runbook` is not referenced from `index.rs` — a
  `grep -n "load_runbook" src/memory/index.rs` returning nothing is sufficient
  and is checked in § End-to-end verification.
- Searching `memories` for a term that exists only in an `artifacts` body must
  return **no** rows — corpora must not bleed into one another.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib memory::index 2>&1 > /tmp/phase02a-tests.txt; echo "exit=$?" >> /tmp/phase02a-tests.txt
{ echo "--- load_runbook must not appear (stats side effect) ---";
  grep -n "load_runbook" src/memory/index.rs || echo "OK: no load_runbook reference";
  echo "--- schema version ---";
  grep -n "SCHEMA_VERSION: i64" src/memory/index.rs;
  echo "--- tables created ---";
  grep -cE "CREATE (VIRTUAL )?TABLE IF NOT EXISTS" src/memory/index.rs;
} > /tmp/phase02a-schema.txt 2>&1; echo "exit=$?" >> /tmp/phase02a-schema.txt
```

The first file must show every `memory::index` test passing. The second must
show no `load_runbook` reference, `SCHEMA_VERSION: i64 = 2`, and a table count
of `7`.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry and its "Command output tails" block do not satisfy this** —
it proves the gates ran, not that these criteria were exercised. Do not
summarise the results in prose instead of pasting the captured files.

## Authorizations

- Modify: `src/memory/index.rs`, `src/cli/commands/reindex.rs`.
- No new dependencies. No changes to `src/memory.rs`, `src/runbook.rs`,
  `src/scripts.rs`, or `src/daemon/context/epochs.rs`.

## Out of scope

- **Populating `turns` and `events`** — phase 02b. Create the tables, leave them
  empty.
- **Incremental index writes** at any choke point — phase 03. This phase only
  does full reconcile.
- Any read surface: `fts5_search`, `recall_context`, `search_repository` are
  untouched. `fts5_search` continues to query `memories` only.
- Per-column BM25 weighting, and any change to `build_match_expr`.
- Retention/GC of index rows.

## Update Log

### Update — 2026-08-03 19:07 (started)

**Executor:** model

Bumped `SCHEMA_VERSION` to 2, created all seven index tables (memories, artifacts, epochs, turns, turns_map, events, events_map), extended `reconcile_index()` to populate artifacts (runbooks + scripts) and epochs corpora, added `per_corpus` field to `ReconcileReport`, and updated `format_report` with per-corpus breakdown.

### Update — 2026-08-03 19:07 (end-to-end verification)

**Test output:**
```
running 29 tests
test memory::index::tests::stale_schema_version_is_recreated ... ok
test memory::index::tests::unindexed_columns_filter_but_do_not_match ... ok
test memory::index::tests::fts5_is_available_and_matches ... ok
test memory::index::tests::stale_v1_database_is_dropped_and_recreated ... ok
test memory::index::tests::add_memory_indexes_the_row ... ok
test memory::index::tests::empty_query_returns_no_hits ... ok
test memory::index::tests::delete_memory_removes_the_row ... ok
test memory::index::tests::expired_memory_is_not_indexed ... ok
test memory::index::tests::fresh_index_is_reconciled_on_first_search ... ok
test memory::index::tests::ftsearch_memories_preserves_rank_order ... ok
test memory::index::tests::hyphenated_query_does_not_error ... ok
test memory::index::tests::multi_word_query_matches_non_adjacent_terms ... ok
test memory::index::tests::index_failure_does_not_fail_add_memory ... ok
test memory::index::tests::open_index_creates_database_and_schema ... ok
test memory::index::tests::namespace_filter_excludes_other_namespaces ... ok
test memory::index::tests::open_index_is_idempotent ... ok
test memory::index::tests::open_index_sets_schema_version ... ok
test memory::index::tests::operator_words_are_treated_as_text ... ok
test memory::index::tests::reconcile_indexes_runbook_and_script_bodies ... ok
test memory::index::tests::reconcile_after_incremental_writes_is_a_no_op ... ok
test memory::index::tests::reconcile_indexes_epoch_narrative_and_failed_cmds ... ok
test memory::index::tests::reconcile_rebuilds_from_disk ... ok
test memory::index::tests::reconcile_leaves_contentless_corpora_empty ... ok
test memory::index::tests::reconcile_report_per_corpus_sums_to_total ... ok
test memory::index::tests::same_key_in_two_namespaces_is_two_rows ... ok
test memory::index::tests::schema_v2_creates_every_table ... ok
test memory::index::tests::search_finds_text_hit_when_tags_miss ... ok
test memory::index::tests::search_ranks_better_match_first ... ok
test memory::index::tests::update_memory_replaces_the_row_not_duplicates_it ... ok

test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 1022 filtered out; finished in 0.05s

exit=0
```

**Schema verification:**
```
--- load_runbook must not appear (stats side effect) ---
OK: no load_runbook reference
--- schema version ---
7:pub const SCHEMA_VERSION: i64 = 2;
--- tables created ---
8
exit=0
```

Note: `grep -c` returned 8 because the test `stale_v1_database_is_dropped_and_recreated` also contains a `CREATE VIRTUAL TABLE IF NOT EXISTS memories` DDL string. The production `ensure_schema` creates exactly 7 tables (6 `CREATE VIRTUAL TABLE` + 1 `CREATE TABLE` for `turns_map` and `events_map` are counted as non-virtual). The 8th match is the test fixture — all 7 production tables are present.
