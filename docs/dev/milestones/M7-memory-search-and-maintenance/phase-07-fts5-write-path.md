# Phase 07: FTS5 Write Path

**Milestone:** M7 — Memory Search & Maintenance
**Status:** todo
**Depends on:** phase-06 (fts5-index-schema, done)
**Estimated diff:** ~280 lines — most of it in `src/memory/index.rs`, plus three
small hook sites in `src/memory.rs`.

**Tags:** language=rust, kind=feature, size=l

## Goal

Phase 06 built the index and nothing writes to it. Make `add_memory`,
`update_memory` and `delete_memory` keep `var/index/memory.db` in step with the
files on disk, and add a **reconciliation** path that rebuilds the index from
those files — both as the repair mechanism and as the thing that proves the
incremental hooks are correct.

`fts5_search()` stays a stub. Phase 08 wires the query path.

## Architecture references

- `src/memory/index.rs` — phase 06's `open_index()` / `ensure_schema()` /
  `SCHEMA_VERSION`. This phase makes them live.
- `src/memory.rs:240` `memory_dir_for_namespace()` — the two-location layout the
  write path and the reconciler both have to walk.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The three mutators write files and never touch the index. `add_memory`
(`src/memory.rs:368`) in full:

```rust
pub fn add_memory(key: &str, value: &str, category: MemoryCategory, namespace: &str) -> Result<()> {
    validate_memory_key(key)?;
    let dir = memory_dir_for_namespace(namespace, &category);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating memory dir {}", dir.display()))?;
    let path = dir.join(format!("{}.md", key));
    std::fs::write(&path, value).with_context(|| format!("writing memory key '{}'", key))?;
    crate::daemon::stats::inc_memories_created();
    Ok(())
}
```

`update_memory` (`:281`) merges frontmatter and writes; `delete_memory` (`:379`)
removes the file if it exists. Each ends with a `stats::` counter bump — the
index hook goes in the same place.

**Note:** these functions do **no** size-capping, locking or masking — masking
and `SESSION_MEMORY_CAP` live in `load_session_memory_block()`. Do not go looking
for that machinery; it is not there. `CLAUDE.md` used to claim otherwise and
**phase 10 already corrected it**, so there is nothing to fix — **do not edit
`CLAUDE.md` in this phase.**

### Five facts, each verified against the real code before this spec was written

**1. FTS5 has no upsert. Update means DELETE then INSERT.** Verified on
`sqlite3 3.53.4`:

```
sqlite> INSERT INTO memories VALUES(...) ON CONFLICT DO NOTHING;
Parse error: UPSERT not implemented for virtual table "memories"
```

A scoped `DELETE` *does* work on the virtual table, including on the
`UNINDEXED` columns, and correctly leaves other namespaces alone:

```
DELETE FROM memories WHERE key='k1' AND namespace='global' AND category='knowledge';
-- the ('k1','agent-x') row survives
```

So the upsert is: scoped `DELETE`, then `INSERT`. There is no shortcut.

**2. `memory::index` can call `memory`'s private items.** `pub mod index;` is
declared *inside* `src/memory.rs:4`, so `index` is a descendant module and Rust
privacy lets it reach its ancestor's private items. Verified by compiling it:

```rust
// from inside src/memory/index.rs
let (fm, body) = super::parse_memory_frontmatter(raw);
// PROBE tags=["a", "b"] summary=Some("s") body="the body"
```

**Use `super::parse_memory_frontmatter`. Do not make it `pub`, and do not write
a second frontmatter parser.** (From the `tests` submodule it is
`super::super::parse_memory_frontmatter`.)

**3. `MemoryInfo` has no body.** `list_memories_with_tags()` returns
`MemoryInfo` (`src/memory.rs:47`), which carries `key`/`tags`/`summary` but
**not** the file body. The index needs the body, so the reconciler must read
each file itself and parse it — it cannot be built on `list_memories_with_tags`
alone.

**4. Namespaces must be enumerated, not passed in.**
`list_memories_with_tags(category, namespaces)` takes the namespace list as a
parameter. For a full rebuild the set is `"global"` plus one entry per agent:
`crate::agents::list_agents()` returns `Vec<AgentInfo>` and `AgentInfo.name`
(`src/agents/mod.rs:57`) is the namespace. It returns an empty `Vec` when
`agents/` does not exist, so no special-casing is needed.

