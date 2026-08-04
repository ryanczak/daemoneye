# Phase 03a: Incremental append hooks — index on write, not only on reconcile

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
**Depends on:** phase-02b (done — all five corpora build from disk via reconcile)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make new content searchable the moment it is written, instead of only after a
`daemoneye reindex`. Hook the four write choke points that feed the corpora
added in 02a/02b — archives, event segments, epochs, and artifacts — with
best-effort index writes. Retention-sweep deletion is phase 03b.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Write paths" — the choke-point table and
  the best-effort contract.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`reconcile_index()` (`src/memory/index.rs`) populates all five corpora from disk.
Nothing writes to the index incrementally except memories, which already work and
are the pattern to copy.

**The best-effort convention** — every index write logs and continues, and never
fails its caller. This is `src/memory.rs:386`; do the same shape at every new
hook:

```rust
std::fs::write(&path, value).with_context(|| format!("writing memory key '{}'", key))?;
crate::daemon::stats::inc_memories_created();
if let Err(e) = crate::memory::index::index_memory_file(key, category, namespace) {
    log::warn!("memory index update failed for '{key}': {e:#}");
}
Ok(())
```

Note the ordering: **the file write happens first and its result is what the
caller sees.** An index failure must never turn a successful append into an
error, and must never `?`-propagate.

**Connection cost is not a concern — do not build a pool or a cached
connection.** Measured on this build, 200 iterations each:

```
per_open_ms          = 0.190   (open_index() alone)
per_open_insert_ms   = 0.277   (open_index() + the two inserts a hook does)
per_insert_only_ms   = 0.041   (inserts on an already-open connection)
```

A fresh `open_index()` per hook costs ~0.28 ms all-in. `log_event` is the hottest
caller at a handful per turn, so this is well under a millisecond per turn
against turns that take seconds. Ship the simple thing.

**The scanners you are reusing** live in `reconcile_index()`: the turns scanner
reads `*.archive.jsonl`, the events scanner reads segments, both tracking byte
offsets with `read_line`. Phase 02b's bounce (`bugs/bug-02b-1.md`) established
the rule you must carry into every new read here: **a per-file read error is
logged and ends that file's scan — it is never `?`-propagated** out to the
caller. Quote from that fix:

```rust
let n = match reader.read_line(&mut line) {
    Ok(n) => n,
    Err(e) => {
        log::warn!("skipping {} at offset {offset}: {e}", path.display());
        break;
    }
};
```

## Spec

### 1. Extract a reusable per-file archive indexer — `src/memory/index.rs`

`reconcile_index()`'s turns scanner currently inlines "scan one archive file and
insert its rows". Extract that inner body into a function taking an open
connection or transaction, the session id, and the path, so both the reconcile
loop and the new seed path (§ 3) call the same code. Keep the reconcile loop's
behaviour identical — this is a pure extraction.

Rust's `rusqlite::Transaction` derefs to `Connection`, so a helper taking
`&rusqlite::Connection` can be called with `&tx` from inside the reconcile
transaction and with `&conn` from a hook. Use that rather than duplicating the
scanner or making it generic.

### 2. Public best-effort hook functions — `src/memory/index.rs`

Add four functions. Each opens the index itself, does its inserts, and returns
`anyhow::Result<()>` so the *call site* can log and swallow — matching
`index_memory_file`'s shape.

- `index_turn(session_id: &str, turn: usize, offset: u64, body: &str)`
- `index_event(segment: &str, offset: u64, event: &str, body: &str)`
- `index_epoch(session_id: &str, seq: u32, kind: &str, body: &str)`
- `index_artifact(kind: &str, name: &str, tags: &str, body: &str)` plus
  `remove_artifact(kind: &str, name: &str)`

`index_turn` and `index_event` insert the map row first, then the FTS row using
`last_insert_rowid()` — the same two-step 02b uses. **Apply
`crate::ai::mask_sensitive` to the body inside these functions**, so no caller
can forget; it is idempotent, so double-masking upstream-masked text is a no-op.

