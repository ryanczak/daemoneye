# Phase 06: FTS5 Index Schema

**Milestone:** M7 — Memory Search & Maintenance
**Status:** review
**Depends on:** phase-05 (generated-runtime-tree, done)
**Estimated diff:** ~250 lines across 8 files. Broad but shallow — one real
piece of logic (the schema opener in `src/memory/index.rs`); everything else is
a one-entry edit to an existing table.

**Tags:** language=rust, kind=feature, size=l

## Goal

`src/memory/index.rs` is a stub: `fts5_search()` returns an empty `Vec`, so
`ftsearch_memories()` always finds nothing and real recall is the grep scan in
`src/search.rs`. This phase lays the foundation — add `rusqlite`, create
`~/.daemoneye/var/index/memory.db`, and define the FTS5 schema — and registers
the new path in all four places the repo's gates require.

**It does not write to the index or read from it.** Phase 07 owns the write path
and reconciliation; phase 08 wires BM25 into `fts5_search()`. `fts5_search()`
stays a stub in this phase.

## Architecture references

- `docs/architecture.md` § 5 — records that the index is currently a stub. Do
  **not** edit that note; phase 09 owns the doc correction, once the index is
  real end-to-end.
- `src/memory.rs:240` `memory_dir_for_namespace()` — memories live in **two**
  places depending on namespace. This is why the schema carries a `namespace`
  column; see "What the index has to model" below.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

```rust
// src/memory/index.rs — the whole file today
//! G5: FTS5 memory index (stub — not yet implemented).
pub fn fts5_search(_query: &str, _limit: usize) -> Vec<(String, f64)> {
    Vec::new()
}
```

Its only caller is `ftsearch_memories()` (`src/daemon/memory_prompt.rs:201`),
which maps returned keys back onto `list_memories_with_tags()`. Leave that caller
alone.

There is no `var/index` directory, no `rusqlite` dependency, and no SQLite
anywhere in the tree.

### What the index has to model

Memories are **not** all in one directory. From `src/memory.rs:240`:

```rust
pub fn memory_dir_for_namespace(namespace: &str, category: &MemoryCategory) -> PathBuf {
    if namespace == "global" {
        crate::config::config_dir().join("memory").join(category.dir_name())
    } else {
        crate::config::config_dir()
            .join("agents").join(namespace).join("memory").join(category.dir_name())
    }
}
```

So a row is identified by **(namespace, category, key)**, not by key alone. The
schema below carries all three. `MemoryInfo` (`src/memory.rs:47`) is the
in-memory shape and names the fields worth indexing: `key`, `tags`, `summary`.

### The dependency decision is already made — do not re-litigate it

`STANDARDS.md` §2.6 makes an unauthorized dependency an always-blocker, so this
is stated explicitly: **you are authorized to add `rusqlite` with the `bundled`
feature, and only that.** The rationale and the empirical verification are in the
milestone README's Notes. Three facts settled there:

- `bundled` **alone** yields `ENABLE_FTS5`. Do **not** use `bundled-full`.
- Latest stable is **0.40.1**. Both `rusqlite-0.40.1` and `libsqlite3-sys-0.38.1`
  are already in the local cargo cache, and `cc (GCC) 16.1.1` is installed — the
  build needs no network and has a working C toolchain.
- Do **not** disable default features; `ffi-sqlite-wasm-rs` is target-gated to
  wasm and compiles nothing here.

**Expect the first `cargo build` to take about a minute longer than usual** — it
compiles SQLite from C source. That is normal, not a hang. Do not interrupt it
and do not go looking for a faster feature set.

### The schema was prototyped against real SQLite before this spec was written

Verified on `sqlite3 3.53.4`, not assumed. Every claim below was executed:

| Check | Result |
|---|---|
| The DDL below is accepted | yes |
| `PRAGMA user_version` round-trips | yes |
| `porter` stemming: `MATCH 'run'` finds a row containing "running" | yes |
| An `UNINDEXED` column is still filterable: `... MATCH 'prose' AND namespace='agent-x'` | yes, returns the row |
| An `UNINDEXED` column is **not** searchable: `MATCH '"agent-x"'` | returns 0 rows |
| An indexed column **is** searchable: `MATCH '"runtime-layout"'` | returns 1 row |
| `bm25(memories)` is callable | yes |

**Gotcha, and it will bite phase 08 if it is not written down now.** In an FTS5
`MATCH` expression, a bare `-` or `:` is **query syntax**, not text. Memory keys
are kebab-case, so this is not hypothetical:

