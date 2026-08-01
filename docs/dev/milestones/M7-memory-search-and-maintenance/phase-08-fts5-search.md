# Phase 08: FTS5 Search

**Milestone:** M7 — Memory Search & Maintenance
**Status:** in-progress
**Depends on:** phase-07 (fts5-write-path, done)
**Estimated diff:** ~310 lines — `src/memory/index.rs` (the query path + tests)
plus ~10 lines in `src/daemon/memory_prompt.rs`.

**Tags:** language=rust, kind=feature, size=l

## Goal

`fts5_search()` still returns an empty `Vec`, so `ftsearch_memories()` finds
nothing and real recall is still the grep scan in `src/search.rs`. Make the
search real: BM25-ranked hits, namespace-filtered, with a query builder that
survives ordinary user input.

This is the milestone's headline capability and its first exit criterion.

## Architecture references

- `src/memory/index.rs` — phase 06's schema, phase 07's write path and
  `reconcile_index()`. This phase adds the read path and gives
  `reconcile_index()` its first production caller.
- `src/daemon/memory_prompt.rs:73` — the only caller, and the reason the query
  builder matters: **the query is the entire user turn**, not a search box.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

```rust
// src/memory/index.rs:53
pub fn fts5_search(_query: &str, _limit: usize) -> Vec<(String, f64)> {
    Vec::new()
}
```

Its only caller (`src/daemon/memory_prompt.rs:201`):

```rust
pub fn ftsearch_memories(query: &str, limit: usize, namespaces: &[&str]) -> Vec<MemoryInfo> {
    let results = index::fts5_search(query, limit);
    let all_memories = list_memories_with_tags(None, namespaces).unwrap_or_default();
    let mut found = Vec::new();
    for (key, _) in results {
        if let Some(info) = all_memories.iter().find(|m| m.key == key) {
            found.push(info.clone());
        }
    }
    found
}
```

and that caller is reached from `memory_prompt.rs:73`:

```rust
// 3. FTS5 search against user turn
let fts_candidates = if !user_turn.is_empty() {
    ftsearch_memories(user_turn, 10, namespaces)
```

Two structural problems fall out of those three snippets, and the spec below
fixes both:

- **`fts5_search` cannot filter by namespace**, so `limit` is applied *before*
  the caller drops out-of-namespace hits. Asking for 10 can yield 3.
- **`m.key == key` ignores namespace.** Phase 07's
  `same_key_in_two_namespaces_is_two_rows` proves the same key really can exist
  twice, so key-only matching can surface the wrong memory.

### Everything below was executed against SQLite 3.53.4 before this spec was written

No claim here is inferred. The last two phases each shipped a wrong assertion
about unexecuted behaviour; this section is the countermeasure.

| Question | Executed result |
|---|---|
| Is `bm25()` negative, and which end is best? | Negative. `strong=-0.000001812` vs `weak=-0.000000798` for the same term — **more negative is better**, so `ORDER BY bm25(memories)` (ascending) is best-first |
| Does `MATCH 'runtime-layout'` work? | **No** — `Error: no such column: layout` |
| `MATCH 'foo:bar'`? | **No** — `Error: no such column: foo` |
| `MATCH '*'`? | **No** — `Error: unknown special query` |
| `MATCH 'a AND b'`? | No error, but parsed as a **boolean operator**, not text |
| Does double-quoting fix all of those? | **Yes** — every one returns a row count with no error |
| Is an `UNINDEXED` column filterable in `WHERE`? | Yes (phase 06 verified; re-confirmed here with `namespace IN (…)`) |
| Fresh install, `reconcile_index()` row count | **9** (7 knowledge + 2 session; no agent memory dirs exist, none expired) |

### The trap: double-quoting the *whole* query makes search useless

Quoting turns the expression into a **phrase** match. The caller passes an
entire user turn, so the naive fix fails completely — executed:

```
query: how do I tune shared_buffers for postgres?
  MATCH '"how do I tune shared_buffers for postgres?"'   -> 0 rows
```

