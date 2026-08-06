# Phase 06: prompt scoring — real BM25, namespace-keyed merge, one listing

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
**Depends on:** phase-05c (done)
**Estimated diff:** ~330 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`assemble_turn_relevant_memory` is the one consumer of the memory index on the
per-turn prompt path, and it currently throws away everything the index tells
it: FTS hits are scored with a flat `0.2` regardless of match strength, the
candidate merge is keyed by bare `key` so two namespaces with the same key
collide, and the function does four full memory-directory scans per turn. This
phase fixes all three and extracts two testable seams so the scoring is provable
by mutation.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Read surfaces", item 3 — names exactly the
  three fixes this phase makes and nothing else.
- `docs/dev/milestones/M11-knowledge-index/README.md` § Exit criteria, the
  "Prompt assembly stops walking directories" bullet.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`src/daemon/memory_prompt.rs` is 231 lines. Line numbers below are current as of
drafting (2026-08-06); re-derive with
`grep -n "list_memories_with_tags\|fn " src/daemon/memory_prompt.rs`.

**Four full directory scans per turn.** `list_memories_with_tags(None, namespaces)`
appears at lines 52, 160, 187 and 203 — once in the parent and once in each of
the three helpers. Each call `read_dir`s three category directories per namespace
and parses the frontmatter of every file found.

```
$ grep -c "list_memories_with_tags(None, namespaces)" src/daemon/memory_prompt.rs
4
```

**The flat FTS constant** (lines 99–104):

```rust
    // FTS5 candidates get base score
    for info in &fts_candidates {
        candidate_keys
            .entry(info.key.clone())
            .or_insert(0.2 * crate::memory::review::effective_confidence(info));
    }
```

`ftsearch_memories` (line 201) receives the BM25 score from `index::fts5_search`
and discards it:

```rust
    for (namespace, key, _score) in results {
```

**Bare-key merge.** `candidate_keys` is a `HashMap<String, f64>` keyed by
`info.key` (lines 79–80, 89, 94, 101), and the final intersection at line 109
looks up `candidate_keys.get(&info.key)`. Two memories with the same key in
different namespaces — `global` and an agent namespace — share one entry, so the
second silently overwrites or is silently dropped.

**Last-writer merge semantics.** Tags use `*entry.or_insert(0.0) = combined`
(overwrite); relates_to and FTS use `.or_insert(...)` (first writer wins). So a
weak tag overlap permanently suppresses a strong FTS hit for the same memory.

**The logged keys can disagree with the block.** The budget loop (lines 119–126)
`continue`s past an entry that does not fit and keeps packing later, smaller
ones, but `keys` and `scores` (lines 137–142) are `scored.iter().take(count)` —
the *first* `count` by rank, not the ones actually included.

**Facts you can rely on, already checked:**

- `crate::memory::review::effective_confidence` (`src/memory/review.rs`) is a
  stub that returns `1.0` for every input. Do not build a test whose outcome
  depends on it varying.
- `MemoryInfo.pinned` is always `None` — `list_memories_with_tags` hardcodes it
  (`src/memory.rs:504`). The `!m.pinned.unwrap_or(false)` filter is therefore a
  no-op today. **Leave it exactly as it is**; writing `pinned` is out of scope.
- The `memories` FTS5 table carries `namespace`, `key` **and** `category` as
  `UNINDEXED` columns (`src/memory/index.rs:43-51`).

**SQLite `bm25()` returns a negative number**, and *more negative means a better
match*; `fts5_search` already orders ascending so index 0 is the best hit. This
was measured, not assumed:

```
$ sqlite3 :memory: "CREATE VIRTUAL TABLE t USING fts5(body, tokenize='porter unicode61');
INSERT INTO t VALUES('quokka quokka quokka');
INSERT INTO t VALUES('the quick brown fox jumps over the lazy dog and then quokka appears once');
SELECT rowid, bm25(t) FROM t WHERE t MATCH 'quokka' ORDER BY bm25(t);"
1|-1.824390243902439e-06
2|-7.9069767441860467e-07
```

Note the magnitudes: with a two-row corpus they are ~1e-6. **Any normalization
that assumes a particular absolute range is wrong.** Normalize relative to the
best hit in the same result set (formula pinned in task 3).

## Spec

### Task 1 — Thread one listing into the three helpers

In `src/daemon/memory_prompt.rs`, change the three helpers so none of them lists
the memory directory. Each takes the already-materialized slice instead:

```rust
pub fn find_by_tag_overlap(
    all: &[MemoryInfo],
    tags: &[String],
    limit: usize,
) -> Vec<MemoryInfo>

pub fn expand_relates_to(all: &[MemoryInfo], keys: &[String]) -> Vec<MemoryInfo>

pub fn ftsearch_memories(
    all: &[MemoryInfo],
    query: &str,
    limit: usize,
    namespaces: &[&str],
) -> Vec<(MemoryInfo, f64)>
```

