# Phase 10: Tree and Doc Truth

**Milestone:** M7 — Memory Search & Maintenance
**Status:** todo
**Depends on:** phase-05 (generated-runtime-tree, done) — this phase edits
`RUNTIME_TREE` and the asset it renders to. **Independent of the FTS5 chain
(06–09); dispatchable out of numeric order.**
**Estimated diff:** ~200 lines across 6 files.

**Tags:** language=rust, kind=bugfix, size=m

## Goal

`memory/incident/` does not exist. The real directory is `memory/incidents/`,
and three places disagree about it — one of them a **live bug** where incident
memories never get their `session_origin` stamped. Fix the code defect, correct
the runtime tree, close the gate gap that let a non-existent path sit in an
agent-facing document, and correct two `CLAUDE.md` rows that describe machinery
the code does not have.

This is the same defect class M6 item 5 was about. Phases 04 and 05 built gates
for it and this slipped through both, which is the more interesting half of the
phase.

## Architecture references

- `src/memory.rs:18` / `:31` — `MemoryCategory::dir_name()` returns
  `"incidents"`; `canonical_name()` returns `"incident"`. `dir_name()` is
  authoritative for paths.
- `src/config/lifecycle.rs:263` `is_covered()` — why the existing gates missed
  this; see "Why the gates did not catch it" below.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### The live bug — incident memories are never stamped

`stamp_artifact_origin` (`src/session_store.rs:374`), reached from
`backfill_session_origin` at `:361`:

```rust
"memory" => {
    for dir_name in &["knowledge", "session", "incident"] {
        let path = base
            .join("memory")
            .join(dir_name)
            .join(format!("{}.md", artifact_name));
        if path.exists() {
```

`memory_dir_for_namespace(_, Incident)` writes to `memory/incidents/`, so
`memory/incident/<name>.md` **never exists** and the loop silently finds
nothing. An incident memory created inside a named session never receives its
`session_origin` stamp. There is no test covering it — the existing backfill
test (`src/session_store_tests.rs:474`) uses a knowledge memory, which works.

Verified empirically: after `daemoneye setup`, `memory/` contains only
`knowledge` and `session`; `incidents/` is created lazily on first incident
write. Nothing anywhere creates the singular form.

### The tree says the same wrong thing

`RUNTIME_TREE` (`src/config/runtime_tree.rs`) and the asset it renders to both
document `incident/`. That text ships to operators as a seeded knowledge memory
and is read by the AI.

### Why the gates did not catch it

This is the part worth understanding before changing anything.
`every_policy_path_appears_in_tree` (phase 05) checks that every `POLICY_TABLE`
path appears in the tree. `POLICY_TABLE` carries **`memory`** and nothing below
it — no `memory/knowledge`, no `memory/incidents`. So the tree's per-category
lines were never cross-checked against anything.

`every_existing_directory_has_a_policy_entry` (Direction A) does not help
either: `is_covered()` (`src/config/lifecycle.rs:263`) treats a directory as
covered if it is a **subdirectory of** a table entry, so a real
`memory/incidents` on disk is "covered" by the `memory` entry without ever being
named.

**The durable fix is therefore not the spelling — it is putting the
per-category paths into `POLICY_TABLE` so the phase-05 cross-check can see
them.** Once they are there, the tree cannot disagree with the code without a
test failing. Spec task 3 is what makes this phase more than a typo fix.

### `agents/*/memory/` is in neither table

`memory_dir_for_namespace()` creates `agents/<ns>/memory/<category>/` for every
non-global namespace. Neither `POLICY_TABLE` nor `RUNTIME_TREE` mentions it.

### Two `CLAUDE.md` rows describe machinery that does not exist

Line 71 claims `add_memory()`/`update_memory()` "enforce size cap, fcntl lock,
masking, index sync (G1)" and that `src/memory.rs` carries G2 schema fields
`volatility`, `lifecycle`, `confidence`, `source`, `pinned`, `last_verified`,
`verified_by`, `usefulness_score`, plus "schema validation, version history".

Checked one claim at a time against `src/memory.rs`:

| Claim | Reality |
|---|---|
| size cap in the mutators | **No.** `SESSION_MEMORY_CAP` applies to `load_session_memory_block()` |
| fcntl lock | **No.** No `flock`/`fcntl` anywhere in the file |
| masking in the mutators | **No.** `mask_sensitive()` is called in `load_session_memory_block()` |
| index sync | **No** as of today (phase 07 adds it) |
| G2 fields | **7 of 8 absent.** Only `pinned` appears, and only as a `MemoryInfo` field that `list_memories_with_tags` always sets to `None` |
| schema validation | **No** such function |
| version history | **No** such machinery |