**5. The incident directory on disk is `incidents`, plural.**
`MemoryCategory::Incident.dir_name()` returns `"incidents"` while
`canonical_name()` returns `"incident"` (`src/memory.rs:18` and `:31`). Use
`dir_name()` for paths and `canonical_name()` for the value stored in the
index's `category` column, exactly as `list_memories_with_tags` already does.

> **The runtime tree is already correct — leave it alone.** It used to say
> `incident/`, a directory that never exists; **phase 10 fixed that** and added
> the `agents/*/memory/` entries. Editing `RUNTIME_TREE` or the asset now would
> break `render_matches_shipped_asset`. Just don't propagate the singular into
> any path you build.

## Spec

### 1. Delete `#![allow(dead_code)]`

Remove the **module-level** attribute at `src/memory/index.rs:3`. Phase 06 added
it because nothing called `open_index`/`ensure_schema`/`SCHEMA_VERSION` yet; this
phase calls all three, so the blanket form must go.

**`reconcile_index` and `ReconcileReport` are the exception, and they need a
narrow allow.** They are deliberately left with no production caller (see Out of
scope), so under `-D warnings` they raise exactly two errors:

```
error: struct `ReconcileReport` is never constructed
error: function `reconcile_index` is never used
```

Put an item-level `#[allow(dead_code)]` on each, with a comment naming why:

```rust
// The operator-facing repair path. Exercised by this phase's reconciliation
// tests; a production caller (startup or a `reindex` subcommand) is deliberately
// deferred — see this phase's Out of scope.
#[allow(dead_code)]
pub struct ReconcileReport { … }

#[allow(dead_code)]
pub fn reconcile_index() -> anyhow::Result<ReconcileReport> { … }
```

**Do not** add an allow to anything else, and do not restore the module-level
form. Two named items is strictly narrower than what phase 06 had, which is the
direction phase 06's review asked for. If a `dead_code` error appears on any
*other* item, that one really is unwired — fix the wiring rather than silencing
it.

### 2. Index write helpers in `src/memory/index.rs`

All three open their own connection via `open_index()` and drop it at the end.

**Open per operation; do not cache a global `Connection`.** Two reasons, and the
second is not optional: memory writes are rare so there is nothing to optimise,
and the test suite repoints `HOME` between tests — a connection cached on first
use would bind to whichever `HOME` won the race and silently corrupt every later
test. A `static` connection is the one design that cannot work here.

```rust
/// Read the memory file for (namespace, category, key), parse it, and upsert
/// the row. A missing file is not an error — it is treated as a delete.
pub fn index_memory_file(
    key: &str,
    category: crate::memory::MemoryCategory,
    namespace: &str,
) -> anyhow::Result<()>

/// Remove the row for (namespace, category, key). Removing a row that is not
/// there is a no-op, not an error.
pub fn remove_from_index(
    key: &str,
    category: crate::memory::MemoryCategory,
    namespace: &str,
) -> anyhow::Result<()>
```

`index_memory_file` builds the path with `memory_dir_for_namespace(namespace,
&category).join(format!("{key}.md"))`, reads it, splits it with
`super::parse_memory_frontmatter`, and upserts:

1. `DELETE FROM memories WHERE key = ?1 AND namespace = ?2 AND category = ?3`
2. `INSERT INTO memories (key, namespace, category, tags, summary, body)
   VALUES (?1, ?2, ?3, ?4, ?5, ?6)`

Column values: `key` as given; `namespace` as given; `category` from
`category.canonical_name()`; `tags` as the frontmatter tags joined with a single
space (FTS5 tokenizes on whitespace, so a joined string is what makes individual
tags searchable); `summary` from the frontmatter or `""` when absent; `body` the
parsed body.

Do the delete and the insert inside one transaction so a crash between them
cannot leave the row missing. `rusqlite` gives you
`let tx = conn.transaction()?; … tx.commit()?;`.

### 3. Reconciliation

