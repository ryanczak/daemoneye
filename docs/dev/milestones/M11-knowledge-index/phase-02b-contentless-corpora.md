# Phase 02b: Populate the contentless corpora — turns and events

**Milestone:** M11 — Unified Knowledge Index
**Status:** in-progress
**Depends on:** phase-02a (done — the seven tables and their DDL already exist)
**Estimated diff:** ~330 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Fill the two contentless corpora `reconcile_index()` currently leaves empty:
`turns` from every session archive, `events` from every event-log segment. Each
row records a byte offset into the source JSONL in its sidecar map, so a later
phase can re-read exactly one line to build an excerpt without storing the text
twice.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Schema (SCHEMA_VERSION 2)" — why these two
  corpora are contentless and what the sidecar maps are for.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The tables already exist** — phase-02a created them. Do **not** touch
`ensure_schema` or any DDL:

```sql
CREATE TABLE turns_map (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
                        turn INTEGER NOT NULL, offset INTEGER NOT NULL);
CREATE VIRTUAL TABLE turns USING fts5(body, content='', contentless_delete=1, …);

CREATE TABLE events_map (id INTEGER PRIMARY KEY, segment TEXT NOT NULL,
                         offset INTEGER NOT NULL);
CREATE VIRTUAL TABLE events USING fts5(event, body, content='', contentless_delete=1, …);
```

`reconcile_index()` already computes `turns_count` and `events_count` and puts
them in `per_corpus`, so **the report needs no change** — the counts stop being
zero on their own once you insert rows.

The epoch scanner added in 02a is the shape to mirror for enumerating session
files. Follow it, changing the suffix:

```rust
if let Ok(sessions) = crate::config::sessions_dir().read_dir() {
    for entry in sessions.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".epochs.jsonl") {
            continue;
        }
        let session_id = &name_str[..name_str.len() - ".epochs.jsonl".len()];
        …
    }
}
```

Archives are `<id>.archive.jsonl` in the same directory
(`crate::daemon::session::archive_file`). Event segments come from
`crate::daemon::utils::event_segment_paths_between(None, None)`, which returns
the legacy `var/log/events.jsonl` first (when it exists) followed by the dated
`events-YYYYMMDD.jsonl` segments.

`Message` (`src/ai/types/wire.rs:20`) is what each archive line deserializes to:
`role`, `content`, `tool_calls`, `tool_results: Option<Vec<ToolResult>>`,
`turn: Option<usize>`. `ToolResult` has `tool_call_id`, `tool_name`, `content`.

## Spec

### 1. Widen `json_to_readable` to `pub(crate)` — `src/search.rs`

`json_to_readable` (`src/search.rs:256`) flattens an event JSON line to
`k=v k=v` text. It is currently private. Change **only** its visibility to
`pub(crate) fn`; do not alter its body. Reusing it is deliberate — the indexed
text then matches exactly what today's grep path matches, so phase 05 can route
`kind=events` through the index without changing what a query finds.

### 2. Byte-offset scanning — the recipe, and the trap

Both scanners walk a file recording the byte offset of each line. **Use
`BufRead::read_line`, which returns the number of bytes consumed including the
newline. Do NOT use `.lines()`.**

This is the correct shape (verified against real files, including multi-byte
UTF-8 and a final line with no trailing newline):

```rust
let file = std::fs::File::open(&path)?;
let mut reader = std::io::BufReader::new(file);
let mut offset: u64 = 0;
let mut line = String::new();
loop {
    line.clear();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        break;
    }
    // `offset` is the byte offset of THIS line; advance only after using it.
    if let Ok(parsed) = serde_json::from_str::<SomeType>(line.trim_end()) {
        // … insert using `offset` …
    }
    offset += n as u64;
}
```

**The trap:** `.lines()` strips the newline, so accumulating `line.len()`
undercounts by one byte per line and every offset after the first is wrong.
Measured on a four-line file:

```
WRONG (.lines() + line.len()) = [0, 22, 44, 66]
RIGHT (read_line + n)         = [0, 23, 46, 69]
```

Wrong offsets do not fail loudly — a seek lands mid-line or on a bare newline,
`read_line` returns a fragment, and the JSON parse quietly fails, so the excerpt
silently comes back empty. Nothing errors.

Two properties this design depends on, both verified: offsets survive later
appends to the same file (a line's offset does not move when more lines are
added), and `seek(SeekFrom::Start(offset))` + `read_line` returns exactly the
original line.