The masking and size-cap claims are not invented — they are attached to the
wrong function. That is what the correction should say.

## Spec

### 1. Fix `stamp_artifact_origin` — and make it drift-proof

In `src/session_store.rs:374`, replace the hardcoded `&["knowledge", "session",
"incident"]` with the enum's own directory names, so this cannot drift again:

```rust
"memory" => {
    for category in [
        crate::memory::MemoryCategory::Knowledge,
        crate::memory::MemoryCategory::Session,
        crate::memory::MemoryCategory::Incident,
    ] {
        let path = base
            .join("memory")
            .join(category.dir_name())
            .join(format!("{}.md", artifact_name));
        if path.exists() {
```

Leave the rest of the loop body unchanged. `MemoryCategory` is `Copy`
(`src/memory.rs:7`), so iterating by value is fine.

**Do not** "fix" this by changing `dir_name()` to return the singular. The
plural is what is on disk in every existing installation; changing it would
orphan real user data.

### 2. Correct the runtime tree, and add the agent memory subtree

In `src/config/runtime_tree.rs`:

**2a.** Under `memory/`, rename the third child from `incident/` to
`incidents/`. The note stays as it is.

**2b.** Under `agents/` → `<name>/`, add a `memory/` node **after**
`briefing.md` and **before** `mailbox/`, with three unannotated children:

```rust
TreeNode {
    name: "memory/",
    note: None,
    blank_before: false,
    children: &[
        TreeNode { name: "session/", note: None, blank_before: false, children: &[] },
        TreeNode { name: "knowledge/", note: None, blank_before: false, children: &[] },
        TreeNode { name: "incidents/", note: None, blank_before: false, children: &[] },
    ],
},
```

**2c.** Update `assets/memory/knowledge/agent-runtime-layout.md` to match. The
exact lines, computed with the phase-05 renderer:

```
    incidents/               ← post-mortems, never auto-loaded
```

and, inside the `agents/<name>/` block:

```
      memory/
        session/
        knowledge/
        incidents/
```

If you get the spacing wrong, `render_matches_shipped_asset` fails **and prints
the correct rendered tree** — copy that output rather than counting spaces. All
new lines are well inside the annotation column (the longest is 18 characters
against a limit of 29), so `annotation_column_is_not_overflowed` stays green.

### 3. Close the gate gap — the point of this phase

Add per-category entries to `POLICY_TABLE` (`src/config/lifecycle.rs`) so the
phase-05 cross-check actually covers them. Six entries, following the shape of
the existing `memory` entry:

| `path` | `lazy` |
|---|---|
| `memory/session` | `false` |
| `memory/knowledge` | `false` |
| `memory/incidents` | **`true`** |
| `agents/*/memory/session` | `true` |
| `agents/*/memory/knowledge` | `true` |
| `agents/*/memory/incidents` | `true` |

All six take `intent: LifecycleIntent::KeepForever`, `config_key: None`,
`implemented: ImplementationStatus::Implemented`, and a short `note`.

**The `lazy` values are not guesswork — get them right or a gate fails.**
`ensure_dirs()` creates `memory/session` and `memory/knowledge` (via
`seed_memory_inner`, which does `create_dir_all` before writing each seeded
memory), so those two are eager. Nothing creates `memory/incidents` until the
first incident is written, so it is lazy.
`every_eager_policy_entry_is_created_by_ensure_dirs`
(`src/config/lifecycle.rs:469`) is the test that checks this.

The wildcard entries are fine under Direction B
(`every_policy_entry_corresponds_to_a_real_path`): it normalises
`agents/*/memory/session` to the prefix `agents`, which exists after
`ensure_dirs()` seeds the example agents.

**Verify the gate actually closed.** Before fixing the tree in task 2, adding
`memory/incidents` to `POLICY_TABLE` should make
`every_policy_path_appears_in_tree` **fail** — that is the whole point. Quote
that red run in your Update Log, then apply task 2 and show it green. If it does
not go red, the cross-check is not covering what this phase claims it does; stop
and report that as a blocker.

### 4. Path-audit inventory

Add matching `InventoryEntry` rows in `src/config/path_audit.rs` for the three
global paths, so `audit-prompts` does not report them `Unknown`:

```rust
InventoryEntry { path: "memory/session",   status: PathStatus::Current, source: "memory::memory_dir_for_namespace(\"global\", Session)" },
InventoryEntry { path: "memory/knowledge", status: PathStatus::Current, source: "memory::memory_dir_for_namespace(\"global\", Knowledge)" },
InventoryEntry { path: "memory/incidents", status: PathStatus::Current, source: "memory::memory_dir_for_namespace(\"global\", Incident)" },
```

Do **not** add the `agents/*/…` wildcard forms to `INVENTORY` — that table is
keyed on concrete normalised paths, and `normalise()` does not expand wildcards.

### 5. Correct `CLAUDE.md`

**5a — line 71**, the `src/memory.rs` row. Replace the false claims with what
the file actually contains. Keep the row one line (the table format requires
it). Something close to:

> `src/memory.rs` | Memory module: `MemoryCategory` (note `Incident.dir_name()`
> is `incidents`, plural, while `canonical_name()` is `incident`), `MemoryInfo`,
> and CRUD — `add_memory` / `update_memory` / `delete_memory` / `read_memory` /
> `list_memories` / `list_memories_with_tags`. `memory_dir_for_namespace()`
> resolves the two-location layout: `memory/<category>/` for the `global`
> namespace, `agents/<ns>/memory/<category>/` otherwise. Masking
> (`mask_sensitive`) and the `SESSION_MEMORY_CAP` size cap apply in
> `load_session_memory_block()`, **not** in the mutators — the mutators do no
> locking, capping or masking. Frontmatter fields are `tags`, `summary`,
> `relates_to`, `created`, `updated`, `expires`

**5b — line 72**, the `src/memory/index.rs` row. Only the flatly-false clause
needs removing: it says "there is no SQLite index, no `var/index/memory.db`",
which phase 06 made untrue. Keep the rest — `fts5_search()` really does still
return an empty `Vec`, and the grep scan in `src/search.rs` really is still
where recall comes from. Make the minimal edit; **phase 09 owns the full
rewrite** once search is real, and a minimal edit here stays correct whether or
not phase 07 lands first.

Do not touch any other row, and do not touch `docs/architecture.md`.

### 6. Tests

- `incident_memory_gets_session_origin_stamped` — in
  `src/session_store_tests.rs`, alongside the existing backfill test at `:474`.
  Seed a temp `HOME`, write a memory file into `memory/incidents/`, call
  `backfill_session_origin` with an `ArtifactRef` of kind `"memory"` naming it,
  and assert the file now contains `session_origin: "<name>"`. This is the
  regression test for the live bug; it fails against the current code.
- `policy_table_covers_every_memory_category` — in
  `src/config/lifecycle.rs` tests. For each of the three
  `MemoryCategory` variants, assert `POLICY_TABLE` contains an entry whose path
  is `format!("memory/{}", category.dir_name())`. Deriving the expected path
  from `dir_name()` rather than hardcoding it is what makes this test catch a
  future rename.

Use `crate::test_home_guard()` **before** `set_var`, and `tempfile::tempdir()`
for the temp `HOME` — a fresh directory per test, cleaned up on drop. Edition
2024, so `set_var` needs `unsafe`:

```rust
let _guard = crate::test_home_guard();
let tmp = tempfile::tempdir().unwrap();
unsafe { std::env::set_var("HOME", tmp.path()) };
```

## Acceptance criteria

- [ ] `stamp_artifact_origin` uses `MemoryCategory::dir_name()`; no hardcoded
      category-directory list remains in `src/session_store.rs`.
- [ ] `incident_memory_gets_session_origin_stamped` passes, and **fails against
      the pre-fix code** — quote the red run in the Update Log.
- [ ] `grep -rn '"incident"' src/` finds no path-building use of the singular.
      (`canonical_name()` returning `"incident"` is correct and stays.)
- [ ] Adding `memory/incidents` to `POLICY_TABLE` **before** the tree fix makes
      `every_policy_path_appears_in_tree` fail — quoted in the Update Log — and
      it is green after the tree fix.
- [ ] `render_matches_shipped_asset`, `every_policy_path_appears_in_tree`,
      `every_existing_directory_has_a_policy_entry`,
      `every_eager_policy_entry_is_created_by_ensure_dirs`,
      `every_policy_entry_corresponds_to_a_real_path` and
      `annotation_column_is_not_overflowed` all pass.
