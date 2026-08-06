# Phase 05c: reconcile scope — an empty corpus must not wipe the others

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
**Depends on:** phase-05b (done — surfaced the defect)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Stop a search over an empty corpus from destroying every other corpus. Make
`open_and_reconcile_if_empty(table)` rebuild **only that corpus**, per the PE's
decision (Option 1 of [bug-05c-1](bugs/bug-05c-1.md)).

## Architecture references

Read before starting:

- [bug-05c-1](bugs/bug-05c-1.md) — the defect, its reproduction, and the two
  options considered. **The PE chose Option 1: per-corpus reconcile.** Option 2
  is rejected; do not implement it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the bug doc above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`reconcile_index()` (`src/memory/index.rs`) opens one transaction, deletes all
seven tables, then rebuilds each corpus from disk in clearly delimited sections:

```rust
tx.execute("DELETE FROM memories", [])
tx.execute("DELETE FROM artifacts", [])
tx.execute("DELETE FROM epochs", [])
tx.execute("DELETE FROM turns", [])
tx.execute("DELETE FROM turns_map", [])
tx.execute("DELETE FROM events", [])
tx.execute("DELETE FROM events_map", [])

// ── memories corpus ──   … walks memory_dir_for_namespace() per namespace/category
// ── artifacts corpus ──  … list_runbooks() + list_scripts_with_tags()
// ── epochs corpus ──     … *.epochs.jsonl in sessions_dir(), via read_epochs()
// ── turns corpus ──      … *.archive.jsonl in sessions_dir(), byte-offset scan
// ── events corpus ──     … event segments, byte-offset scan
```

`open_and_reconcile_if_empty(table)` calls the whole thing when `table` is empty,
which is the bug: five corpora, seven tables, one indiscriminate rebuild.

**There are five corpora, not seven.** `turns`/`turns_map` and `events`/
`events_map` are each one corpus in two tables — rebuilding either **must** clear
both halves together or the map ids desynchronise from the FTS rowids.

## Spec

### 1. A `Corpus` enum — `src/memory/index.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corpus { Memories, Artifacts, Epochs, Turns, Events }
```

Give it `fn table_name(self) -> &'static str` returning the FTS table
(`"memories"`, `"artifacts"`, `"epochs"`, `"turns"`, `"events"`) and
`fn from_table(name: &str) -> Option<Corpus>` for the reverse. `from_table`
returns `None` for anything unrecognised — including `"turns_map"` and
`"events_map"`, which are not corpora.

### 2. Extract one rebuild function per corpus

Extract each `// ── … corpus ──` section into its own function taking
`&rusqlite::Transaction` (or `&rusqlite::Connection`; `Transaction` derefs to it,
the same trick phase 03a used for `index_archive_file`):

```rust
fn rebuild_memories(tx: &rusqlite::Connection) -> anyhow::Result<()>
fn rebuild_artifacts(tx: &rusqlite::Connection) -> anyhow::Result<()>
fn rebuild_epochs(tx: &rusqlite::Connection) -> anyhow::Result<()>
fn rebuild_turns(tx: &rusqlite::Connection) -> anyhow::Result<()>
fn rebuild_events(tx: &rusqlite::Connection) -> anyhow::Result<()>
```

**Each function owns its own DELETE**, so it is self-contained:

- `rebuild_memories` → `DELETE FROM memories`
- `rebuild_artifacts` → `DELETE FROM artifacts`
- `rebuild_epochs` → `DELETE FROM epochs`
- `rebuild_turns` → `DELETE FROM turns` **and** `DELETE FROM turns_map`
- `rebuild_events` → `DELETE FROM events` **and** `DELETE FROM events_map`

**This is a pure extraction.** Move the existing code; do not change what it
reads, how it composes bodies, its masking, or its per-file error handling. The
02b lesson still binds: a per-file read error is logged and ends that file's
scan, never `?`-propagated past the file it came from.

### 3. `reconcile_index()` keeps its exact contract

Rewrite its body to open one transaction and call all five in the current order
(memories, artifacts, epochs, turns, events), then commit. Its signature,
its `ReconcileReport` (`rows_before`, `rows_after`, `per_corpus` in that stable
order), and its observable behavior must be **unchanged** — `daemoneye reindex`
and a dozen existing tests depend on it.

### 4. `reconcile_corpus` — the new targeted entry point

```rust
pub fn reconcile_corpus(corpus: Corpus) -> anyhow::Result<usize>
```

Opens its own connection and one transaction, calls that corpus's rebuild
function, commits, and returns the corpus's row count afterwards.

### 5. Point `open_and_reconcile_if_empty` at it

Replace its `reconcile_index()` call with `reconcile_corpus(c)` for the `Corpus`
resolved from the table name. If `from_table` returns `None`, **do not reconcile
at all** — log a warning and return the connection as-is. Silently rebuilding
everything on an unrecognised name is how this bug shipped.

Keep the existing re-open-after-reconcile step: a reconcile can drop and recreate
the DB, so the returned connection must be fresh.

## Acceptance criteria

- [ ] **The bug is fixed.** Index a turn and an epoch, then run a search whose
      corpus is empty (e.g. `kind="memory"` with no memories). Both the turn and
      the epoch are **still findable afterwards**. This is the criterion the
      phase exists for.
