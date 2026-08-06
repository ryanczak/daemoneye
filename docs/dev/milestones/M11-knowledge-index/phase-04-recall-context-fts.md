# Phase 04: recall_context on FTS — ranked query mode, correct excerpts, cross-session scope

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
**Depends on:** phase-03b (done — the `turns` corpus is populated incrementally
and swept on retention)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Move `recall_context` query mode off the substring scan and onto BM25 over the
`turns` corpus, fix the two rendering defects that make its output misleading,
and add an opt-in `scope: "all"` that searches every session instead of just the
current one.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Read surfaces", item 1 — the settled shape
  of this change.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`src/daemon/context/recall.rs` reads the archive file directly in both modes.
`recall()` opens `archive_file(session_id)` and dispatches to `range_query` or
`query_search`. Three things are wrong, and **all three were reproduced on this
build** — the transcripts below are real, not sketches.

### Defect 1 — query mode excerpts the wrong field

`query_search` decides a message matched using `matches_content`, which checks
`msg.content` **and** every `tool_results[].content` (`recall.rs:176`). But it
then builds the excerpt from `msg.content` alone:

```rust
let excerpt = build_excerpt(&msg.content, &lower_q, EXCERPT_HALF);
```

When the query matched only a tool-result body, `build_excerpt` finds nothing,
falls back to byte 0, and returns the head of an unrelated string. Probe: a
message whose `content` is padding and whose tool result contains `KERNELPANIC`,
queried for `KERNELPANIC`:

```
turn 7 (assistant): AAAAAAAAAA padding padding padding BBBBBBBBBB
```

The matched text does not appear in the output at all. This is the exact case the
milestone exit criterion names.

### Defect 2 — range mode drops tool-result bodies

`range_query` renders only `msg.content` (`recall.rs:113`):

```rust
results.push(format!("turn {} ({}): {}", turn, msg.role, msg.content));
```

Probe: turn 3 with a tool result containing `OUTPUT_MARKER disk full`, recalled
by range `[3, 3]`:

```
turn 3 (assistant): ran the command
```

The command's actual output — usually the reason someone recalls a turn — is
gone.

### Defect 3 — an 8-match ceiling and no ranking

`const MAX_MATCHES: usize = 8;` (`recall.rs:13`), and `query_search` `break`s at
that count in **file order**. The first eight chronological substring hits win;
relevance never enters into it.

### What you are building on

`fts5_search` (`src/memory/index.rs:154`) is **memory-only** — its SQL is
`SELECT namespace, key, bm25(memories) FROM memories WHERE memories MATCH ?1`.
Do **not** try to reuse or generalise it. Write a separate turns search.

`turns` is contentless (`content=''`), so a hit gives you a rowid and nothing
else. `turns_map` is the sidecar: `id` (= the FTS rowid), `session_id`, `turn`,
`offset`. The excerpt comes from re-reading the archive line at `offset` — the
round-trip phases 02b/03a/03b built and pinned.

`build_match_expr` (`src/memory/index.rs`) already quotes each user term and
joins with `OR`; reuse it, because the caller passes a whole user phrase and an
unquoted phrase match would return nothing.

## Spec

### 1. `search_turns` — `src/memory/index.rs`

Add beside `fts5_search`:

```rust
pub struct TurnHit {
    pub session_id: String,
    pub turn: i64,
    pub offset: u64,
    pub score: f64,
}

pub fn search_turns(query: &str, limit: usize, session_id: Option<&str>) -> Vec<TurnHit>
```

Join the FTS table to its map and order by BM25, best first:

```sql
SELECT m.session_id, m.turn, m.offset, bm25(turns)
FROM turns t JOIN turns_map m ON m.id = t.rowid
WHERE turns MATCH ?1
ORDER BY bm25(turns)
LIMIT ?2
```

`bm25()` in SQLite returns a **negative** score where more negative is a better
match, so plain `ORDER BY bm25(turns)` ascending is already best-first — do not
add `DESC`.

When `session_id` is `Some`, add `AND m.session_id = ?3`. When `None`, search
every session.

**Best-effort, exactly like `fts5_search`:** any failure logs and returns an
empty `Vec`. Search degrading to "no hits" must never be fatal, and must never
`?`-propagate out.