Delete the `let all_memories = list_memories_with_tags(None, namespaces)...`
line from each helper body and iterate `all` instead. `namespaces` stays on
`ftsearch_memories` because `fts5_search` needs it for its `WHERE` clause; the
other two no longer need it — drop the parameter.

`ftsearch_memories` now returns the raw BM25 score alongside each resolved
`MemoryInfo` (normalization happens in task 3, not here). Resolve each
`(namespace, key, score)` hit against `all` by matching **both** `m.namespace ==
namespace && m.key == key`, as it does today. Preserve the existing rank order.

After this task, `assemble_turn_relevant_memory` must call
`list_memories_with_tags` exactly once and pass `&all_memories` down.

**One call site outside this file must be updated**, and it is a test — do not
delete it. `src/memory/index.rs:1842`, inside
`ftsearch_memories_preserves_rank_order`:

```rust
        let results = crate::daemon::memory_prompt::ftsearch_memories("quokka", 10, &["global"]);
        assert!(results.len() >= 2, "should find both memories");
        assert_eq!(
            results[0].key, "quokka-strong",
```

becomes:

```rust
        let all = crate::memory::list_memories_with_tags(None, &["global"]).unwrap();
        let results =
            crate::daemon::memory_prompt::ftsearch_memories(&all, "quokka", 10, &["global"]);
        assert!(results.len() >= 2, "should find both memories");
        assert_eq!(
            results[0].0.key, "quokka-strong",
```

The assertion message stays as it is. Run `cargo build` after this task before
moving on.

### Task 2 — Extract a `score_candidates` seam

Add to `src/daemon/memory_prompt.rs`:

```rust
/// Merge the three candidate sources into one namespace-keyed scored set.
/// Returns active (non-expired, above-threshold, non-pinned) memories only,
/// sorted by descending score.
pub(crate) fn score_candidates(
    all: &[MemoryInfo],
    all_tags: &[String],
    user_turn: &str,
    namespaces: &[&str],
    threshold: f64,
) -> Vec<(MemoryInfo, f64)>
```

It performs, in this order: the active filter (the same three predicates
currently at lines 56–61, unchanged); the three candidate lookups against the
**active** slice; the merge (task 3); the descending sort by score. Ties keep a
deterministic order — break them by `(namespace, key)` ascending so the output is
reproducible across runs.

Filtering to `active` *before* the candidate lookups is a deliberate change: an
expired or below-threshold memory must never reach the merge at all.

`assemble_turn_relevant_memory` keeps its existing signature and becomes: list
once → `score_candidates` → `pack_within_budget` (task 4) → log → format.

### Task 3 — Real normalized BM25, namespace-keyed merge, max-wins

Inside `score_candidates`, key the merge map by the tuple, not the bare key:

```rust
let mut candidate_scores: std::collections::HashMap<(String, String), f64> =
    std::collections::HashMap::new();   // (namespace, key) -> score
```

Every insert and the final lookup use `(info.namespace.clone(), info.key.clone())`.

**Each source computes a contribution; the merged score is the maximum across
sources** — replace both the overwrite (`*entry.or_insert(0.0) = combined`) and
the first-writer-wins (`.or_insert(...)`) semantics with a max-merge helper:

```rust
fn merge_max(map: &mut HashMap<(String, String), f64>, info: &MemoryInfo, score: f64) {
    let e = map
        .entry((info.namespace.clone(), info.key.clone()))
        .or_insert(f64::NEG_INFINITY);
    if score > *e {
        *e = score;
    }
}
```

The three contributions:

- **Tag overlap** — unchanged: `overlap as f64 / all_tags.len().max(1) as f64`,
  times `effective_confidence(info)`.
- **relates_to** — unchanged: `0.3 * effective_confidence(info)`.
- **FTS** — replaces the flat `0.2`:

  ```rust
  /// Weight applied to a normalized BM25 hit. Chosen so the strongest FTS hit
  /// (0.6) outranks a relates_to hit (0.3) while a full tag-overlap match (1.0)
  /// still leads.
  const FTS_WEIGHT: f64 = 0.6;
  ```

  Given `hits: Vec<(MemoryInfo, f64)>` from `ftsearch_memories` where each `f64`
  is the raw (negative) `bm25()` value, let `mag_i = -raw_i` and
  `mag_max = the largest mag_i in this result set`. Then:

  ```rust
  let normalized = if mag_max > 0.0 { mag_i / mag_max } else { 0.0 };
  let contribution = FTS_WEIGHT * normalized * effective_confidence(info);
  ```

  `normalized` is in `(0.0, 1.0]`, so the best hit contributes exactly
  `FTS_WEIGHT` and weaker hits contribute strictly less. Guard `mag_max == 0.0`
  (empty result set, or a degenerate all-zero score) with the `else` arm above —
  do not divide by zero.

