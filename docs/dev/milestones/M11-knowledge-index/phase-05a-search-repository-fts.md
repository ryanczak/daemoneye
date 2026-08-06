# Phase 05a: search_repository on FTS — stemming and ranking for the four existing kinds

**Milestone:** M11 — Unified Knowledge Index
**Status:** done
**Depends on:** phase-04 (done — `search_turns` established the
index-search + offset-round-trip shape this phase reuses)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Route `search_repository`'s four existing kinds — `memory`, `runbooks`,
`scripts`, `events` — through the FTS5 index so they gain stemming and BM25
ranking, while keeping the tool's current output shape (line-level matches with
surrounding context) and its filename-match behavior. The new kinds `turns` and
`epochs` are **phase 05b**.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Read surfaces", item 2.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`search_repository_with_namespaces` (`src/search.rs:28`) builds a list of
`(dir, kind_label)` pairs from `kind`, then `search_dir` walks each directory
doing a **case-insensitive substring** scan, emitting one `SearchResult` per
matching line:

```rust
pub struct SearchResult {
    pub kind: String,
    pub name: String,
    pub line_number: usize,
    pub matched_line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}
```

`search_dir` also matches on **filename** (`src/search.rs`, the `stem` /
`file_name` check) — a file whose stem contains the query is a hit even when its
body does not. Events are handled separately by `search_events_in_segments`.
`MAX_RESULTS = 50`.

**The substring scan is why `search_repository` misses obvious things.**
Executed on this build against an `artifacts` row whose body is
`"the service must restart cleanly"`:

```
PROBE05 query=restarting   expr="restarting"   hits=1
PROBE05 query=restarted    expr="restarted"    hits=1
PROBE05 query=restart      expr="restart"      hits=1
PROBE05 literal substring 'restarting' in body = false
```

FTS finds the document for every inflection; the literal substring finds it for
none but the exact one. That gap is the whole point of this phase — **and it is
also the trap**, see § 2 below.

### What exists to build on

`src/memory/index.rs` currently exposes exactly two searches:

- `fts5_search(query, limit, namespaces) -> Vec<(namespace, key, score)>` — memories.
- `search_turns(query, limit, session_id) -> Vec<TurnHit>` — turns (phase 04).

There is **no** artifact or event search yet; you are adding them. Copy
`search_turns`'s shape (phase 04, `src/memory/index.rs:250`): best-effort,
returns a `Vec`, logs and returns empty on any failure, never `?`-propagates.

`build_match_expr` quotes each user term and joins with `OR` — reuse it.
`bm25()` is negative-is-better, so `ORDER BY bm25(<table>)` ascending is
already best-first; **do not** add `DESC`.

## Spec

### 1. Two new index searches — `src/memory/index.rs`

```rust
pub struct ArtifactHit { pub kind: String, pub name: String, pub score: f64 }
pub struct EventHit { pub segment: String, pub offset: u64, pub event: String, pub score: f64 }

pub fn search_artifacts(query: &str, limit: usize, kind: Option<&str>) -> Vec<ArtifactHit>
pub fn search_events(query: &str, limit: usize) -> Vec<EventHit>
```

`search_artifacts` selects `kind, name, bm25(artifacts)` from `artifacts`,
filtered by `kind = ?` when `Some` (`"runbook"` / `"script"` — the singular
labels `index_artifact` stores, **not** the plural tool-facing `"runbooks"`).

`search_events` joins `events` to `events_map` on `m.id = e.rowid` and returns
`segment`, `offset`, `event`, score — the same map-join shape `search_turns`
uses.

### 2. Route the four kinds through the index — `src/search.rs`

Rework `search_repository_with_namespaces` so the index chooses **which
documents match, and in what order**, and the existing line scan then produces
the `SearchResult` rows for those documents only.

For `memory` / `runbooks` / `scripts`:

1. Call the index search to get ranked hits.
2. Resolve each hit to its file path (memory: `memory_dir_for_namespace` +
   category + `<key>.md`; artifacts: `runbooks/<name>.md`, `scripts/<name>`).
