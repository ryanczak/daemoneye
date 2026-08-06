# Phase 05b: search_repository gains `turns` and `epochs` kinds

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
**Depends on:** phase-05a (done — the FTS routing scaffold and the
index-hit → file → `SearchResult` pattern this phase reuses)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Add two new `kind` values to `search_repository`: `turns` (conversation history
across sessions) and `epochs` (compaction narratives). Both corpora are already
populated and indexed; this phase only adds the read routing.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Read surfaces", item 2.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Phase 05a reworked `search_repository_with_namespaces` (`src/search.rs`) into a
`match kind` that dispatches to three helpers: `search_artifact_dir_fts`,
`search_memory_fts`, `search_events_fts`. Each takes the index hits, resolves
each hit to its source, and pushes `SearchResult`s. **Read `search_events_fts`
first — it is the closest analogue to what you are writing**, because like
`turns` it resolves a contentless hit through a `(file, offset)` pair.

`search_turns(query, limit, session_id) -> Vec<TurnHit>` already exists
(`src/memory/index.rs`, phase 04). `TurnHit` is
`{ session_id, turn, offset, score }`. Pass `None` for `session_id` here —
`search_repository` is not session-scoped.

**There is no `search_epochs` yet; you are adding it.** The `epochs` table is
**stored-content**, not contentless — unlike `turns` and `events` it holds its
own text and needs no file round-trip:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS epochs USING fts5(
    session_id UNINDEXED,
    seq        UNINDEXED,
    kind       UNINDEXED,
    body,
    tokenize = 'porter unicode61 remove_diacritics 2'
);
```

So `search_epochs` selects `body` directly and there is **no** offset, no file
read, and no `_map` table. Do not invent one.

**The 05a path-resolution bug, so you do not repeat it.** 05a's three helpers all
shipped with the same defect: an index hit was joined to a directory *without its
file extension*, `read_to_string` failed, and every hit was silently skipped —
producing empty results with no error. Runbooks are `<name>.md`, event segments
are `<stem>.jsonl`. **`turns` has the same hazard**: the archive file is
`archive_file(session_id)`, which is `<session_id>.archive.jsonl`, **not**
`<session_id>`. Use `crate::daemon::session::archive_file(&hit.session_id)` — do
not hand-build the path.

## Spec

### 1. `search_epochs` — `src/memory/index.rs`

```rust
pub struct EpochHit {
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub body: String,
    pub score: f64,
}

pub fn search_epochs(query: &str, limit: usize) -> Vec<EpochHit>
```

Select `session_id, seq, kind, body, bm25(epochs)` from `epochs`,
`ORDER BY bm25(epochs)` ascending (negative-is-better; **no `DESC`**),
`LIMIT ?`. Use `open_and_reconcile_if_empty("epochs")` — 05a's helper — and
`build_match_expr` for the query. Best-effort: log and return an empty `Vec` on
any failure; never `?`-propagate.

### 2. Two new routing arms — `src/search.rs`

Add `"turns"` and `"epochs"` arms to the `match kind` in
`search_repository_with_namespaces`, each delegating to a new helper beside the
existing three.

**`search_turns_fts`** — for each `TurnHit`:

- Resolve the archive with `crate::daemon::session::archive_file(&hit.session_id)`.
- Read the line at `hit.offset` (reuse `read_line_at_offset`, which 05a already
  added for events).
- Deserialize the `Message` and build the `matched_line` from `msg.content` plus
  each `tool_results[].content`, so a match that exists only in a tool result is
  visible — the same defect phase 04 fixed for `recall_context`.
- `kind` = `"turns"`, `name` = `format!("{} turn {}", hit.session_id, hit.turn)`
  so the session is identifiable in the output.
- `line_number` = 1. A JSONL line has no meaningful line number; do not fake one.
- A line that fails to read or deserialize is logged and skipped, never
  `?`-propagated.

**`search_epochs_fts`** — for each `EpochHit`, push one `SearchResult` with
`kind` = `"epochs"`, `name` = `format!("{} epoch {}", hit.session_id, hit.seq)`,
`matched_line` = the stored `body`, `line_number` = 1. No file access at all.

Both respect `MAX_RESULTS` and preserve rank order (best first).

**Context lines:** neither corpus has surrounding lines to show —
`context_before` / `context_after` are empty vectors. Do not fabricate context by
slicing the body.

### 3. `"all"` does **not** gain these kinds

Leave the `"all"` arm exactly as it is. `turns` and `epochs` are large,
conversational, and would swamp a general `all` search that today returns
curated knowledge (memory, runbooks, scripts, events). They are opt-in by
explicit `kind`. **Do not add them to `"all"`.**

### 4. Tool definition — `src/ai/tools/defs.rs`

Extend `search_repository`'s `kind` description to list `'turns'` and
`'epochs'`, and say in the tool description that both are opt-in and not
included in `'all'`. Do not add or rename params.

## Acceptance criteria

- [ ] `kind="turns"` finds an archived turn by free text and the result's
      `name` contains both the session id and the turn number.
- [ ] **A turn matching only inside a `tool_results` body is found and its
      matched text is visible in the output** — the phase-04 defect must not
      reappear here.
- [ ] `kind="epochs"` finds an epoch narrative by free text, with `name`
      containing the session id and seq.
- [ ] **Both are rank-ordered.** Build a fixture where the best match is written
      *last* and assert it is returned **first**, for each kind separately.
- [ ] **`"all"` does NOT include turns or epochs.** With a turn and an epoch both
      matching the query, a `kind="all"` search returns **neither** — assert
      their absence explicitly, not merely that other kinds are present.
- [ ] A `turns` hit whose archive file is missing is skipped without panicking
      and without failing the whole search.
- [ ] `MAX_RESULTS` still caps the total for both new kinds.
- [ ] **A failing index never breaks the tool.** With the index unwritable, both
      new kinds return empty and do not panic or propagate.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green, no existing test removed or `#[ignore]`d.