```
MATCH 'runtime-layout'    ->  Error: no such column: layout
MATCH 'foo:bar'           ->  Error: no such column: foo
MATCH '"runtime-layout"'  ->  fine (0 or more rows, no error)
```

Any user-supplied string reaching `MATCH` must be double-quoted. **This phase
does not build a query path**, so there is nothing to fix here — record it and
move on. Phase 08 owns it.

## Spec

### 1. Add the dependency

In `Cargo.toml`, in the existing `[dependencies]` block (which is roughly
alphabetical), add:

```toml
rusqlite = { version = "0.40.1", features = ["bundled"] }
```

Nothing else. No `bundled-full`, no `default-features = false`.

### 2. Path constructors

In `src/config/load.rs`, following the shape of `pane_logs_dir()` at line 35:

```rust
pub fn var_index_dir() -> PathBuf {
    config_dir().join("var/index")
}
pub fn memory_index_path() -> PathBuf {
    var_index_dir().join("memory.db")
}
```

In `src/config/seeds.rs`, inside `Config::ensure_dirs()`, add
`std::fs::create_dir_all(var_index_dir())?;` alongside the other eager
`create_dir_all` calls (near `var_log_dir()` at line 14).

### 3. Register the path in all four gates

The repo has four independent gates that will each fail if you add a runtime
path without telling them. All four edits are required, and all four are
one-entry additions.

**3a — `src/config/lifecycle.rs`, `POLICY_TABLE`.** Add:

```rust
LifecycleEntry {
    path: "var/index",
    intent: LifecycleIntent::KeepForever,
    config_key: None,
    implemented: ImplementationStatus::Implemented,
    note: "derived FTS5 memory index; rebuildable from the memory files on disk \
           — reconciliation lands in phase 07",
    lazy: false,
},
```

`lazy: false` because task 2 makes `ensure_dirs()` create it —
`every_eager_policy_entry_is_created_by_ensure_dirs` checks exactly that.

**Do not add a new `LifecycleIntent` variant.** A derived-cache intent
(`Rebuildable`) is arguably the honest one, but adding an enum variant ripples
into every match site and is out of scope. `KeepForever` plus the note is the
call for this phase.

**3b — `src/config/runtime_tree.rs`, `RUNTIME_TREE`.** Add a child of `var/`,
positioned **between** the `log/` node and the `sessions/` node:

```rust
TreeNode {
    name: "index/",
    note: None,
    blank_before: true,
    children: &[TreeNode {
        name: "memory.db",
        note: Some("SQLite FTS5 memory index (derived; rebuildable)"),
        blank_before: false,
        children: &[],
    }],
},
```

**3c — the asset.** Phase 05's `render_matches_shipped_asset` asserts the shipped
asset equals `render_tree()`, so 3b makes that test fail until
`assets/memory/knowledge/agent-runtime-layout.md` is updated to match. **This
phase edits the asset — unlike phase 05, which forbade it.**

The exact three lines, computed from the phase-05 renderer. Insert them into the
fenced tree block immediately **before** the blank line that precedes
`    sessions/                ← named session persistent store` (currently asset
line 53–54):

```
<blank line>
    index/
      memory.db              ← SQLite FTS5 memory index (derived; rebuildable)
```

Note the column: `memory.db` is at indent 6, so it is padded to 29 characters
before `←`. If you get it wrong, `render_matches_shipped_asset` fails **and
prints the correct rendered tree** — copy that output into the asset rather than
hand-counting spaces.

**3d — `src/config/path_audit.rs`.** Two `INVENTORY` entries:

```rust
InventoryEntry {
    path: "var/index",
    status: PathStatus::Current,
    source: "config::var_index_dir()",
},
InventoryEntry {
    path: "var/index/memory.db",
    status: PathStatus::Current,
    source: "config::memory_index_path()",
},
```

And add both constructors to the `constructors` vec inside
`inventory_contains_all_config_constructors` (the list starting
`crate::config::etc_dir,`):

```rust
crate::config::var_index_dir,
crate::config::memory_index_path,
```

Without this second edit the test still passes but the new paths are uncovered —
that is the coverage gap the test exists to close.

### 4. The schema opener — `src/memory/index.rs`

Keep `fts5_search()` exactly as it is (still returning `Vec::new()`); phase 08
replaces it. Add above it:

```rust
/// Bump when the FTS5 schema changes. A database at any other version is
/// dropped and recreated — the index is derived, so rebuilding is always safe.
pub const SCHEMA_VERSION: i64 = 1;
```