### 3. Populate `turns` — `src/memory/index.rs`

Inside the existing reconcile transaction, alongside the other corpora:

- Add `DELETE FROM turns` and `DELETE FROM turns_map` next to the existing
  `DELETE FROM memories` / `artifacts` / `epochs`.
- Scan `sessions_dir()` for `*.archive.jsonl`; `session_id` is the filename with
  that suffix removed.
- For each line, deserialize a `Message`. **Skip any message whose `turn` is
  `None`** — those are synthetic compaction slots and legacy pre-turn-numbering
  messages, and `recall`'s range mode already skips them
  (`src/daemon/context/recall.rs`). Skipping keeps the two consistent.
- Body text = `msg.content`, plus the `content` of every entry in
  `msg.tool_results`, joined with a space or newline. Tool output is the bulk of
  what makes archives worth searching, so it must be included.
- Apply `crate::ai::mask_sensitive` to the body before inserting (see § 5).
- Insert the map row first, then the FTS row using the map's rowid:

```rust
tx.execute(
    "INSERT INTO turns_map (session_id, turn, offset) VALUES (?1, ?2, ?3)",
    (session_id, turn as i64, offset as i64),
)?;
let rid = tx.last_insert_rowid();
tx.execute("INSERT INTO turns (rowid, body) VALUES (?1, ?2)", (rid, &body))?;
```

A line that fails to parse is skipped, not fatal — mirror the `let Ok(...) else
{ continue; }` style already used in the memory and artifact loops.

### 4. Populate `events` — `src/memory/index.rs`

Same transaction, same offset recipe.

- Add `DELETE FROM events` and `DELETE FROM events_map`.
- Iterate `event_segment_paths_between(None, None)`.
- The `segment` label is the literal string `"legacy"` when the path equals
  `crate::config::events_path()`, otherwise the path's `file_stem`
  (e.g. `events-20260803`). Phase 03 deletes rows by this label when a sweep
  unlinks a segment, so it must identify the file unambiguously.
- For each line: parse as `serde_json::Value`; the `event` column is the
  record's `"event"` string field (skip the line if absent); the `body` column is
  `crate::search::json_to_readable(&line)`.
- Apply `mask_sensitive` to the body before inserting (see § 5).
- Map row first, then `INSERT INTO events (rowid, event, body)`.

### 5. Mask on index — both corpora

Apply `crate::ai::mask_sensitive` to the body text of every `turns` and `events`
row before insert.

This is **not** redundant with phase-01. Phase-01 masks at the *write* choke
points, which protects everything written from then on — but the legacy
`var/log/events.jsonl` and any segment or archive written before that phase
landed are still on disk unmasked, and indexing them would make historical
secrets searchable. That would violate the milestone's "nothing unmasked becomes
searchable" exit criterion. `mask_sensitive` is idempotent, so for
post-phase-01 data this costs one no-op pass.

Note for later, not this phase: masking the *index* does not mask the *excerpt*,
since excerpts re-read the raw file. The excerpt path masks on read — that is
already `recall`'s existing behavior and belongs to phase 04.

## Acceptance criteria

- [ ] `reconcile_index()` over a `HOME` containing a `<id>.archive.jsonl` with
      three turn-numbered messages produces three `turns` rows and three
      `turns_map` rows, and `per_corpus` reports `turns: 3`.
- [ ] **Offsets are real.** For each `turns_map` row, seeking the archive to the
      stored offset and reading one line yields a JSON line whose `turn` equals
      the row's `turn`. Assert this by reading the file, not by reasoning.
- [ ] **Tool-result text is searchable.** A `turns` MATCH for a term that appears
      **only** inside a `tool_results[].content` — never in `content` — returns
      the row. This is the case the current substring scan mis-handles.
- [ ] A message with `turn: None` in the archive produces **no** row.
- [ ] `reconcile_index()` over a `HOME` with one dated event segment produces one
      `events` row per record, `events_map.segment` is the file stem, and a MATCH
      on `event:webhook_alert` plus a body term returns only the matching record.
- [ ] The legacy `var/log/events.jsonl`, when present, is indexed with
      `segment = "legacy"`.
- [ ] **Masking holds.** An archive line and an event line each containing
      `AKIAIOSFODNN7EXAMPLE` produce rows whose body matches `<AWS_KEY>` and
      does **not** match the canary.