Zero, against a memory whose body is literally
`increase shared_buffers when the working set grows`. **Per-term** quoting
joined with `OR` finds it:

```
  MATCH '"how" OR "do" OR "tune" OR "shared_buffers" OR "postgres"'  -> 1 row
```

**And ranking absorbs the noise that `OR` lets in.** Two documents, one
relevant and one containing only the stopword-ish `how`/`do`:

```
relevant  -0.000003162   <- tune / shared_buffers / postgres all hit
noise     -0.000001903   <- only "how" and "do" hit
```

Both match; the relevant one ranks first and `LIMIT` keeps it. That is why the
spec does **not** ask for a stopword list — `bm25` is the mechanism.

One more executed detail: `unicode61` treats `_` as a separator, so
`"shared_buffers"` becomes a two-token phrase and matches the document either
way. Do not special-case underscores.

## Spec

### 1. Change the signature so namespace filtering happens in SQL

```rust
/// Search the FTS5 index. Returns up to `limit` hits as
/// `(namespace, key, bm25_score)`, best match first.
///
/// Best-effort: any failure returns an empty `Vec` after logging. The index is
/// a derived cache and search degrading to "no hits" must never be fatal.
pub fn fts5_search(
    query: &str,
    limit: usize,
    namespaces: &[&str],
) -> Vec<(String, String, f64)>
```

Returning the namespace alongside the key is what lets the caller disambiguate;
see task 5.

### 2. Reconcile an empty index on first search

This is the fix for the gap phase 07 recorded: seeded memories are written by
`seed_memory_inner` with a direct `fs::write` (`src/config/seeds.rs:80`), which
bypasses `add_memory` and therefore the index hook. **A fresh install has zero
indexed rows**, so without this the milestone ships a search that cannot find
its own seed data.

After opening the connection, before querying:

```rust
let count: i64 = conn
    .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
    .unwrap_or(0);
if count == 0 {
    if let Err(e) = reconcile_index() {
        log::warn!("memory index reconcile failed: {e:#}");
    }
    // fall through and query anyway — an empty result is still a valid answer
}
```

**Use the row count, not a `std::sync::Once` or any other process-global
latch.** Tests repoint `HOME` between cases, so a once-per-process guard would
fire in whichever test ran first and leave every later one with an unreconciled
index — the same trap that rules out a cached `Connection` (phase 07, task 2).
Row-count triggering is stateless and therefore test-safe.

Re-open the connection after reconciling, or call `reconcile_index()` before
opening your own — do not hold a connection across it.

### 3. Build the MATCH expression — the load-bearing part

A free function, so the tests can drive it directly:

```rust
/// Turn arbitrary user text into a safe FTS5 MATCH expression.
/// Returns `None` when the input yields no usable terms.
fn build_match_expr(query: &str) -> Option<String>
```

Rules, in order:

1. Split `query` on whitespace.
2. Drop any token containing **no** alphanumeric character (`?`, `--`, `:::`).
3. Escape the token for a quoted FTS5 string: replace each `"` with `""`.
4. Wrap the result in double quotes.
5. Deduplicate **case-insensitively**, preserving first-seen order.
6. Keep at most **32** terms (a long turn otherwise builds a huge expression;
   truncation is fine because `bm25` ranks what remains).
7. Join with `" OR "`. If no terms survive, return `None`.

`fts5_search` returns an empty `Vec` immediately when this returns `None` — do
not run a query with an empty MATCH.

Worked example, matching the executed evidence above:

```
input:  "how do I tune shared_buffers for postgres?"
output: Some("\"how\" OR \"do\" OR \"I\" OR \"tune\" OR \"shared_buffers\" OR \"for\" OR \"postgres\"")
```

Note `postgres?` keeps its `?` inside the quotes — that is fine and intended,
because the tokenizer discards punctuation. **Do not strip punctuation from
inside a token**; quoting already makes it safe, and stripping would break
`shared_buffers`.

### 4. The query