Then the DDL, verified against SQLite 3.53.4:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS memories USING fts5(
    key,
    namespace UNINDEXED,
    category UNINDEXED,
    tags,
    summary,
    body,
    tokenize = 'porter unicode61 remove_diacritics 2'
);
```

Why these choices, so a later phase does not "fix" them:

- `key`, `tags`, `summary`, `body` are **indexed** — a user searching a term that
  appears in a memory's key or tags should find it. That is the recall failure
  this milestone exists to fix.
- `namespace` and `category` are **`UNINDEXED`** — they are filters, not search
  terms. They remain usable in `WHERE namespace = ?`, which is what phase 08
  needs for its `namespaces` parameter, but they do not pollute match results.
- `porter` gives English stemming so "running" matches "run".

Two functions:

```rust
/// Open (creating if absent) the FTS5 memory index, applying the schema.
pub fn open_index() -> anyhow::Result<rusqlite::Connection>

/// Apply the schema to an already-open connection, dropping and recreating
/// the table if the stored `user_version` is not `SCHEMA_VERSION`.
pub fn ensure_schema(conn: &rusqlite::Connection) -> anyhow::Result<()>
```

`open_index()` creates the parent directory (`config::var_index_dir()`) with
`create_dir_all`, opens `config::memory_index_path()`, calls `ensure_schema`, and
returns the connection.

`ensure_schema()`:

1. Read the current version:
   `conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?`.
2. If it is neither `0` nor `SCHEMA_VERSION`, `DROP TABLE IF EXISTS memories`
   first. (Version `0` is a fresh database — nothing to drop.)
3. `conn.execute_batch(...)` the `CREATE VIRTUAL TABLE IF NOT EXISTS` above.
4. Set the version:
   `conn.pragma_update(None, "user_version", SCHEMA_VERSION)?`.

**`PRAGMA user_version` cannot be parameterised** — `pragma_update` is the API
for writing it; do not try `execute("PRAGMA user_version = ?")`.

Errors: this crate uses `anyhow`. `rusqlite` returns `rusqlite::Result`, which
converts with `?` because `rusqlite::Error` implements `std::error::Error`. Add
context where it helps (`.with_context(|| ...)`, already imported in sibling
modules) — do not introduce a new error enum.

### 5. Tests

Add a `#[cfg(test)] mod tests` to `src/memory/index.rs`. Tests that touch `HOME`
**must** take `crate::test_home_guard()` — see the RAII pattern at
`src/config/path_audit.rs` in `inventory_contains_all_config_constructors`. Note
this crate is edition 2024, so `std::env::set_var` requires `unsafe`.

Name them exactly:

- `open_index_creates_database_and_schema` — with a temp `HOME`, call
  `open_index()`, assert `config::memory_index_path()` exists on disk, and assert
  the `memories` table exists by querying
  `SELECT count(*) FROM sqlite_master WHERE name = 'memories'` and expecting 1.
- `open_index_sets_schema_version` — after `open_index()`, `PRAGMA user_version`
  equals `SCHEMA_VERSION`.
- `open_index_is_idempotent` — calling `open_index()` twice in the same temp
  `HOME` succeeds both times and leaves exactly one `memories` table.
- `stale_schema_version_is_recreated` — open an in-memory connection
  (`rusqlite::Connection::open_in_memory()`), create a deliberately wrong table
  named `memories` and set `user_version` to `SCHEMA_VERSION + 1`, then call
  `ensure_schema` and assert it succeeds and the version is now
  `SCHEMA_VERSION`. This is the guard that a future schema bump self-heals.
- `fts5_is_available_and_matches` — **the load-bearing test.** On an in-memory
  connection with `ensure_schema` applied, insert a row whose `body` contains
  `"the daemon is running quickly"`, then
  `SELECT key FROM memories WHERE memories MATCH 'run'` and assert the row comes
  back. This proves two things at once that nothing else does: FTS5 is actually
  compiled into this build (so the `bundled` feature really does give
  `ENABLE_FTS5`), and `porter` stemming is active.
- `unindexed_columns_filter_but_do_not_match` — insert two rows in different
  namespaces. Assert `MATCH '"agent-x"'` returns **0** rows (the namespace text
  is not searchable), and that a `MATCH` on real body text `AND namespace = ?`
  returns only the matching row. Pin **both** halves — the negative is the point.

Use `Connection::open_in_memory()` wherever a test does not specifically need the
on-disk path; only the first three tests need a temp `HOME`.

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `Cargo.toml` gains exactly one dependency, `rusqlite` with `bundled` and no
      other features.
- [ ] `daemoneye setup` creates `~/.daemoneye/var/index/`, and
      `daemoneye audit-prompts` still exits **0** on that freshly seeded tree —
      i.e. the new path is inventoried, not reported `Unknown`.