```rust
/// What a reconcile pass changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Rows present in the index at the start of the pass.
    pub rows_before: usize,
    /// Rows present after the rebuild.
    pub rows_after: usize,
}

/// Rebuild the whole index from the memory files on disk.
pub fn reconcile_index() -> anyhow::Result<ReconcileReport>
```

The index is derived, so the honest implementation is a full rebuild rather than
a diff: count the existing rows, `DELETE FROM memories`, then walk every
(namespace, category) pair and re-insert every file. Wrap the whole rebuild in
one transaction.

Namespaces: `"global"` plus `crate::agents::list_agents()?` names. Categories:
all three of `Session`, `Knowledge`, `Incident`. Skip a directory that does not
exist. For each `*.md` file, the key is the file stem.

**Skip expired memories**, matching `list_memories_with_tags`, which filters on
`info.is_expired()` (`src/memory.rs:62`). An expired memory should not be
recallable, so it should not be in the index.

### 4. Hook the three mutators — best-effort, never fatal

In `src/memory.rs`, after the successful `fs::write` / `fs::remove_file` and
next to the existing `stats::` bump:

```rust
if let Err(e) = crate::memory::index::index_memory_file(key, category, namespace) {
    log::warn!("memory index update failed for '{key}': {e:#}");
}
```

and for `delete_memory`, the same shape around `remove_from_index`.

**The files are the source of truth and the index is derived, so an index
failure must never fail the caller.** If the database is unwritable, adding a
memory still has to succeed — the user's content matters more than the cache,
and `reconcile_index()` exists to repair the gap later. This is a behaviour the
spec pins with a test, not a preference.

`log::warn!` is the idiom in this crate (see `src/session_store.rs:362`).
`{e:#}` prints the full `anyhow` context chain.

Note `update_memory` takes its fields through `UpdateMemoryArgs`
(`src/memory.rs:269`); destructure or re-derive `key`/`category`/`namespace` for
the hook.

### 5. Tests

Add to the existing `#[cfg(test)] mod tests` in `src/memory/index.rs`. Every
test that touches `HOME` must take `crate::test_home_guard()` **before**
`set_var`, and must use **`tempfile::tempdir()`** — a fresh directory per test,
which cleans up on drop. Do not use a fixed `/tmp` path; a previous phase did
and it silently disabled a test's only assertion on warm runs. The exact shape:

```rust
let _guard = crate::test_home_guard();
let tmp = tempfile::tempdir().unwrap();
unsafe { std::env::set_var("HOME", tmp.path()) };
```

(Edition 2024, so `set_var` needs `unsafe`.)

Name them exactly:

- `add_memory_indexes_the_row` — `add_memory` a knowledge memory whose body
  contains a distinctive word, then assert a `MATCH` on that word returns
  exactly 1 row with the expected key.
- `update_memory_replaces_the_row_not_duplicates_it` — add, then
  `update_memory` with a new body. Assert the total row count for that key is
  **1**, not 2, and that a `MATCH` on the *old* body text now returns 0 rows.
  This is the test that proves the DELETE-then-INSERT upsert is real.
- `delete_memory_removes_the_row` — add, delete, assert 0 rows for that key.
- `same_key_in_two_namespaces_is_two_rows` — add the same key under `"global"`
  and under an agent namespace, assert 2 rows, then delete the global one and
  assert the agent row survives. This pins that the scoped DELETE keys on
  namespace and not on key alone.
- `index_failure_does_not_fail_add_memory` — the load-bearing negative. Make the
  index unwritable, call `add_memory`, and assert **it returns `Ok`** and the
  memory file exists on disk. Create a *file* named `var/index` where the
  directory is expected — `create_dir_all` then fails, so `open_index()` errors
  and the hook takes its warn path. Do **not** assert on log output; assert on
  the return value and the file.
- `reconcile_rebuilds_from_disk` — write two memory files directly with
  `std::fs::write` (bypassing `add_memory`, so the index never learns about
  them), call `reconcile_index()`, and assert `rows_after == 2`.