The namespace list is dynamic, so the `IN` placeholders must be built at
runtime. **This exact shape was compiled and run before this spec was written**
— it returned `[("global", "k", -1e-6)]`:

```rust
let placeholders = (0..namespaces.len())
    .map(|i| format!("?{}", i + 3))
    .collect::<Vec<_>>()
    .join(",");
let sql = format!(
    "SELECT namespace, key, bm25(memories) FROM memories
     WHERE memories MATCH ?1 AND namespace IN ({placeholders})
     ORDER BY bm25(memories) LIMIT ?2"
);
let mut params: Vec<Box<dyn rusqlite::ToSql>> =
    vec![Box::new(expr), Box::new(limit as i64)];
for ns in namespaces {
    params.push(Box::new(ns.to_string()));
}
let mut stmt = conn.prepare(&sql)?;
let rows = stmt.query_map(
    rusqlite::params_from_iter(params.iter().map(|b| &**b)),
    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
)?;
```

`ORDER BY bm25(memories)` **ascending** — best first, per the executed evidence.
Do not add `DESC`.

An empty `namespaces` slice would produce `IN ()`, which is a syntax error.
Return an empty `Vec` before building the query when `namespaces.is_empty()`.

### 5. Update the caller

In `src/daemon/memory_prompt.rs`, pass `namespaces` through and match on both
fields:

```rust
let results = index::fts5_search(query, limit, namespaces);
let all_memories = list_memories_with_tags(None, namespaces).unwrap_or_default();
let mut found = Vec::new();
for (namespace, key, _score) in results {
    if let Some(info) = all_memories
        .iter()
        .find(|m| m.key == key && m.namespace == namespace)
    {
        found.push(info.clone());
    }
}
found
```

The loop already preserves `fts5_search`'s order, so rank order survives into
the returned `Vec<MemoryInfo>`. Keep that property — a test pins it.

### 6. Remove the two `#[allow(dead_code)]`

Task 2 gives `reconcile_index()` a production caller and constructs
`ReconcileReport`, so both attributes phase 07 added come off.
`grep -c 'allow(dead_code)' src/memory/index.rs` must return **0**.

**This is the whole lint story for this phase — there is nothing left
deliberately unused.** If a `dead_code` error appears anyway, something the spec
asked for is genuinely unwired; fix the wiring and do not add an attribute.

### 7. Tests

Add to the existing `#[cfg(test)] mod tests` in `src/memory/index.rs` (the last
two in `src/daemon/memory_prompt.rs` if that is more natural). Tests touching
`HOME` take `crate::test_home_guard()` **before** `set_var` and use
`tempfile::tempdir()`:

```rust
let _guard = crate::test_home_guard();
let tmp = tempfile::tempdir().unwrap();
unsafe { std::env::set_var("HOME", tmp.path()) };
```

Name them exactly:

- `search_finds_text_hit_when_tags_miss` — **the milestone exit criterion.**
  Add a memory whose *body* contains a distinctive word and whose *tags* do not
  mention it at all, then search for that word and assert the memory comes back.
  This is the case that cannot work today.
- `search_ranks_better_match_first` — index two memories, one where the term
  dominates a short body and one where it is buried in filler; assert the
  strong one is returned **first**. Assert on order, not just membership.
- `hyphenated_query_does_not_error` — search `"runtime-layout"`. Must return
  normally (any row count) rather than erroring. Without the quoting this raises
  *"no such column: layout"*.
- `operator_words_are_treated_as_text` — search `a AND b`. Must return normally
  and must **not** be interpreted as a boolean expression.
- `empty_query_returns_no_hits` — `""` and `"   ?  "` both yield an empty `Vec`
  and run no query.
- `namespace_filter_excludes_other_namespaces` — same distinctive body text in
  `global` and in `agent-x`; searching with `["global"]` returns exactly the
  global row.
- `fresh_index_is_reconciled_on_first_search` — seed a temp `HOME` with
  `Config::ensure_dirs()` and **do not add any memory**. Search for a word
  present in a seeded knowledge memory and assert a hit. Then assert the index
  holds **9** rows — the executed fresh-install count. This is the test that
  stops M7 shipping a search that cannot find its own seed data.