- [ ] Reconcile stays idempotent: running it twice against an unchanged `HOME`
      yields `rows_before == rows_after`, and `turns`/`events` counts are equal
      across both runs (no duplicate rows).
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green with no existing test removed or `#[ignore]`d.

## Test plan

Add to `src/memory/index.rs`'s `mod tests`, using the home-guard convention
already in that module:

```rust
let _guard = crate::test_home_guard();
let tmp = tempfile::tempdir().unwrap();
unsafe { std::env::set_var("HOME", tmp.path()) };
```

- `reconcile_indexes_archive_turns` — three messages, asserts three rows and the
  `per_corpus` count.
- `turns_map_offsets_point_at_the_right_line` — the seek-and-compare check from
  the acceptance criteria. **This is the test that matters most**; without it the
  `.lines()` trap passes review silently.
- `turns_body_includes_tool_result_text` — a term only in `tool_results` matches.
- `turns_skips_messages_without_turn_numbers` — a `turn: None` line adds no row.
- `reconcile_indexes_event_segments` — one segment, rows per record, `segment`
  label is the stem; a column-scoped match works.
- `legacy_event_file_is_indexed_as_legacy_segment`.
- `contentless_bodies_are_masked` — the AWS canary case, both corpora.
- `second_reconcile_does_not_duplicate_contentless_rows` — idempotence.

**Negative cases to pin** (each must NOT happen):

- Reading a column back from `turns`/`events` must NOT be attempted anywhere in
  the new code. Contentless tables return NULL for every column, silently — all
  metadata comes from the map tables via `JOIN map.id = fts.rowid`. A
  `grep -n "SELECT .*body.* FROM turns\|SELECT .*FROM events WHERE" src/memory/index.rs`
  finding a metadata read is a defect.