`index_artifact` must be **replace-not-append**: delete any existing row for
that `(kind, name)` before inserting, or repeated writes to one runbook
accumulate duplicate rows. `index_memory_file`'s delete-then-insert in one
transaction is the pattern.

### 3. Hook the archive append — `src/daemon/session.rs`

In `append_archive_message` (`:276`), the offset of the line about to be written
is **the archive file's length immediately before the append** — and that means
*after* the seeding copy, not before it:

```rust
// Seed: if the archive doesn't exist but the working file does, copy it.
if !archive_path.exists() && working_path.exists() {
    let _ = std::fs::copy(&working_path, &archive_path);
}
```

**Gotcha — the seeding copy is an index gap if you only hook the append.** That
`fs::copy` can drop *many* lines into the archive at once, and none of them pass
through the append path. If you index only the appended line, everything the seed
copied stays unsearchable until the next full reconcile, which breaks the
milestone's "searchable without a reindex" criterion. Handle it explicitly:

- Detect that the seed actually happened (the copy branch ran and succeeded).
- When it did, index the whole freshly-seeded file with the § 1 helper.
- Then compute the offset for the new line and index it.

Get the length with `std::fs::metadata(&archive_path).map(|m| m.len())`,
defaulting to skipping the index write if it fails — never unwrap. Compute it
**after** any seed and **before** opening the file for append.

Body text is the same shape 02b indexes: `msg.content` plus each
`tool_results[].content`. Skip messages whose `turn` is `None`, exactly as the
reconcile scanner does.

### 4. Hook the event append — `src/daemon/utils/event_log.rs`

`log_event` (`:10`) already builds the final `record` map and serialises it to
`line` before writing. Capture the segment file's length before the append, and
after a successful write index the row with `index_event`:

- `segment` label: `"legacy"` when the path equals `crate::config::events_path()`,
  otherwise the path's `file_stem` — identical to 02b, because 03b deletes by
  this label.
- `event` column: the event name already in hand.
- `body`: `crate::search::json_to_readable(&line)`, matching what 02b indexes and
  what the grep path matches.

`log_event` returns `()` and its doc comment says errors are silently discarded.
Keep that contract: log at `warn` and continue.

### 5. Hook epoch and artifact writes

- `append_epoch` (`src/daemon/context/epochs.rs:113`) — after a successful write,
  call `index_epoch` with the same body composition 02b's reconcile uses
  (narrative, each `failed_cmds` command, each artifact entry).
- `write_runbook` / `write_script` — call `index_artifact` after the file write
  succeeds. Tags: runbook tags come from the frontmatter the caller already has
  or from `list_runbooks()`; scripts may pass an empty tag string if no tag
  source is at hand — the body is what carries the search value.
- `delete_runbook` / `delete_script` — call `remove_artifact` after the file is
  removed.

**Do NOT call `crate::runbook::load_runbook()` anywhere in this work.** It bumps
`inc_runbooks_executed()` (`src/runbook.rs:190`) and would report phantom runbook
executions. This already bit phase 02a; read the file directly.

## Acceptance criteria

- [ ] Appending an archived message makes it searchable **without** a reconcile:
      call `append_archive_message` on a fresh session, then query `turns`
      directly and get the row.
- [ ] The appended row's `turns_map.offset` is correct — seek the archive to it
      and read back the same line. Assert by reading the file, not by reasoning.
- [ ] **The seeding case is covered.** With a working session file holding three
      messages and no archive yet, one `append_archive_message` produces **four**
      searchable turns rows (three seeded + one appended), and each offset seeks
      to its own line.
- [ ] `log_event` makes an event searchable immediately, with `segment` labelled
      as in 02b and the offset seeking to the right line.
- [ ] `append_epoch` makes an epoch narrative searchable immediately.
- [ ] Writing a runbook twice leaves **one** `artifacts` row for it, not two;
      deleting it leaves none.
- [ ] **Incremental and reconcile agree.** After exercising every hook, run
      `reconcile_index()` and assert the per-corpus counts are unchanged — the
      incremental path must not double-count or miss rows relative to a rebuild.
      This is the strongest single check in the phase; make it a test.