### 2. Rewrite query mode — `src/daemon/context/recall.rs`

Replace `query_search`'s file scan with `search_turns`. For each hit, re-read
that one line from the archive at `hit.offset`, deserialize the `Message`, and
render one block. Delete `MAX_MATCHES`; the cap is now the `limit` argument.

**Choosing which field to excerpt — pin this exactly, it is the fix for Defect
1.** The FTS row's `body` concatenates `content` and every `tool_results[].content`,
so a hit does not say which field matched. Resolve it after re-reading:

1. Lowercase each whitespace-separated term of the query.
2. If `msg.content` contains any term → excerpt from `msg.content`.
3. Else, for each `tool_results[]` in order, if its `content` contains any term →
   excerpt from that body, and label the block so the source is visible.
4. Else (the match was stemming-only — e.g. query `restarting` matched indexed
   `restart`, so no literal substring exists) → excerpt from `msg.content` from
   its head. **This fallback must exist**; a stemmed hit is a real hit and must
   still render something rather than being dropped.

Keep `build_excerpt` and its ±`EXCERPT_HALF` char-space windowing as-is — it is
correct and multi-byte-safe. You are changing *what string is passed in*, not how
the window is computed.

### 3. Render tool results in range mode — `src/daemon/context/recall.rs`

In `range_query`, after the existing `turn N (role): content` line, append each
tool result's body. Keep the existing line format unchanged so current output
stays recognisable; add the bodies beneath it. Empty `tool_results` renders
exactly as today (no trailing blank lines, no empty label).

### 4. `scope` parameter — tool def, args, executor

- `src/ai/tools/defs.rs`: add an optional `scope` param to `recall_context`
  (`ParamTy::Str`), documented as `"current"` (default) or `"all"`.
- `src/ai/types/pending.rs`: add `scope` to the `RecallContext` variant.
- `src/ai/tools/args.rs`: default it to `"current"`.
- `src/daemon/context/recall.rs`: add `pub scope: Option<String>` to `RecallArgs`.
  Anything other than `"all"` — including `None` and an unrecognised string —
  means current-session. **Do not error on an unknown value**; silently scoping to
  the current session is the safe reading and matches how the other tools treat
  free-text enums.
- `src/daemon/executor/mod.rs`: pass it through in the `PendingCall::RecallContext`
  arm.

**Cross-session hits must be labeled with their session id**, otherwise a turn
number from another session is indistinguishable from one in this session and
actively misleads. Prefix those blocks — `[session <id>] turn 12 (user): …`.
Same-session hits keep the current unprefixed format.

**Range mode ignores `scope` entirely** — it is exact retrieval from one archive
by turn number, and turn numbers are only meaningful within a session. Do not
plumb `scope` into `range_query`.

## Acceptance criteria

- [ ] **Defect 1 fixed.** A message whose `content` does not contain the query but
      whose `tool_results[].content` does: query mode's output **contains the
      matched text**. Assert on the rendered string, not on the hit count.
- [ ] **Defect 2 fixed.** A range recall of a turn with a tool result renders the
      tool-result body. Assert the body text appears in the output.
- [ ] Query mode returns **more than 8** blocks when more than 8 turns match and
      the limit allows — the old ceiling is gone.
- [ ] Results are **BM25-ordered, not file-ordered**. Build a fixture where the
      best match is written *last* in the archive and assert it is rendered
      **first**. A test that only checks "all hits present" does not pin ranking.
- [ ] `scope: "all"` returns a hit from a **different** session, and that block is
      prefixed with its session id.
- [ ] **Default scope is current-session.** With two sessions holding the same
      query text, a default-scope recall returns **only** the current session's
      turn — assert the other session's text is **absent**, not merely that the
      current one is present.
- [ ] An unknown `scope` value (e.g. `"everything"`) behaves as `"current"` and
      does not error.
- [ ] A stemming-only match (query `restarting` against an indexed body
      containing `restart`) still renders a block rather than being dropped.
- [ ] Range mode is unaffected by `scope`.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green, no existing test removed or `#[ignore]`d.

## Test plan

