# Phase 05a: search_repository on FTS — stemming and ranking for the four existing kinds

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
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

<!-- entries appended below this line -->