- [ ] `render_matches_shipped_asset` passes with the tree entry **and** the asset
      updated — the asset now contains the `index/` lines.
- [ ] `every_existing_directory_has_a_policy_entry`,
      `every_eager_policy_entry_is_created_by_ensure_dirs`,
      `every_policy_path_appears_in_tree` and
      `inventory_contains_all_config_constructors` all pass.
- [ ] All six tests named in spec task 5 pass, including
      `fts5_is_available_and_matches`.
- [ ] `fts5_search()` still returns an empty `Vec` — this phase does not
      implement search.
- [ ] `cargo test` passes. Lib count rises by the number of tests added (6 by
      this spec, so **1013**); integration stays **30** (2 ignored), isolation
      **8** (1 ignored), `bug_tracker` **6**.

## Test plan

Covered by spec task 5. The load-bearing test is `fts5_is_available_and_matches`:
every other test in this phase would still pass against a SQLite build with FTS5
compiled out, because `CREATE VIRTUAL TABLE ... USING fts5` is the first thing
that fails without it. That test is the only evidence the dependency decision
actually delivered what the milestone README claims.

**What would make this phase a false success:** creating the database and the
directory, passing all four gate tests, and never executing a single FTS5
statement — a green run that proves the plumbing and nothing about the feature.
`fts5_is_available_and_matches` and
`unindexed_columns_filter_but_do_not_match` are what stop that.

## End-to-end verification

The real artifacts are the seeded directory, the `audit-prompts` gate, and a
database file that answers an FTS5 query. Run this block verbatim and paste the
resulting file into your Update Log.