3. Scan **that one file** for matching lines, emitting `SearchResult`s with the
   existing `line_number` / `matched_line` / context fields.
4. Preserve rank order across files — a better-ranked document's rows come first.

For `events`: `search_events` gives `(segment, offset)`. Re-read that one line at
`offset` from the segment file and emit one `SearchResult` with
`line_number` = 1 and the readable body as `matched_line`. This is the same
offset round-trip phases 02b–04 use; a per-line read error is logged and skipped,
never `?`-propagated.

**THE TRAP — a stemmed hit has no literal substring, so the line scan finds
nothing.** The probe above proves it: `restarting` matches a body containing only
`restart`, and `body.contains("restarting")` is `false`. If you scan for the raw
query and emit nothing when it misses, this phase **makes search worse** — the
index found the document and you threw it away, and the milestone's headline
criterion ("'restarting' finds a runbook containing 'restart'") fails while every
naive test still passes.

Required behavior: when a document is an index hit but **no line matches
literally**, still emit one `SearchResult` for it — the document's first
non-empty line as `matched_line`, `line_number` = that line's number, context
per the usual rule. A stemmed hit is a real hit.

**Filename matching is preserved and is independent of the index.** A file whose
stem or filename contains the query is still a hit even when the index does not
match its body. Keep that behavior for `runbooks` / `scripts` / `memory`; the
index adds hits, it does not remove them. De-duplicate so a file that is both a
filename match and an index hit does not appear twice.

`MAX_RESULTS = 50` still caps the total. `kind = "all"` keeps its current
meaning (memory + runbooks + scripts + events).

**Do not change `SearchResult`'s fields, `format_results`, or the tool's
parameter list.** This phase changes which documents are found and their order —
not the rendering. Phase 05b adds the new kinds.

### 3. Tool description — `src/ai/tools/defs.rs`

Update `search_repository`'s description to say matching is stemmed and results
are relevance-ranked. Do **not** add or rename params, and do **not** touch
`deferred_group`.

## Acceptance criteria

- [ ] **The headline criterion**: a runbook whose body contains `restart` (and
      **not** the literal string `restarting`) is found by a `kind="runbooks"`
      search for `restarting`, and the returned `SearchResult` is non-empty.
- [ ] The same holds for `memory` and for `scripts`.
- [ ] **A stemmed-only hit still renders a line.** Assert `matched_line` is
      non-empty for the case above — this is the guard against the § 2 trap.
- [ ] `kind="events"` finds a `webhook_alert` record by free text.
- [ ] **Results are rank-ordered, not directory-walk-ordered.** Build a fixture
      where the best-matching document sorts *last* alphabetically and assert its
      rows come **first**. A test that only asserts "both found" does not pin
      ranking.
- [ ] **Filename matching still works and is not double-counted.** A file whose
      stem contains the query but whose body does not is still returned; and a
      file matching *both* ways appears **exactly once**, not twice.
- [ ] **A non-matching document is absent.** Assert a decoy file's name does
      **not** appear in the results — not merely that the wanted one does.
- [ ] `MAX_RESULTS` still caps the total at 50.
- [ ] **A failing index never breaks the tool.** With the index unwritable,
      `search_repository_with_namespaces` still returns normally (filename
      matches may still appear); it must not panic or propagate.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green, no existing test removed or `#[ignore]`d.

## Test plan

Use the home-guard convention (`crate::test_home_guard()` plus a tempdir `HOME`).
`src/search.rs` already has tests (`search_respects_kind_filter` and others near
`:402`) — follow their fixture style.

- `stemmed_query_finds_runbook_with_root_word` — the headline case.
- `stemmed_hit_renders_a_non_empty_matched_line` — the trap guard.
- `stemmed_query_finds_memory_entry`
- `stemmed_query_finds_script`
- `events_kind_finds_webhook_alert_by_free_text`
- `results_are_rank_ordered_not_alphabetical` — best match named last alphabetically.
- `filename_match_still_returned_without_body_match`
- `file_matching_name_and_body_appears_once`
- `non_matching_document_is_absent`
- `search_survives_unwritable_index`