- `ftsearch_memories_preserves_rank_order` — drive the public
  `ftsearch_memories` and assert the returned `MemoryInfo` order matches the
  score order, not filesystem/`list_memories_with_tags` order.

## Acceptance criteria

- [ ] `fts5_search` returns real BM25-ranked results; `ftsearch_memories` passes
      `namespaces` through and matches on `(namespace, key)`.
- [ ] All eight tests named in spec task 7 pass.
- [ ] `search_finds_text_hit_when_tags_miss` passes — the milestone's first exit
      criterion.
- [ ] `fresh_index_is_reconciled_on_first_search` passes and asserts **9** rows.
- [ ] `grep -c 'allow(dead_code)' src/memory/index.rs` returns **0**, and
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] A hyphenated query does not raise *"no such column"* — pinned by
      `hyphenated_query_does_not_error`.
- [ ] `cargo build` zero new warnings; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes. Lib count rises by the number of tests added (8 by
      this spec, so **1031** — the baseline is **1023**); integration stays
      **30** (2 ignored), isolation **8** (1 ignored), `bug_tracker` **6**.
- [ ] Only `src/memory/index.rs` and `src/daemon/memory_prompt.rs` change.

## Test plan

Covered by spec task 7. Three tests carry the phase:

`search_finds_text_hit_when_tags_miss` is the milestone exit criterion written
as code — a memory whose text matches and whose tags do not is precisely what
the tag-based path cannot surface.

`fresh_index_is_reconciled_on_first_search` is what makes the feature true on a
machine nobody has used yet. Without it every other test can pass while a real
first-run install returns nothing.

`search_ranks_better_match_first` is the only test that proves `bm25` is
actually wired. A search that returned all matches in arbitrary order would
satisfy every membership assertion in this phase.

**What would make this phase a false success:** a `fts5_search` that returns
matches in insertion order with a constant score. Every membership test would
pass. The ranking test is the guard, which is why it must assert **order**, not
"contains".

A second, quieter one: quoting the whole query instead of per-term. That
compiles, raises no errors, passes `hyphenated_query_does_not_error` and
`empty_query_returns_no_hits` — and returns zero hits for every realistic user
turn. `search_finds_text_hit_when_tags_miss` and
`fresh_index_is_reconciled_on_first_search` both use multi-word queries and are
what catch it.

## End-to-end verification

Run this block verbatim and paste the resulting file into your Update Log.