- `reconcile_after_incremental_writes_is_a_no_op` — **the strongest test in this
  phase.** Do a realistic sequence through the public API: `add_memory` three
  memories across two categories, `update_memory` one, `delete_memory` one. Then
  call `reconcile_index()` and assert `report.rows_before == report.rows_after`.
  A full rebuild finding exactly what the incremental hooks left behind is what
  "the index survives edits" actually means — it verifies the hooks against disk
  rather than against the order they happened to run in.
- `expired_memory_is_not_indexed` — reconcile a directory containing a memory
  whose frontmatter `expires` is a past date; assert it contributes no row.

## Acceptance criteria

- [ ] The **module-level** `#![allow(dead_code)]` is gone from
      `src/memory/index.rs`; the only remaining allows are the two item-level
      ones on `reconcile_index` and `ReconcileReport`
      (`grep -c 'allow(dead_code)' src/memory/index.rs` returns **2**, and
      `grep -c '#!\[allow' src/memory/index.rs` returns **0**).
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] All eight tests named in spec task 5 pass.
- [ ] `add_memory` returns `Ok` when the index cannot be opened, and the memory
      file is still written — pinned by `index_failure_does_not_fail_add_memory`.
- [ ] `reconcile_after_incremental_writes_is_a_no_op` passes: a full rebuild
      after a mixed add/update/delete sequence finds the same row count.
- [ ] `fts5_search()` still returns an empty `Vec` — phase 08 owns the query
      path.
- [ ] `cargo build` zero new warnings; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes. Lib count rises by the number of tests added (8 by
      this spec, so **1023** — the baseline is **1015** since phase 10 landed);
      integration stays **30** (2 ignored), isolation **8** (1 ignored),
      `bug_tracker` **6**.
- [ ] Only `src/memory/index.rs` and `src/memory.rs` change.

## Test plan

Covered by spec task 5. Two tests carry the phase:

`reconcile_after_incremental_writes_is_a_no_op` is the milestone exit criterion
("the index survives edits… verified by a reconciliation test rather than by
construction order") expressed as code. It is strong precisely because it does
not assert a hand-computed number — it asserts that two independent paths to the
same state agree.

`index_failure_does_not_fail_add_memory` pins the derived-cache contract. Without
it, a later refactor that propagates the index error with `?` would look correct
and would start losing user memories the first time the database is unwritable.

**What would make this phase a false success:** hooks that silently no-op — e.g.
an `index_memory_file` that returns `Ok(())` early on any error path — would pass
`index_failure_does_not_fail_add_memory` and every delete test, while indexing
nothing. `add_memory_indexes_the_row` and `reconcile_rebuilds_from_disk` are what
stop that, because both assert a positive row count.

## End-to-end verification

The real artifact is a database that has rows in it after ordinary use. Run this
block verbatim and paste the resulting file into your Update Log.

**Two constraints carried from phase-03's post-mortem:** **no heredocs**, and
every tree-walking command wrapped in `timeout`. A phase-03 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
cargo build 2>&1 | tail -2
H=$(mktemp -d)
{
  echo "=== seed a tree ==="
  HOME="$H" timeout 120 ./target/debug/daemoneye setup 2>&1 | tail -2
  echo "=== the seeded knowledge memories are on disk ==="
  timeout 30 ls -1 "$H/.daemoneye/memory/knowledge/" | wc -l

  echo "=== the index exists and is a real SQLite file ==="
  timeout 30 ls -l "$H/.daemoneye/var/index/memory.db"
  echo "db-exists-exit=$?   # 0 == PASS"

  echo "=== the write-path tests ==="
  timeout 900 cargo test --lib memory::index 2>&1 | grep -E "^test |^test result"

  echo "=== no allow(dead_code) remains ==="
  timeout 30 grep -c "allow(dead_code)" src/memory/index.rs
  echo "grep-count-above-must-be-0"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/phase07-e2e.txt 2>&1
rm -rf "$H"
cat /tmp/phase07-e2e.txt
```

`grep -c` returning **0** for `allow(dead_code)` together with `clippy-exit=0` is
the proof that the attribute was removed rather than the warning suppressed
somewhere else.

Note the index file will exist because `ensure_dirs()` creates the directory and
phase 06's `open_index()` creates the database — `daemoneye setup` does not
itself add memories, so **do not** expect rows in it from `setup` alone. Row
counts are what the unit tests assert.

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

- [ ] May add dependencies: **none.** `rusqlite`, `anyhow` and `log` are all
      already in `Cargo.toml`.
- [ ] May touch `docs/architecture.md`: **no.** Its § 5 stub note stays until
      phase 09.
- [ ] May touch `CLAUDE.md`: **no.** Its stale claim about `add_memory` is
      phase 09's to correct.
- [ ] May create new files: no.

## Out of scope

- **Implementing `fts5_search()`.** Still a stub until phase 08; an acceptance
  criterion pins it.
- **Quoting/escaping user queries for `MATCH`.** Phase 08 owns it. Recorded here
  only so this phase does not introduce an unquoted `MATCH` on user input: a bare
  `-` or `:` is FTS5 query syntax, so `MATCH 'runtime-layout'` raises *"no such
  column: layout"*. The `MATCH` calls in this phase's own tests use literals you
  control, so quote them and move on.
- **Calling `reconcile_index()` from daemon startup, or adding a `reindex`
  subcommand.** This phase provides and tests the function. Wiring it into the
  boot path has a real cost (it walks every memory file) and a CLI command is its
  own small feature — both are deferred. **This is why the two item-level allows
  in task 1 are required**; the function is genuinely unreferenced outside tests
  and that is intended, not an oversight.
- **`RUNTIME_TREE`, the shipped asset, `POLICY_TABLE` and the path-audit
  `INVENTORY`.** Phase 10 already corrected `incident/` → `incidents/` and added
  the `agents/*/memory/` entries. Nothing here needs touching, and editing the
  tree would break `render_matches_shipped_asset`.
- **Masking memory content before indexing.** The files on disk are not masked
  either, and the database sits inside the same private `~/.daemoneye/` tree.
  Not this phase's problem to invent.
- **Changing `ftsearch_memories()`** (`src/daemon/memory_prompt.rs:201`) or
  `list_memories_with_tags`.
- **A schema change.** If one seems necessary, note it and stop — `ensure_schema`
  drops and recreates on a `SCHEMA_VERSION` mismatch, so a bump is cheap, but the
  current columns are sufficient for everything this phase does.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-08-01

**Your implementation was correct. The spec was not.** Everything you wrote is
on disk and verified working — the architect temporarily added the two allows
described in task 1 and got `cargo test` at **1023 lib** (exactly this phase's
target) / 30 integration (2 ignored) / 8 isolation (1 ignored) / 6 bug_tracker,
with clippy exit 0. Then reverted, so the tree is back in your state. **Do not
redo any of it.**

The run hard-failed on a `NoProgressStall` after 60 consecutive read-only calls,
and that stall was the spec's fault. Task 1 said "delete the allow; if a
`dead_code` warning survives, fix the wiring, do not re-add the attribute" while
Out of scope said "leave `reconcile_index()` uncalled outside tests". Those two
instructions cannot both be satisfied: `reconcile_index` and `ReconcileReport`
have no production caller *by design*, so `-D warnings` raises two `dead_code`
errors and the spec forbade both available fixes. You spent the stall grepping
for a non-test caller that the spec had told you not to create. That was an
impossible search, not a mistake.

**Task 1 has been rewritten** with the resolution: item-level
`#[allow(dead_code)]` on `reconcile_index` and `ReconcileReport` only, with the
comment shown, and the module-level `#![allow(dead_code)]` staying deleted. The
acceptance criterion now pins the exact grep counts (2 item-level, 0
module-level).

**There is exactly one edit left**: add those two attributes. Then run the
gates and the End-to-end block. `cargo test` must report **1023** lib — if it
reports more, something got added that should not have been.

### Update — 2026-08-01 (escalation)

**Chosen lever:** resume (`continue_phase`)
**Rationale:** the implementation is complete and independently verified correct
on disk; the only blocker was an unsatisfiable instruction in the spec, now
fixed. Re-dispatching from the top risks re-deriving 450 lines of working code,
and a takeover would forfeit the model data point on work the model actually got
right. A fresh briefing-seeded context also breaks the read-only loop that the
stale instruction produced.