**Negative cases to pin** (each must NOT happen):

- A stemmed hit must **not** be dropped for lack of a literal substring.
- A file matching both by name and by index must **not** appear twice — assert
  the count is exactly 1, not ≥ 1.
- A decoy document must **not** appear at all.
- An index failure must **not** propagate out of `search_repository_with_namespaces`.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib search > /tmp/phase05a-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05a-tests.txt
cargo test --lib memory::index >> /tmp/phase05a-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05a-tests.txt
{ echo "--- new index searches are best-effort (return Vec, not Result) ---";
  grep -n "pub fn search_artifacts\|pub fn search_events" src/memory/index.rs;
  echo "--- bm25 ordering ascending, no DESC ---";
  grep -n "ORDER BY bm25" src/memory/index.rs;
  echo "--- SearchResult fields unchanged ---";
  grep -n -A8 "pub struct SearchResult" src/search.rs;
} > /tmp/phase05a-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase05a-checks.txt
```

**Paste the contents of both files whole and unedited.** Do not retype test
names, do not trim the listing, and do not reconstruct it to match a count you
expect — read the files back and paste what is in them. At review the pasted
names are diffed against a live run; any name that does not exist in the tree
fails `STANDARDS.md` §1 outright.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **`docs/dev/WORKFLOW.md` requires
one such entry per dispatch** — an earlier round's entry does not carry forward,
and the server-authored `(complete)` entry never satisfies it.

## Mutation check before reporting complete

Delete the stemmed-hit fallback (the branch that emits a `SearchResult` when an
index-hit document has no literally matching line), confirm
`stemmed_hit_renders_a_non_empty_matched_line` **fails**, then restore it and
confirm it passes. State both results in your Update Log. That fallback is the
one place this phase can silently make search worse, so a test that passes
without it is not testing it.

## Authorizations

- Modify: `src/memory/index.rs`, `src/search.rs`, `src/ai/tools/defs.rs`.
- No new dependencies. No DDL or `SCHEMA_VERSION` change.
- Do **not** change `SearchResult`'s fields, `format_results`, or
  `search_repository`'s parameter list.

## Out of scope

- **New kinds `turns` and `epochs`** — phase 05b. Do not add them here.
- **Prompt assembly / `assemble_turn_relevant_memory`** — phase 06.
- `recall_context` — done in phase 04, do not revisit.
- The `_ =>` arm that currently defaults an unknown `kind` to runbooks. It is
  odd, but changing it is a behavior change no criterion here covers; leave it
  and note it in the Update Log if it gets in your way.

## Update Log

### Update — 2026-08-06 04:18 (started)

**Executor:** Claude (Sonnet 4.5)
**Status:** implementing phase 05a — FTS routing for search_repository's four existing kinds

<!-- entries appended below this line -->

### Update — 2026-08-05 (escalation)

**Chosen lever:** resume (`continue_phase`)
**Rationale:** the spec held — the executor built the whole design correctly
(`+134` in `src/memory/index.rs`, `+375/-32` in `src/search.rs`, 1/3 tasks
complete) and hard-failed at `stage verify` on **eight instances of one trivial
borrow mistake**, not on any design question.

**Verified state of the failed run's working tree** (dirty, uncommitted):

`cargo build` fails with 8 × `E0308`, every one the same shape — an owned value
passed where a borrow is expected at the new call sites:

```
src/search.rs:45  search_artifact_dir_fts(dir, kind_label, query, query_lower, …)
                    dir:         expected `&Path`, found `PathBuf`
                    query_lower: expected `&str`,  found `String`