**The property that matters and must not be lost:** two FTS hits with different
BM25 values get **pairwise distinct** scores. A normalization that clamps,
rounds, or buckets them back together defeats the whole task.

### Task 4 — Log the entries actually included, not the first N by rank

Add:

```rust
/// Render scored entries in rank order, keeping those that fit the byte budget.
/// Returns the rendered entry alongside its `MemoryInfo` and score so the
/// caller logs exactly what it emitted.
pub(crate) fn pack_within_budget(
    scored: &[(MemoryInfo, f64)],
    budget: usize,
) -> Vec<(String, MemoryInfo, f64)>
```

It keeps the current loop shape — including the `continue`-past-a-too-large-entry
behavior, which is intentional packing and must be preserved. Do not turn it into
a `break`.

`assemble_turn_relevant_memory` then derives `count`, `total`, the joined block,
and the `memory_retrieved` event's `keys` / `scores` arrays **from this one
vector**. `keys` stays a `Vec<String>` of bare keys (the event's shape is
unchanged); `scores` stays a `Vec<f64>` in the same order. The header
(`[TURN MEMORY] {count} memories, {total} bytes`) and the `stats::` calls keep
their current behavior.

### Task 5 — Tests

Add an inline `#[cfg(test)] mod tests` at the bottom of
`src/daemon/memory_prompt.rs` (the idiom used across `src/daemon/`). Every test
takes `let _guard = crate::test_home_guard();` then sets `HOME` to a
`tempfile::tempdir()` — see `src/memory/index.rs:1820-1831` for the exact
preamble, and seed memories with `crate::memory::add_memory(key, body, category,
namespace)`. Test names and behaviors are pinned in § Test plan.

## Acceptance criteria

- [ ] `grep -c "list_memories_with_tags(" src/daemon/memory_prompt.rs` prints `1`.
- [ ] `grep -n "0\.2 \* crate::memory::review" src/daemon/memory_prompt.rs` finds
      nothing (exit 1).
- [ ] `grep -n "FTS_WEIGHT" src/daemon/memory_prompt.rs` finds the const
      definition and its use site.
- [ ] `cargo test --lib daemon::memory_prompt` passes and reports **more than 0**
      tests — the module has none today, so a filter that matches nothing would
      also "pass".
- [ ] `cargo test --lib` reports **more than 1122** passed, 0 failed. (1122 is
      the baseline measured at drafting; the exact new total is yours, but it
      must not be 1122 and no test may be removed.)
- [ ] `cargo fmt --all`, `cargo build`, and
      `cargo clippy --all-targets --all-features -- -D warnings` are clean.
- [ ] The mutation pair in § End-to-end verification is captured, and the
      restore is proven by the grep that follows it.

## Test plan

All in the new `mod tests` in `src/daemon/memory_prompt.rs` unless stated.

- `fts_hits_get_pairwise_distinct_scores` — seed two `Knowledge` memories in
  `global` whose bodies match a query with clearly different strengths (the
  `quokka-strong` / `quokka-weak` shape at `src/memory/index.rs:1822-1840` is a
  working pair). Neither memory carries tags, and `SessionTags` contributes none,
  so FTS is the only source. Call `score_candidates` and assert: both are
  present; their scores are **strictly different** from each other; the stronger
  match sorts first; and the top score equals `FTS_WEIGHT`.
- `fts_score_is_not_the_flat_constant` — from the same fixture, assert the weak
  hit's score is strictly greater than `0.0` and strictly less than
  `FTS_WEIGHT`, and that neither score equals `0.2`.
- `same_key_in_two_namespaces_scores_separately` — `add_memory` the same key
  under `global` and under an agent namespace (`add_memory(key, body, cat,
  "analyst")`) with different bodies, one a much stronger match. Call
  `score_candidates` with `namespaces = &["analyst", "global"]` and assert **two**
  entries come back, with distinct `namespace` values and distinct scores. Under
  the bare-key merge exactly one survived — that is the regression this pins.
- `tag_hit_does_not_suppress_a_stronger_fts_hit` — one memory tagged so it
  overlaps exactly one of several session tags (a tag contribution well below
  `FTS_WEIGHT`) while also being the strongest FTS hit for the user turn. Assert
  its score is strictly greater than the tag-only contribution. Under the old
  first-writer-wins merge it kept the tag score.