- [ ] `policy_table_covers_every_memory_category` passes.
- [ ] `daemoneye audit-prompts` still exits **0** on a freshly seeded tree.
- [ ] `CLAUDE.md` no longer claims the mutators do size-capping, locking,
      masking or index sync, no longer lists the seven absent G2 fields, and no
      longer says there is no `var/index/memory.db`.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes. Lib count rises by 2 (**1015**, or **1023** if phase
      07 landed first); integration stays **30** (2 ignored), isolation **8**
      (1 ignored), `bug_tracker` **6**.

## Test plan

Covered by spec task 6, plus the two red-run demonstrations the acceptance
criteria require. Those two matter more than the tests themselves:

- The **pre-fix red run** of `incident_memory_gets_session_origin_stamped`
  proves the bug was real rather than theoretical.
- The **pre-tree-fix red run** of `every_policy_path_appears_in_tree` proves the
  gate gap is genuinely closed. Without it, task 3 is six table entries that
  might be inert.

**What would make this phase a false success:** correcting the spelling in the
tree and the asset, everything green, and `POLICY_TABLE` still carrying only
`memory` — the documents agree again but nothing stops them diverging next time,
which is exactly how this defect survived phases 04 and 05. The second red run
is what distinguishes the two outcomes.

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
  echo "=== the tree no longer claims a directory that cannot exist ==="
  timeout 30 grep -c "incident/" assets/memory/knowledge/agent-runtime-layout.md
  echo "singular-count-above-must-be-0"
  timeout 30 grep -c "incidents/" assets/memory/knowledge/agent-runtime-layout.md
  echo "plural-count-above-must-be-2   # memory/ and agents/<name>/memory/"

  echo "=== seeded tree still audits clean ==="
  HOME="$H" timeout 120 ./target/debug/daemoneye setup 2>&1 | tail -2
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts > /dev/null 2>&1
  echo "clean-audit-exit=$?   # 0 == PASS"

  echo "=== what actually exists under memory/ ==="
  timeout 30 ls -1 "$H/.daemoneye/memory/"

  echo "=== CLAUDE.md no longer carries the false claims ==="
  timeout 30 grep -c "fcntl lock" CLAUDE.md
  echo "fcntl-count-above-must-be-0"
  timeout 30 grep -c "no .var/index/memory.db" CLAUDE.md
  echo "no-index-claim-count-above-must-be-0"

  echo "=== the gates ==="
  timeout 600 cargo test --lib runtime_tree 2>&1 | grep -E "^test result"
  timeout 600 cargo test --lib lifecycle 2>&1 | grep -E "^test result"
  timeout 600 cargo test --lib path_audit 2>&1 | grep -E "^test result"
  timeout 600 cargo test --lib session_store 2>&1 | grep -E "^test result"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/phase10-e2e.txt 2>&1
rm -rf "$H"
cat /tmp/phase10-e2e.txt
```

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

- [ ] May add dependencies: **none**.
- [ ] May touch `docs/architecture.md`: **no.** Its § 5 stub note is phase 09's.
- [ ] May touch `CLAUDE.md`: **yes**, exactly the two rows named in task 5.
- [ ] May edit `assets/memory/knowledge/agent-runtime-layout.md`: **yes**, and
      this phase must, per task 2c.
- [ ] May create new files: no.

## Out of scope

- **Changing `MemoryCategory::dir_name()` or `canonical_name()`.** The plural on
  disk is load-bearing for existing installations; the singular in tool
  arguments is the documented AI-facing contract. Both are correct as they are.
- **`fts5_search()`, the index write path, or anything under
  `src/memory/index.rs`.** Phases 07 and 08.
- **The rest of `CLAUDE.md`** and all of `docs/architecture.md`. Phase 09 owns
  the index-related doc rewrite; this phase makes two minimal corrections and
  stops.
- **Backfilling `session_origin` onto incident memories that were missed while
  the bug was live.** The fix is forward-looking. A migration is a separate
  decision with its own risk.
- **Making `pinned` actually populate.** `list_memories_with_tags` hardcodes
  `pinned: None` (`src/memory.rs`), so the field is inert — a real gap, but not
  this phase's, and the corrected `CLAUDE.md` row simply stops claiming the G2
  fields exist rather than describing `pinned`'s state.
- **Adding `agents/*/…` entries to the path-audit `INVENTORY`.** See task 4.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