Use the home-guard convention (`crate::test_home_guard()` plus a tempdir `HOME`).

**Fixture gotcha that will cost you a run if you miss it:** `ToolResult` requires
**three** fields — `tool_call_id`, `tool_name`, `content`. A fixture line omitting
`tool_name` fails to deserialize, the whole message is silently skipped, and your
test sees an empty result that looks like a code bug. Write fixtures as:

```json
{"role":"assistant","content":"ran it","turn":3,
 "tool_results":[{"tool_call_id":"t1","tool_name":"run_terminal_command","content":"OUTPUT_MARKER disk full"}]}
```

Tests:

- `query_excerpt_comes_from_the_matched_tool_result`
- `range_mode_renders_tool_result_bodies`
- `query_mode_returns_more_than_eight_matches`
- `query_results_are_bm25_ordered_not_file_ordered` — best match written last.
- `scope_all_finds_another_session_and_labels_it`
- `default_scope_excludes_other_sessions`
- `unknown_scope_value_behaves_as_current`
- `stemmed_only_match_still_renders_a_block`
- `range_mode_ignores_scope`

**Negative cases to pin** (each must NOT happen):

- Default scope must **not** leak another session's turns. Assert the foreign
  text is absent.
- A cross-session block must **not** render without its session-id prefix.
- Query mode must **not** drop a hit whose only match is a stemmed form.
- `search_turns` must **not** propagate an index error to its caller — assert the
  caller still returns normally with an unwritable index.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib daemon::context::recall > /tmp/phase04-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase04-tests.txt
cargo test --lib memory::index >> /tmp/phase04-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase04-tests.txt
{ echo "--- MAX_MATCHES ceiling is gone ---";
  grep -n "MAX_MATCHES" src/daemon/context/recall.rs || echo "OK: no MAX_MATCHES";
  echo "--- bm25 ordering is ascending (best-first), no DESC ---";
  grep -n -A3 "ORDER BY bm25(turns)" src/memory/index.rs;
  echo "--- search_turns is best-effort, returns Vec not Result ---";
  grep -n "pub fn search_turns" src/memory/index.rs;
} > /tmp/phase04-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase04-checks.txt
```

**Paste the contents of both files whole and unedited.** Do not retype test
names, do not trim the listing, and do not reconstruct it to match a count you
expect — read the files back and paste what is in them. A transcript whose test
names do not all exist in the tree fails `STANDARDS.md` §1 outright, and it is
checked at review by diffing the pasted names against a live run.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **`docs/dev/WORKFLOW.md` requires
one such entry per dispatch** — an earlier round's entry does not carry forward,
and the server-authored `(complete)` entry never satisfies it.

## Mutation check before reporting complete

Change the `ORDER BY bm25(turns)` to `ORDER BY m.turn` (file order), confirm
`query_results_are_bm25_ordered_not_file_ordered` **fails**, then restore it and
confirm it passes. State both results in your Update Log. A ranking test that
passes under file ordering is not testing ranking.

## Authorizations

- Modify: `src/memory/index.rs`, `src/daemon/context/recall.rs`,
  `src/ai/tools/defs.rs`, `src/ai/tools/args.rs`, `src/ai/types/pending.rs`,
  `src/daemon/executor/mod.rs`.
- Update `CLAUDE.md`'s `recall_context` tools-table row to mention `scope` —
  `tests/doc_truth.rs` cross-checks that table. Do **not** change the tool counts
  line; this phase adds no tool.
- No new dependencies. No DDL or `SCHEMA_VERSION` change.

## Out of scope

- **`search_repository`** — phase 05. Do not touch `src/search.rs`.
- **Prompt assembly / `assemble_turn_relevant_memory`** — phase 06.
- **The `LimitsConfig::default()` hardcode** at `src/daemon/executor/mod.rs:538`.
  The milestone README floated folding it here "if the diff stays small". It is
  not small: `SessionCtx` carries no config, so threading real limits means
  changing `execute_tool_call`'s signature and every call site. Leave the
  `LimitsConfig::default()` line exactly as it is; it is a separate phase.
- Epoch-corpus search and any new `recall_context` mode beyond `scope`.

## Update Log

<!-- entries appended below this line -->