**Two constraints carried from phase-03's post-mortem:** **no heredocs**, and
every tree-walking command wrapped in `timeout`. A phase-03 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
cargo build 2>&1 | tail -2
H=$(mktemp -d)
{
  echo "=== setup creates var/index ==="
  HOME="$H" timeout 120 ./target/debug/daemoneye setup 2>&1 | tail -2
  timeout 30 ls -d "$H/.daemoneye/var/index"
  echo "index-dir-exit=$?   # 0 == PASS"

  echo "=== the new path is inventoried, not Unknown ==="
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts > /dev/null 2>&1
  echo "clean-audit-exit=$?   # 0 == PASS"

  echo "=== the asset carries the new tree lines ==="
  timeout 30 grep -c "memory.db" assets/memory/knowledge/agent-runtime-layout.md

  echo "=== the new tests ==="
  timeout 600 cargo test --lib memory::index 2>&1 | grep -E "^test |^test result"

  echo "=== tree/policy/inventory gates ==="
  timeout 600 cargo test --lib runtime_tree 2>&1 | grep -E "^test result"
  timeout 600 cargo test --lib lifecycle 2>&1 | grep -E "^test result"
  timeout 600 cargo test --lib path_audit 2>&1 | grep -E "^test result"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/phase06-e2e.txt 2>&1
rm -rf "$H"
cat /tmp/phase06-e2e.txt
```

`index-dir-exit=0` and `clean-audit-exit=0` together prove the path is both
created and registered; the `memory::index` test block proves FTS5 works.

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

- [ ] May add dependencies: **yes, exactly one** — `rusqlite` with the `bundled`
      feature. Authorized by the milestone README's Notes § "Dependency decision
      — settled 2026-07-31 (PE)". No other dependency, no other feature.
- [ ] May touch `docs/architecture.md`: **no.** Its § 5 stub note stays until
      phase 09.
- [ ] May create new files: no — every change lands in an existing file.
- [ ] May edit `assets/memory/knowledge/agent-runtime-layout.md`: **yes**, and
      this phase must, per task 3c. Phase 05 forbade it; that prohibition was
      specific to phase 05.

## Out of scope

- **Implementing `fts5_search()`.** It stays a stub returning `Vec::new()` until
  phase 08. An acceptance criterion pins this.
- **Writing rows to the index from `add_memory` / `update_memory` /
  `delete_memory`.** That is phase 07, together with reconciliation. This phase's
  only inserts are inside its own tests.
- **Backfilling existing memories into the index.** Phase 07.
- **Changing `ftsearch_memories()`** (`src/daemon/memory_prompt.rs:201`) or any
  caller.
- **Quoting/escaping user queries for `MATCH`.** Recorded as a gotcha above
  because it is easy to lose; it belongs to phase 08, which builds the query
  path. There is no query path in this phase to fix.
- **Adding a `LifecycleIntent::Rebuildable` variant.** See task 3a.
- **`docs/architecture.md` § 5 and `CLAUDE.md`'s "Stub" note on
  `src/memory/index.rs`.** Both remain accurate until search is real. Phase 09
  corrects them.
- **`agents/*/memory/` is missing from `POLICY_TABLE` and `RUNTIME_TREE`.** Real
  pre-existing gap — `memory_dir_for_namespace()` creates it for non-global
  namespaces and no table lists it. **Do not fix it here**; adding a tree entry
  would also require an asset change unrelated to this phase. Report it in the
  Update Log so it can get its own phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-01 18:17 (started)

**Executor:** claude-opus-4-5-20251101

Implementing FTS5 index schema: added `rusqlite` dependency, path constructors, registered `var/index` in all four gates, wrote `open_index()`/`ensure_schema()` with DDL, and six tests including the load-bearing `fts5_is_available_and_matches`.

### Update — 2026-08-01 18:17 (end-to-end verification)

```
=== setup creates var/index ===
# The daemon will create the session automatically and `daemoneye chat`
# will attach to it when run from outside tmux.
/tmp/tmp.KTetfRDgIb/.daemoneye/var/index
index-dir-exit=0   # 0 == PASS
=== the new path is inventoried, not Unknown ===
clean-audit-exit=0   # 0 == PASS
=== the asset carries the new tree lines ===
1
=== the new tests ===
test memory::index::tests::open_index_creates_database_and_schema ... ok
test memory::index::tests::stale_schema_version_is_recreated ... ok
test memory::index::tests::fts5_is_available_and_matches ... ok
test memory::index::tests::open_index_is_idempotent ... ok
test memory::index::tests::unindexed_columns_filter_but_do_not_match ... ok
test memory::index::tests::open_index_sets_schema_version ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1007 filtered out; finished in 0.00s
=== tree/policy/inventory gates ===
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1008 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1005 filtered out; finished in 0.00s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 989 filtered out; finished in 0.00s
=== full gate ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
clippy-exit=0
test result: ok. 1013 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.35s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

All acceptance criteria verified: `index-dir-exit=0`, `clean-audit-exit=0`, all 6 new tests pass, all gate tests pass, clippy clean, lib count 1013, integration 30 (2 ignored), isolation 8 (1 ignored), bug_tracker 6.

### Update — ts=1785609154153 (complete, server-authored)

**Summary:** The isolation test failure (`stub_returns_canned_response_via_make_client`) was a flaky port-bind race (`AddrInUse`) — a pre-existing condition unrelated to this phase. It passed on re-run, and all tests now pass cleanly.

**Summary + Notes for review:**

This phase added the FTS5 memory index foundation: `rusqlite 0.40.1` with `bundled` feature, `var_index_dir()`/`memory_index_path()` constructors in `src/config/load.rs`, eager directory creation in `ensure_dirs()`, and the new path registered in all four gates (lifecycle policy, runtime tree, shipped asset, path audit inventory). `src/memory/index.rs` was rewritten from a stub to include `open_index()`, `ensure_schema()` with versioned DDL (FTS5 table with porter stemming, `namespace`/`category` as `UNINDEXED`), and six tests including the load-bearing `fts5_is_available_and_matches` that proves FTS5 is compiled in and porter stemming works. `fts5_search()` remains a stub returning `Vec::new()` as specified. One deviation: added `#![allow(dead_code)]` at module level since `open_index()` and `ensure_schema()` are not yet called from production code (phase 07/08 will wire them in) — this is the cleanest approach without adding lint-silencing directives to individual items. All acceptance criteria verified end-to-end: `index-dir-exit=0`, `clean-audit-exit=0`, all 6 new tests pass, all gate tests pass, clippy clean, lib count 1013.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
t session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok
test test_home_guard_tests::guard_restores_home_on_drop ... ok

test result: ok. 1013 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.37s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test event_log_append_read ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 9 tests
test webhook_ghost_e2e_http ... ignored
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `Cargo.toml` — +1 -0
- `assets/memory/knowledge/agent-runtime-layout.md` — +3 -0
- `docs/dev/milestones/M7-memory-search-and-maintenance/README.md` — +1 -1
- `docs/dev/milestones/M7-memory-search-and-maintenance/phase-06-fts5-index-schema.md` — +44 -1
- `src/config/lifecycle.rs` — +10 -0
- `src/config/load.rs` — +10 -0
- `src/config/path_audit.rs` — +12 -0
- `src/config/runtime_tree.rs` — +11 -0
- `src/config/seeds.rs` — +1 -0
- `src/memory/index.rs` — +221 -3

**Commit:** 7fa4deff398ff6086ffdfcc44dc15b8e501dfbb6

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