- [ ] The same holds for `kind="all"` with several empty corpora in the chain.
- [ ] **The reconcile still happens for the corpus that was empty.** Searching an
      empty `artifacts` corpus with a runbook present on disk finds it — the
      self-healing property is preserved, not removed.
- [ ] `reconcile_corpus(Corpus::Turns)` clears **both** `turns` and `turns_map`,
      and the rebuilt rows' offsets still seek to their own lines. Same for
      `events` / `events_map`.
- [ ] **`reconcile_index()` is unchanged in behavior.** Its `per_corpus` vector
      has the same five entries in the same order, and `rows_after` matches a
      pre-refactor run on the same fixture.
- [ ] `Corpus::from_table("turns_map")` and `from_table("nonsense")` both return
      `None`, and `open_and_reconcile_if_empty` with such a name reconciles
      **nothing** — assert no other corpus lost rows.
- [ ] **Phase 05b's workaround can be removed.** Delete the
      "seed EVERY corpus" block from `all_kind_excludes_turns_and_epochs`
      (`src/search.rs`) — keep only the turn and epoch fixtures — and the test
      must still **fail under mutation** (adding `search_turns_fts` to the `"all"`
      arm) and pass restored. This is the end-to-end proof that the bug is gone.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green, no existing test removed or `#[ignore]`d.

## Test plan

Use the home-guard convention already in each module.

- `empty_corpus_search_preserves_other_corpora` — the headline case: a turn and
  an epoch survive a `kind="memory"` search on an empty memory store.
- `all_kind_search_preserves_turns_and_epochs` — same via the `"all"` chain.
- `reconcile_corpus_rebuilds_only_its_own_corpus` — seed two corpora, reconcile
  one, assert the other's row count is **unchanged**.
- `reconcile_corpus_turns_clears_both_tables`
- `reconcile_corpus_events_clears_both_tables`
- `reconcile_index_report_is_unchanged` — five `per_corpus` entries, same order.
- `unknown_table_name_reconciles_nothing`
- `empty_artifacts_corpus_still_self_heals` — the property we are keeping.

**Negative cases to pin** (each must NOT happen):

- A per-corpus reconcile must **not** reduce any other corpus's row count —
  assert the other counts are exactly equal before and after, not merely non-zero.
- `from_table("turns_map")` must **not** resolve to `Corpus::Turns`.
- An unrecognised table name must **not** trigger a full reconcile.
- No rebuild function may `?`-propagate a per-file read error past that file.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib memory::index > /tmp/phase05c-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05c-tests.txt
cargo test --lib search >> /tmp/phase05c-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05c-tests.txt
{ echo "--- each rebuild owns its own DELETE ---";
  grep -n "DELETE FROM memories\|DELETE FROM artifacts\|DELETE FROM epochs\|DELETE FROM turns\|DELETE FROM events" src/memory/index.rs;
  echo "--- open_and_reconcile_if_empty no longer calls reconcile_index ---";
  sed -n '/fn open_and_reconcile_if_empty/,/^}/p' src/memory/index.rs;
  echo "--- 05b workaround removed from the guard test ---";
  sed -n '/fn all_kind_excludes_turns_and_epochs/,/^    }/p' src/search.rs;
} > /tmp/phase05c-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase05c-checks.txt
```

**Paste the contents of both files whole and unedited.** Read the files back and
paste what is in them. Do not type test names from memory and do not reconstruct
a listing to match a count you expect — at review the pasted names are diffed
against a live run, and any name that does not exist in the tree fails
`STANDARDS.md` §1 outright.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`.

## Mutation check before reporting complete

**This phase's mutation check is the one that matters most, because the previous
phase shipped its mutation by accident. Read this twice.**

Change `open_and_reconcile_if_empty` back to calling `reconcile_index()` instead
of `reconcile_corpus(c)`. Confirm `empty_corpus_search_preserves_other_corpora`
**fails**. Then **restore it** and confirm it passes. State both results in your
Update Log.

**A mutation check is always break → observe → RESTORE.** You never keep the
mutation, and you never rewrite a test to match mutated code — if a test fails,
the code is wrong, not the test. Before reporting complete, run
`grep -n "reconcile_index()" src/memory/index.rs` and confirm the only remaining
call sites are `reconcile_index`'s own definition and its tests — **never** inside
`open_and_reconcile_if_empty`.

## Authorizations

- Modify: `src/memory/index.rs`, `src/search.rs` (only to remove the 05b
  workaround from `all_kind_excludes_turns_and_epochs`).
- No new dependencies. No DDL or `SCHEMA_VERSION` change.
- Do **not** change `ReconcileReport`, `reconcile_index()`'s signature, or the
  `daemoneye reindex` command.

## Out of scope

- **Whether "empty" is the right trigger at all.** The bug doc raises that a user
  with genuinely zero memories rebuilds on every search. Real, but a separate
  decision — leave the trigger as-is.
- Phase 06 (prompt scoring) and phase 07.
- Any change to the incremental hooks from 03a/03b.

## Update Log

<!-- entries appended below this line -->