- [ ] **A failing index never breaks its caller.** With the index made
      unwritable, `append_archive_message` and `log_event` still write their file
      and return normally.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green, no existing test removed or `#[ignore]`d.

## Test plan

Use the home-guard convention already in each module (`crate::test_home_guard()`
plus a tempdir `HOME`).

- `append_archive_message_indexes_the_turn` — searchable with no reconcile.
- `appended_turn_offset_seeks_to_its_line` — the seek-and-compare check.
- `archive_seed_indexes_every_copied_line` — the four-row case from the
  acceptance criteria. **This is the test most likely to be skipped and the one
  that matters most**; without it the seed gap ships silently.
- `log_event_indexes_the_event` — plus segment label and offset seek.
- `append_epoch_indexes_the_narrative`.
- `rewriting_a_runbook_replaces_its_artifact_row` — write twice, assert one row.
- `deleting_a_runbook_removes_its_artifact_row`.
- `incremental_and_reconcile_agree` — exercise each hook, snapshot `per_corpus`,
  `reconcile_index()`, assert identical counts.
- `index_failure_does_not_break_append` — the best-effort guarantee. The existing
  `index_failure_does_not_fail_add_memory` test in `src/memory/index.rs` shows
  how this module already simulates an unusable index; follow it.

**Negative cases to pin** (each must NOT happen):

- A message with `turn: None` passed to `append_archive_message` must add **no**
  `turns` row, matching the reconcile scanner.
- Rewriting one runbook must not leave a stale duplicate row — assert the count
  is exactly 1, not merely ≥ 1.
- No hook may `?`-propagate an index error into its caller's return value. Pin
  by asserting the caller still succeeds with a broken index, not by inspection.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib memory::index 2>&1 > /tmp/phase03a-tests.txt; echo "exit=$?" >> /tmp/phase03a-tests.txt
cargo test --lib daemon::session 2>&1 >> /tmp/phase03a-tests.txt; echo "exit=$?" >> /tmp/phase03a-tests.txt
{ echo "--- load_runbook must not appear in index or hook paths ---";
  grep -rn "load_runbook" src/memory/index.rs src/daemon/session.rs src/daemon/utils/event_log.rs \
    || echo "OK: no load_runbook reference";
  echo "--- no ?-propagated index errors at hook call sites ---";
  grep -n "index_turn(\|index_event(\|index_epoch(\|index_artifact(\|remove_artifact(" \
    src/daemon/session.rs src/daemon/utils/event_log.rs src/daemon/context/epochs.rs \
    src/runbook.rs src/scripts.rs;
} > /tmp/phase03a-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase03a-checks.txt
```

The first file must show both test groups passing. The second must show no
`load_runbook` reference, and every hook call site wrapped in an
`if let Err(e) = …` rather than terminated with `?`.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **`docs/dev/WORKFLOW.md`
requires one such entry per dispatch** — an entry from an earlier round of this
phase does not carry forward, and the server-authored `(complete)` entry and its
"Command output tails" block never satisfy it. Do not summarise in prose instead
of pasting the captured files.

## Authorizations

- Modify: `src/memory/index.rs`, `src/daemon/session.rs`,
  `src/daemon/utils/event_log.rs`, `src/daemon/context/epochs.rs`,
  `src/runbook.rs`, `src/scripts.rs`.
- No new dependencies. No DDL or `SCHEMA_VERSION` change.
- Do **not** move, rename, or re-home any existing function beyond the single
  extraction § 1 asks for. If a helper looks misplaced, leave it and say so in
  the Update Log.

## Out of scope

- **Retention-sweep deletion** (`sweep_session_archives`, `sweep_event_segments`
  removing rows) — phase 03b.
- Any read surface: `recall_context`, `search_repository`, `fts5_search` are
  untouched.
- Connection pooling or a cached global connection — measured above as
  unnecessary.
- The `fs::copy` archive-seeding defect noted in the milestone README (a
  post-compaction seed captures synthetic slot0/slot1 head messages). This phase
  *indexes* whatever the seed copied; it does not change what the seed copies.
  If your tests surface it concretely, note it in the Update Log and it will be
  filed separately.

## Update Log

*(executor appends here)*