src/search.rs:51  search_memory_fts(query, query_lower, …)   expected `&str`, found `String`
src/search.rs:54  search_events_fts(query, query_lower, …)   expected `&str`, found `String`
src/search.rs:58  search_memory_fts(…)                       expected `&str`, found `String`
src/search.rs:61  search_artifact_dir_fts(…)                 &Path / &str
```

No design defect, no missing example, no spec ambiguity — a refined re-dispatch
would re-derive work that is already correct on disk. The remaining edit is
adding `&` / `.as_path()` at roughly eight call sites.

### Update — 2026-08-05 (escalation 2)

**Chosen lever:** resume (`continue_phase`), assist 2 of 3
**Rationale:** assist 1 achieved its stated goal — the 8 `E0308` borrow errors are
gone, `cargo build` and `cargo clippy` are clean, and the tool-description task
landed. The run then stalled *again* (turn 214, `tool:read_file`, diff frozen
since turn 167) on a **new** problem it could not diagnose. This executor applies
a stated fix reliably and stalls when it has to diagnose, so the fix is to hand it
the diagnosis, not to take the phase over.

**Diagnosis — 7 failing tests, one root cause, and it is a real production bug.**
All seven fail with an empty result set, and **two of them are pre-existing tests**
(`search_finds_match_in_runbooks`, `search_respects_kind_filter`), so this is a
regression, not just unmet new criteria:

```
stemmed_query_finds_runbook_with_root_word  … Results: []
search_finds_match_in_runbooks              … assertion failed: !results.is_empty()
search_respects_kind_filter                 … assertion failed: !results.is_empty()
```

Root cause, verified by inspection of all four index searches:

```
fts5_search       → reconciles when `SELECT count(*) FROM memories` is 0
search_artifacts  → NO reconcile-on-empty
search_events     → NO reconcile-on-empty
search_turns      → NO reconcile-on-empty
```

`fts5_search` rebuilds the index when its corpus is empty; the three newer
searches do not. Now that `search_repository` routes *through* those searches, a
fresh or empty index makes it return nothing at all. This is not a test-fixture
artifact — on a fresh install, or after any `SCHEMA_VERSION` bump drops and
recreates the DB, the first `search_repository` for runbooks would return empty
until something else happened to trigger a reconcile.

**It also reveals a latent gap in phase 04.** `search_turns` has the same hole, so
`recall_context` query mode returns no hits on a fresh index. Phase 04's spec never
required reconcile-on-empty and its criteria all passed, so this is not a defect
against that phase — but leaving it knowingly broken would be shipping a bug, so
this assist authorizes fixing all four searches through one shared helper.

### Update — 2026-08-05 (architect takeover)

**Executor:** Claude (direct) — takeover after 2 assists on the same stall class.

**What the executor had right and I kept:** `search_artifacts` / `search_events`
and their SQL, the FTS routing in `search.rs`, the stemmed-hit fallback, the
`open_and_reconcile_if_empty` helper wired into all four searches, and the tool
description. None of that was rewritten.

**What I fixed — three bugs, two of them one class.** An index hit was resolved to
a path without its extension, so `read_to_string` failed and every hit was
silently skipped:

1. `search_artifact_dir_fts` joined `hit.name` bare. Runbooks are stored as
   `<name>.md` (`runbook.rs:186`), scripts as `<name>` with no extension
   (`scripts.rs:69`). Now branches on `kind_label`. This alone fixed 5 of 7
   failures.
2. `search_events_fts` joined `hit.segment` bare. The indexed label is the file
   *stem*; the file is `<stem>.jsonl`. Now formats the extension.
3. De-dup key mismatch: `index_hit_names` stores the bare artifact name but the
   filename branch compared `path.file_name()` (with extension), so a file hit
   both ways was emitted twice. Now compares against the stem.

**Two test defects, one of them mine.**

- `events_kind_finds_webhook_alert_by_free_text` was malformed: it wrote the
  segment as `events-2026-01-01` (real segments are `events-YYYYMMDD.jsonl`),
  inserted only the `event` column leaving `body` empty, and reversed the
  map/FTS insert order the schema requires. Rewritten to go through the real
  `index_event` hook.
- `file_matching_name_and_body_appears_once` asserted a count of 1 against a
  fixture with **two** matching body lines. **That was my spec error, not the
  executor's** — my acceptance criterion said "appears exactly once" without
  accounting for the pre-existing one-result-per-matching-line semantics, which
  `search_dir` has always had. Fixed the fixture to have exactly one matching
  body line so the assertion tests de-dup rather than line counting.

**Mutation check (run, not claimed).** Disabling the stemmed-hit fallback:

```
test search::tests::stemmed_hit_renders_a_non_empty_matched_line ... FAILED
  panicked at src/search.rs:947