**Two constraints carried from phase-03's post-mortem:** **no heredocs**, and
every tree-walking command wrapped in `timeout`. A phase-03 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
cargo build 2>&1 | tail -2
H=$(mktemp -d)
{
  echo "=== a fresh install has memories on disk and no index rows yet ==="
  HOME="$H" timeout 120 ./target/debug/daemoneye setup 2>&1 | tail -1
  timeout 30 ls -1 "$H/.daemoneye/memory/knowledge" | wc -l
  echo "knowledge-file-count-above-should-be-7"
  timeout 30 ls "$H/.daemoneye/var/index/memory.db" 2>&1 | tail -1

  echo "=== the search tests ==="
  timeout 900 cargo test --lib memory::index 2>&1 | grep -E "^test |^test result"

  echo "=== no allow(dead_code) remains ==="
  timeout 30 grep -c 'allow(dead_code)' src/memory/index.rs
  echo "count-above-must-be-0"

  echo "=== audit still clean ==="
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts > /dev/null 2>&1
  echo "clean-audit-exit=$?   # 0 == PASS"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/phase08-e2e.txt 2>&1
rm -rf "$H"
cat /tmp/phase08-e2e.txt
```

The `.db` file is **not** expected to exist after `setup` alone — `setup` adds no
memories and nothing calls `open_index()` during it. That is the gap task 2
closes at first search, and `fresh_index_is_reconciled_on_first_search` is what
proves it closed.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it shows that
build/lint/test ran, not that this phase's acceptance criteria were exercised.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**. `rusqlite`, `anyhow` and `log` are already
      present.
- [ ] May touch `docs/architecture.md`: **no.** Its § 5 stub note and
      `CLAUDE.md`'s `src/memory/index.rs` row are **phase 09's** to rewrite once
      this lands.
- [ ] May touch `CLAUDE.md`: **no.**
- [ ] May create new files: no.

## Out of scope

- **`docs/architecture.md` § 5 and the `CLAUDE.md` index row.** Both still
  describe a stub. They become wrong the moment this phase lands, and phase 09
  exists to fix them — do not pre-empt it.
- **A `reindex` CLI subcommand or a startup reconcile.** Task 2's
  reconcile-on-empty is deliberately the minimum that makes search correct. An
  operator-facing command is a separate feature.
- **Stopword lists, synonyms, fuzzy matching, or query expansion.** `bm25`
  ranking is the precision mechanism for this phase; the executed evidence above
  shows it separates a relevant document from a stopword-only match.
- **Changing the schema or `SCHEMA_VERSION`.** The current columns support
  everything here. (If one were needed, `ensure_schema` drops and recreates on a
  version mismatch, so a bump is cheap — but nothing in this phase needs it.)
- **Re-ranking or merging inside `memory_prompt.rs` beyond the caller change in
  task 5.** The candidate-merging logic above line 73 is not this phase's.
- **`src/search.rs`.** The grep scan stays exactly as it is; this phase does not
  remove or reroute it.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-01 22:08 (started)

**Executor:** Claude (Sonnet 4.5)

Implementing FTS5 search: `build_match_expr()`, namespace-filtered `fts5_search()`, reconcile-on-empty, caller update in `memory_prompt.rs`, and 8 tests.

### Update — 2026-08-01 22:15 (end-to-end verification)

**End-to-end verification transcript:**

```
=== a fresh install has memories on disk and no index rows yet ===
# will attach to it when run from outside tmux.
7
knowledge-file-count-above-should-be-7
ls: cannot access '/tmp/tmp.NiTd0JC7Hi/.daemoneye/var/index/memory.db': No such file or directory

=== the search tests ===
test memory::index::tests::stale_schema_version_is_recreated ... ok
test memory::index::tests::fts5_is_available_and_matches ... ok
test memory::index::tests::unindexed_columns_filter_but_do_not_match ... ok
test memory::index::tests::add_memory_indexes_the_row ... ok
test memory::index::tests::delete_memory_removes_the_row ... ok
test memory::index::tests::fresh_index_is_reconciled_on_first_search ... ok
test memory::index::tests::expired_memory_is_not_indexed ... ok
test memory::index::tests::empty_query_returns_no_hits ... ok
test memory::index::tests::ftsearch_memories_preserves_rank_order ... ok
test memory::index::tests::hyphenated_query_does_not_error ... ok
test memory::index::tests::index_failure_does_not_fail_add_memory ... ok
test memory::index::tests::open_index_creates_database_and_schema ... ok
test memory::index::tests::namespace_filter_excludes_other_namespaces ... ok
test memory::index::tests::open_index_is_idempotent ... ok
test memory::index::tests::open_index_sets_schema_version ... ok
test memory::index::tests::operator_words_are_treated_as_text ... ok
test memory::index::tests::reconcile_after_incremental_writes_is_a_no_op ... ok
test memory::index::tests::reconcile_rebuilds_from_disk ... ok
test memory::index::tests::same_key_in_two_namespaces_is_two_rows ... ok
test memory::index::tests::search_finds_text_hit_when_tags_miss ... ok
test memory::index::tests::search_ranks_better_match_first ... ok
test memory::index::tests::update_memory_replaces_the_row_not_duplicates_it ... ok
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1009 filtered out; finished in 0.02s

=== no allow(dead_code) remains ===
0
count-above-must-be-0

=== audit still clean ===
clean-audit-exit=0   # 0 == PASS

=== full gate ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy-exit=0
test result: ok. 1031 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.46s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