## Test plan

Use the home-guard convention. `src/search.rs`'s 05a tests are the fixture model.

**Fixture gotchas that will cost you a run:**

- `ToolResult` requires **three** fields — `tool_call_id`, `tool_name`,
  `content`. Omitting `tool_name` makes the whole message fail to deserialize and
  silently vanish, which looks like a code bug.
- Populate `turns` through `crate::memory::index::index_turn(...)` and epochs
  through `index_epoch(...)` — the real hooks — rather than hand-writing SQL. A
  hand-written `INSERT` into a contentless FTS table with the wrong column set or
  insert order produces rows that never match. This exact mistake made 05a's
  events test fail.
- The archive file must be written at the offset you index, so write the file
  first and index the byte offset you actually used.

Tests:

- `turns_kind_finds_archived_turn`
- `turns_hit_shows_tool_result_text`
- `epochs_kind_finds_narrative`
- `turns_results_are_rank_ordered`
- `epochs_results_are_rank_ordered`
- `all_kind_excludes_turns_and_epochs`
- `turns_hit_with_missing_archive_is_skipped`
- `new_kinds_survive_unwritable_index`

**Negative cases to pin** (each must NOT happen):

- `kind="all"` must **not** return turns or epochs — assert absence.
- A turn matching only in a tool result must **not** render without that text.
- A missing archive file must **not** panic or abort the search.
- Neither helper may `?`-propagate an index or IO error to its caller.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib search > /tmp/phase05b-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05b-tests.txt
cargo test --lib memory::index >> /tmp/phase05b-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05b-tests.txt
{ echo "--- search_epochs is best-effort (returns Vec) ---";
  grep -n "pub fn search_epochs" src/memory/index.rs;
  echo "--- archive path built via archive_file(), not hand-joined ---";
  grep -n "archive_file" src/search.rs;
  echo "--- all-arm must NOT mention turns/epochs helpers ---";
  sed -n '/"all" => {/,/}/p' src/search.rs;
} > /tmp/phase05b-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase05b-checks.txt
```

**Paste the contents of both files whole and unedited.** Read the files back and
paste what is in them. Do not type test names from memory and do not reconstruct
a listing to match a count you expect — at review the pasted names are diffed
against a live run, and any name that does not exist in the tree fails
`STANDARDS.md` §1 outright.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **`docs/dev/WORKFLOW.md` requires
one such entry per dispatch.**

## Mutation check before reporting complete

Add `search_turns_fts` to the `"all"` arm, confirm
`all_kind_excludes_turns_and_epochs` **fails**, then remove it and confirm it
passes. State both results in your Update Log. The `"all"` exclusion is a
deliberate design choice and needs a test that actually holds it.

## Authorizations

- Modify: `src/memory/index.rs`, `src/search.rs`, `src/ai/tools/defs.rs`.
- No new dependencies. No DDL or `SCHEMA_VERSION` change.
- Do **not** change `SearchResult`'s fields, `format_results`, or
  `search_repository`'s parameter list.
- Do **not** modify the four existing kinds' behavior — 05a is approved and done.

## Out of scope

- **Prompt assembly / `assemble_turn_relevant_memory`** — phase 06.
- `recall_context` — phase 04, done.
- Adding turns/epochs to `"all"` — explicitly rejected above.

## Update Log

<!-- entries appended below this line -->