```

Restored → `ok. 1 passed`. The fallback is genuinely load-bearing.

### Update — 2026-08-05 (end-to-end verification)

Captured mechanically; both files pasted whole.

**/tmp/phase05a-tests.txt:**

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 30 tests
test ai::types::pending::tests::summary_search_repository_truncated ... ok
test manifest::tests::auto_search_empty_on_no_match ... ok
test manifest::tests::auto_search_deduplicates ... ok
test manifest::tests::auto_search_follows_relates_to_links ... ok
test manifest::tests::auto_search_matches_runbook_name ... ok
test manifest::tests::auto_search_matches_memory_key ... ok
test manifest::tests::auto_search_max_three_items ... ok
test manifest::tests::auto_search_matches_memory_tags ... ok
test memory::index::tests::ftsearch_memories_preserves_rank_order ... ok
test manifest::tests::auto_search_respects_4kb_cap ... ok
test manifest::tests::auto_search_matches_summary_text ... ok
test manifest::tests::auto_search_matches_runbook_tag ... ok
test memory::index::tests::fresh_index_is_reconciled_on_first_search ... ok
test search::tests::events_kind_finds_webhook_alert_by_free_text ... ok
test memory::index::tests::search_ranks_better_match_first ... ok
test memory::index::tests::search_finds_text_hit_when_tags_miss ... ok
test search::tests::memory_search_dirs_label_incidents_plural ... ok
test search::tests::file_matching_name_and_body_appears_once ... ok
test search::tests::filename_match_still_returned_without_body_match ... ok
test search::tests::non_matching_document_is_absent ... ok
test search::tests::results_are_rank_ordered_not_alphabetical ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test search::tests::search_survives_unwritable_index ... ok
test search::tests::stemmed_hit_renders_a_non_empty_matched_line ... ok
test search::tests::stemmed_query_finds_memory_entry ... ok
test search::tests::stemmed_query_finds_script ... ok
test search::tests::stemmed_query_finds_runbook_with_root_word ... ok

test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 1073 filtered out; finished in 1.13s

exit=0
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 52 tests
test memory::index::tests::fts5_is_available_and_matches ... ok
test memory::index::tests::add_memory_indexes_the_row ... ok
test memory::index::tests::archive_seed_indexes_every_copied_line ... ok
test memory::index::tests::appended_turn_offset_seeks_to_its_line ... ok
test memory::index::tests::delete_memory_removes_the_row ... ok
test memory::index::tests::append_archive_message_indexes_the_turn ... ok
test memory::index::tests::append_epoch_indexes_the_narrative ... ok
test memory::index::tests::contentless_bodies_are_masked ... ok
test memory::index::tests::deleting_a_runbook_removes_its_artifact_row ... ok
test memory::index::tests::empty_query_returns_no_hits ... ok
test memory::index::tests::expired_memory_is_not_indexed ... ok
test memory::index::tests::fresh_index_is_reconciled_on_first_search ... ok
test memory::index::tests::hyphenated_query_does_not_error ... ok
test memory::index::tests::ftsearch_memories_preserves_rank_order ... ok
test memory::index::tests::stale_schema_version_is_recreated ... ok
test memory::index::tests::stale_v1_database_is_dropped_and_recreated ... ok
test memory::index::tests::incremental_and_reconcile_agree ... ok
test memory::index::tests::index_failure_does_not_break_append ... ok
test memory::index::tests::index_failure_does_not_fail_add_memory ... ok
test memory::index::tests::unindexed_columns_filter_but_do_not_match ... ok
test memory::index::tests::index_failure_does_not_break_log_event ... ok
test memory::index::tests::legacy_event_file_is_indexed_as_legacy_segment ... ok
test memory::index::tests::invalid_utf8_file_does_not_abort_reconcile ... ok
test memory::index::tests::log_event_indexes_the_event ... ok
test memory::index::tests::log_event_offset_seeks_to_its_line ... ok
test memory::index::tests::multi_word_query_matches_non_adjacent_terms ... ok
test memory::index::tests::message_without_turn_is_not_indexed ... ok
test memory::index::tests::malformed_line_is_skipped_and_later_offsets_stay_correct ... ok
test memory::index::tests::open_index_creates_database_and_schema ... ok
test memory::index::tests::namespace_filter_excludes_other_namespaces ... ok
test memory::index::tests::open_index_is_idempotent ... ok
test memory::index::tests::open_index_sets_schema_version ... ok
test memory::index::tests::operator_words_are_treated_as_text ... ok
test memory::index::tests::reconcile_after_incremental_writes_is_a_no_op ... ok
test memory::index::tests::reconcile_indexes_archive_turns ... ok
test memory::index::tests::reconcile_indexes_epoch_narrative_and_failed_cmds ... ok
test memory::index::tests::reconcile_indexes_event_segments ... ok
test memory::index::tests::reconcile_indexes_runbook_and_script_bodies ... ok
test memory::index::tests::reconcile_leaves_contentless_corpora_empty ... ok
test memory::index::tests::reconcile_rebuilds_from_disk ... ok
test memory::index::tests::reconcile_report_per_corpus_sums_to_total ... ok
test memory::index::tests::rewriting_a_runbook_replaces_its_artifact_row ... ok
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

test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 1051 filtered out; finished in 0.11s

exit=0
```