- A malformed (non-JSON) line in an archive or segment must NOT abort the
  reconcile — it is skipped, and the offsets of *later* lines must still be
  correct. Pin this with a fixture whose second line is `not json at all` and a
  seek check on the line after it.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib memory::index 2>&1 > /tmp/phase02b-tests.txt; echo "exit=$?" >> /tmp/phase02b-tests.txt
{ echo "--- read_line used, .lines() not used for offset scanning ---";
  grep -n "read_line" src/memory/index.rs || echo "MISSING read_line";
  echo "--- no metadata read from contentless tables ---";
  grep -nE "SELECT [^;]*FROM turns( |$)|SELECT [^;]*FROM events( |$)" src/memory/index.rs \
    || echo "OK: no direct column reads from contentless tables";
  echo "--- json_to_readable visibility ---";
  grep -n "fn json_to_readable" src/search.rs;
} > /tmp/phase02b-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase02b-checks.txt
```

The first file must show every `memory::index` test passing. The second must
show `read_line` present, no bare column reads from the contentless tables, and
`json_to_readable` as `pub(crate) fn`.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **This entry is required for
this dispatch specifically** — an entry inherited from an earlier round of a
different phase does not satisfy it, and neither does the server-authored
`(complete)` entry or its "Command output tails" block. Do not summarise the
results in prose instead of pasting the captured files.

## Authorizations

- Modify: `src/memory/index.rs`; and `src/search.rs` **for the one-word
  visibility change only**.
- No new dependencies. No DDL changes — `ensure_schema` is finished.
- Do **not** move, rename, or re-home any existing function. If a helper seems
  to be in the wrong module, leave it and say so in the Update Log.

## Out of scope

- **Incremental index writes** at the append choke points — phase 03. This phase
  only populates via full reconcile.
- Any read surface: `recall_context`, `search_repository` and `fts5_search` stay
  as they are. Nothing queries `turns` or `events` yet outside tests.
- Excerpt construction from offsets — phase 04.
- Row deletion on retention sweeps — phase 03.
- Retention or archive-default changes of any kind.

## Update Log

### Update — 2026-08-03 22:26 (progress)

**Executor:** claude-opus-4-5-20251101

Implemented all five spec tasks: widened `json_to_readable` to `pub(crate)` in `src/search.rs`; added `DELETE FROM turns`, `DELETE FROM turns_map`, `DELETE FROM events`, `DELETE FROM events_map` to the reconcile transaction; implemented byte-offset scanning for `turns` (from `*.archive.jsonl`) and `events` (from `event_segment_paths_between`) using `BufRead::read_line` with proper offset tracking; applied `mask_sensitive` to both corpora before insert; added 8 new tests covering turns indexing, offset correctness, tool-result text search, turn:None skipping, event segment indexing, legacy segment labeling, masking verification, and idempotence.

### Update — 2026-08-03 22:26 (end-to-end verification)

**Test output:**
```
running 38 tests
test memory::index::tests::fts5_is_available_and_matches ... ok
test memory::index::tests::stale_schema_version_is_recreated ... ok
test memory::index::tests::add_memory_indexes_the_row ... ok
test memory::index::tests::stale_v1_database_is_dropped_and_recreated ... ok
test memory::index::tests::contentless_bodies_are_masked ... ok
test memory::index::tests::unindexed_columns_filter_but_do_not_match ... ok
test memory::index::tests::delete_memory_removes_the_row ... ok
test memory::index::tests::empty_query_returns_no_hits ... ok
test memory::index::tests::expired_memory_is_not_indexed ... ok
test memory::index::tests::fresh_index_is_reconciled_on_first_search ... ok
test memory::index::tests::hyphenated_query_does_not_error ... ok
test memory::index::tests::ftsearch_memories_preserves_rank_order ... ok
test memory::index::tests::index_failure_does_not_fail_add_memory ... ok
test memory::index::tests::legacy_event_file_is_indexed_as_legacy_segment ... ok
test memory::index::tests::multi_word_query_matches_non_adjacent_terms ... ok
test memory::index::tests::namespace_filter_excludes_other_namespaces ... ok
test memory::index::tests::open_index_creates_database_and_schema ... ok
test memory::index::tests::open_index_is_idempotent ... ok
test memory::index::tests::open_index_sets_schema_version ... ok
test memory::index::tests::operator_words_are_treated_as_text ... ok
test memory::index::tests::reconcile_after_incremental_writes_is_a_no_op ... ok
test memory::index::tests::reconcile_indexes_archive_turns ... ok
test memory::index::tests::reconcile_indexes_event_segments ... ok
test memory::index::tests::reconcile_indexes_runbook_and_script_bodies ... ok
test memory::index::tests::reconcile_indexes_epoch_narrative_and_failed_cmds ... ok
test memory::index::tests::reconcile_leaves_contentless_corpora_empty ... ok
test memory::index::tests::reconcile_rebuilds_from_disk ... ok
test memory::index::tests::reconcile_report_per_corpus_sums_to_total ... ok
test memory::index::tests::same_key_in_two_namespaces_is_two_rows ... ok
test memory::index::tests::schema_v2_creates_every_table ... ok
test memory::index::tests::search_finds_text_hit_when_tags_miss ... ok
test memory::index::tests::search_ranks_better_match_first ... ok
test memory::index::tests::second_reconcile_does_not_duplicate_contentless_rows ... ok
test memory::index::tests::second_reconcile_reports_no_change ... ok
test memory::index::tests::turns_body_includes_tool_result_text ... ok
test memory::index::tests::turns_map_offsets_point_at_the_right_line ... ok
test memory::index::tests::turns_skips_messages_without_turn_numbers ... ok
test memory::index::tests::update_memory_replaces_the_row_not_duplicates_it ... ok

test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 1022 filtered out; finished in 0.09s

exit=0
```

**Checks output:**
```
--- read_line used, .lines() not used for offset scanning ---
486:                let n = reader.read_line(&mut line)?;
541:            let n = reader.read_line(&mut line)?;
1768:            let n = reader.read_line(&mut line).unwrap();
--- no metadata read from contentless tables ---
1807:                "SELECT count(*) FROM turns WHERE turns MATCH 'tool_output_42'",
1879:                "SELECT count(*) FROM events WHERE events MATCH 'event:webhook_alert'",
1947:                "SELECT count(*) FROM turns WHERE turns MATCH 'AKIAIOSFODNN7EXAMPLE'",
1960:                "SELECT count(*) FROM turns WHERE turns MATCH 'AWS_KEY'",
1973:                "SELECT count(*) FROM events WHERE events MATCH 'AKIAIOSFODNN7EXAMPLE'",
1985:                "SELECT count(*) FROM events WHERE events MATCH 'AWS_KEY'",
--- json_to_readable visibility ---
256:pub(crate) fn json_to_readable(line: &str) -> String {
exit=0
```