- `expired_memory_is_excluded_and_the_guard_is_not_vacuous` — seed an **expired**
  memory (write the file directly with `expires: "2020-01-01"` in the
  frontmatter, as at `src/memory/index.rs:1604-1611`) that is a strong match for
  the query, *and* a non-expired control memory that also matches. Assert the
  expired key is absent **and** the control key is present. The second assertion
  is the point: without it the test passes whenever the fixture is empty for any
  reason.
- `packing_reports_the_entries_it_emitted` — build a `scored` vector by hand
  where the second entry's rendered form is larger than the remaining budget and
  the third fits. Call `pack_within_budget` directly and assert the returned
  vector is entries one and three — not one and two. This is the fix for the
  `take(count)` mismatch; construct the `MemoryInfo` values inline rather than
  going through the filesystem.
- `ftsearch_memories_preserves_rank_order` in `src/memory/index.rs` — **update
  the call site and the tuple access as shown in task 1. Do not delete or
  rename this test.**

## End-to-end verification

This phase ships no CLI-visible artifact — the changed code runs on the daemon's
per-turn prompt path, which needs a live LLM turn to observe. The evidence
required instead is the **structural proof** that the walks and the flat constant
are gone, plus a **mutation pair** proving the new scoring is actually load-
bearing.

Run this block verbatim and paste the resulting file's contents into an Update
Log entry titled `### Update — <date> (end-to-end verification)`. **The
server-authored `(complete)` entry does not satisfy this**, no matter how
accurately its summary describes what you ran.

```sh
set -x
{
  echo "== structural =="
  grep -c "list_memories_with_tags(" src/daemon/memory_prompt.rs; echo "exit=$?"
  grep -n "0\.2 \* crate::memory::review" src/daemon/memory_prompt.rs; echo "exit=$?"
  grep -n "FTS_WEIGHT" src/daemon/memory_prompt.rs; echo "exit=$?"

  echo "== baseline: module tests green =="
  cargo test --lib daemon::memory_prompt 2>&1 | tail -5; echo "exit=$?"
} > /tmp/p06-e2e.txt 2>&1
cat /tmp/p06-e2e.txt
```

Then the mutation, in three steps, appending to the same file:

```sh
# 1. MUTATE: change the FTS contribution back to the flat constant.
#    Edit the one line in score_candidates that computes the FTS contribution so
#    it reads `let contribution = 0.2 * crate::memory::review::effective_confidence(info);`
{
  echo "== MUTATED: FTS contribution forced to flat 0.2 =="
  cargo test --lib daemon::memory_prompt 2>&1 | tail -20; echo "exit=$?"
} >> /tmp/p06-e2e.txt 2>&1

# 2. RESTORE the line to the FTS_WEIGHT * normalized * eff form.
{
  echo "== RESTORED =="
  cargo test --lib daemon::memory_prompt 2>&1 | tail -5; echo "exit=$?"
  echo "== restore proof: the flat constant must be absent (expect exit=1, no output) =="
  grep -n "0\.2 \* crate::memory::review" src/daemon/memory_prompt.rs; echo "exit=$?"
  echo "== restore proof: the real formula must be present =="
  grep -n "FTS_WEIGHT \* normalized" src/daemon/memory_prompt.rs; echo "exit=$?"
} >> /tmp/p06-e2e.txt 2>&1
cat /tmp/p06-e2e.txt
```

The mutated run **must show at least one failing test**, and you must name in
your Update Log which tests failed. A mutation that leaves every test green
means the scoring is not covered and the phase is not done.

**The restore is mandatory and is checked at review by grepping the shipped
source.** Three earlier phases in this milestone shipped a mutation that was
never undone. The two `grep` lines in step 2 are what prove it: the first must
print nothing and `exit=1`, the second must print the line and `exit=0`.

## Authorizations

None. No new dependencies, no architecture-doc changes.

## Out of scope

- **`effective_confidence`** stays the `1.0` stub. Do not implement review
  scoring.
- **Writing `pinned`.** The no-op `pinned` filter stays exactly as it is.
- **Eliminating the last directory listing.** One `list_memories_with_tags` call
  per turn remains, and that is the target. Removing it entirely needs a reverse
  `relates_to` index and index-side expiry filtering — a different phase.
- **`config`-driven budget/threshold.** `budget = 4096` and `threshold = 0.5`
  stay hardcoded with the existing `// G5 stub` comment; the `_config` parameter
  stays unused.
- **The `memory_retrieved` event's shape.** Same event name, same four fields.
  Only the *values* of `keys` and `scores` change.
- **Anything in `src/memory/index.rs` beyond the one test call site** named in
  task 1. `fts5_search` keeps its current signature and behavior.
- **Phase 07's situational injections** — no turns/epochs lines in the dynamic
  block here.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