**/tmp/phase05a-checks.txt:**

```
--- new index searches are best-effort (return Vec, not Result) ---
339:pub fn search_artifacts(query: &str, limit: usize, kind: Option<&str>) -> Vec<ArtifactHit> {
401:pub fn search_events(query: &str, limit: usize) -> Vec<EventHit> {
--- bm25 ordering ascending, no DESC ---
206:         ORDER BY bm25(memories) LIMIT ?2"
271:             ORDER BY bm25(turns)
280:             ORDER BY bm25(turns)
354:             ORDER BY bm25(artifacts)
362:             ORDER BY bm25(artifacts)
414:               ORDER BY bm25(events)
--- SearchResult fields unchanged ---
4:pub struct SearchResult {
5-    pub kind: String,
6-    pub name: String,
7-    pub line_number: usize,
8-    pub matched_line: String,
9-    pub context_before: Vec<String>,
10-    pub context_after: Vec<String>,
11-}
12-
exit=0
```

### Review verdict — 2026-08-05

- **Verdict:** escalated (architect takeover)
- **Bounces:** 0 reviews; 2 assists + 3 `hard_fail`s, all `NoProgressStall`
- **Executor:** Claude (direct) — Qwen/Qwen3.6-27B-FP8 for the bulk of the code
- **Scope deviations:** one authorized — `search_turns` gained reconcile-on-empty
  (a latent phase-04 gap; leaving it knowingly broken was not acceptable)
- **Calibration:** see below

All four gates green: `cargo fmt --all --check` (0), `cargo build` (0),
`cargo clippy --all-targets --all-features -- -D warnings` (0), `cargo test`
(1103 passed, 0 failed).

**Calibration — the takeover was called one assist early, deliberately.**
`max_assists` is 3 and only 2 were spent. Assist 1 succeeded at its stated goal
(8 compile errors → 0). Assist 2 carried a complete written diagnosis and the
executor still stalled — which disproved the working theory that this executor
reliably applies a stated fix. A third identical attempt had no new information
behind it, so the honest move was to take over rather than spend the budget to
reach the same conclusion.

**Calibration — my assist-2 diagnosis was wrong, and that cost a run.** I
identified missing reconcile-on-empty as the root cause. That was a real bug and
the fix was correct, but it was **not** why the tests failed: the failing test
calls `index_artifact` directly, so its corpus was never empty. The actual cause
was the missing `.md`. I asserted a root cause from reading code instead of
running the failing test first — the same "execute, don't assert" rule this
milestone keeps re-earning, applied to diagnosis rather than to specs.
