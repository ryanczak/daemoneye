# Phase 06: Session ledger and chapter rollups

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-05 (epoch records)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make the in-context representation of a very long session **O(log turns)**
(design defect D3): when uncovered epochs pile up, roll the oldest five into
one immutable `chapter` record; render the working-set head as
*ledger → chapters → recent epochs*. A 3000-turn session becomes a few
chapter lines + ≤ 8 epoch summaries + the live tail, instead of either a
constant-size lossy summary or an unbounded list.

## Architecture references

Read before starting:

- `docs/design/context-management.md#32-epoch-chain--hierarchical-summaries-instead-of-one-regenerated-digest`
  — chapters and the ledger.
- `docs/design/context-management.md#6-invariants` — chapters *cover*
  epochs; nothing is deleted or edited.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors against phase 05's landed code.

## Current state

Phase 05 delivered, in `src/daemon/context/epochs.rs`:

- `EpochRecord` with `kind: String` ("epoch"), `covers: Option<(u32, u32)>`
  (always `None` so far), `tally: EpochTally` (capped lists + `_count`
  totals), `narrative: Option<String>`.
- `read_epochs` / `append_epoch` (append-only JSONL).
- `render_context_block(epochs)` — renders the last 8 epochs plus an
  "…{n} earlier epochs — chapter rollups arrive in a later phase" count
  line. **That count line is this phase's replacement point.**
- The epoch build runs synchronously in the `should_digest` block of
  `src/daemon/server/ask.rs`, right after `append_epoch`.
- `build_narrative_summary(messages, model_entry)`
  (`src/daemon/digest.rs`) — the small-model summarizer with 20 s timeout;
  the rollup summarizer below reuses its client/timeout pattern but takes a
  plain string prompt, so extract the reusable core first (see Spec §2).

## Spec

### 1. Rollup decision + record — in `src/daemon/context/epochs.rs`

```rust
/// Number of uncovered epochs that triggers a chapter rollup.
/// From config `[compaction] rollup_after` (serde default 10).
/// Number of oldest uncovered epochs folded per rollup.
const ROLLUP_FOLD: usize = 5;

/// An epoch is "uncovered" when kind == "epoch" and no chapter's `covers`
/// range contains its seq.
pub fn uncovered_epochs(all: &[EpochRecord]) -> Vec<&EpochRecord>;

/// When uncovered count exceeds `rollup_after`, fold the ROLLUP_FOLD oldest
/// uncovered epochs into one chapter record and append it. Chapter fields:
/// - kind: "chapter", covers: Some((first.seq, last.seq))
/// - seq: next seq (chapters share the same monotonic seq space)
/// - turn_start/turn_end, ts_start/ts_end: union of the folded epochs
/// - msg_count: sum; tally: element-wise sum (lists re-capped at 10,
///   `_count` fields summed exactly)
/// - narrative: summarizer output (§2), or the structured fallback (§2)
pub async fn maybe_rollup(id: &str, config: &Config) -> Option<EpochRecord>;
```

Add `rollup_after: u32` to `CompactionConfig` (`src/config/types.rs`,
default fn returning 10 — follow the phase-03 field pattern).

`EpochTally` gains `pub fn merge(&mut self, other: &EpochTally)` —
element-wise sum with list re-capping; unit-tested in isolation.

### 2. Chapter narrative — reuse the summarizer core

In `src/daemon/digest.rs`, refactor `build_narrative_summary` into:

```rust
/// One-shot small-model call: system prompt + user text → trimmed response.
/// 20 s timeout; None on any failure. (Extracted from
/// build_narrative_summary — that function becomes a thin caller.)
pub async fn summarize_once(system: &str, user_text: &str,
                            model_entry: &ModelEntry) -> Option<String>;
```

Chapter system prompt (pin verbatim):

```
You are compacting an SRE assistant's session history. You will be shown
5 epoch summaries, each covering a span of conversation turns. Write ONE
combined summary of at most 3 lines preserving: what was worked on, key
outcomes/decisions, and anything still unresolved. Past tense, terse,
no preamble.
```

User text: the folded epochs' `"Epoch {seq} (turns {a}–{b}): {narrative or
tally one-liner}"` lines joined by newlines.

**Structured fallback** (summarizer failed/disabled): chapter narrative =
first line of each folded epoch's narrative (or its tally one-liner), joined
with `" · "`, truncated to 500 chars at a char boundary. The rollup must
never fail outright for lack of a model — pin a test with
`narrative_enabled = false`.

### 3. Ledger + rendering — replace the phase-05 count line

`render_context_block(epochs)` becomes:

```
Session ledger: {N} turns compacted across {k} epochs — commands {ok} ok /
{fail} failed · files edited {n} · alerts {n} · ghosts {n} · ~{p}k prompt /
~{c}k completion tokens
Chapters:
  Chapter {seq} (turns {a}–{b}): {narrative}
  …
Recent epochs:
  Epoch {seq} (turns {a}–{b}): {…}
  …(last 8 uncovered epochs, unchanged from phase 05)
```

- The ledger line sums **chapters' tallies + uncovered epochs' tallies**
  (covered epochs are excluded — their numbers live in their chapter;
  summing both would double-count. Pin this with a test.)
- Chapters render oldest-first, all of them (each is ≤ 3 lines; at
  `rollup_after = 10`/`ROLLUP_FOLD = 5` a 3000-turn session has ~tens of
  chapters — acceptable; deeper hierarchies are future work).
- Omit the `Chapters:` section when none exist; omit the ledger when only
  one epoch exists (the epoch line already says it all).

### 4. Call site — in the `should_digest` block (`ask.rs`)

After `append_epoch(…)`, call `maybe_rollup(session_id, config).await`; then
`read_epochs` (fresh, including any new chapter) and render. One rollup per
compaction pass at most — no loop (backlog drains one chapter per
compaction; pin with a test that 30 uncovered epochs produce exactly one
chapter per `maybe_rollup` call).

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] With `rollup_after = 10`: 11 uncovered epochs → `maybe_rollup` appends
      one chapter covering seqs 1–5; `uncovered_epochs` then returns 6.
- [ ] With 10 uncovered epochs → no rollup (**negative case**: threshold is
      "exceeds", not "reaches").
- [ ] The chapter's tally equals the element-wise sum of the folded epochs'
      tallies; the ledger over chapters+uncovered equals the ledger over all
      original epochs (no double-count test).
- [ ] Rollup succeeds with the summarizer disabled (structured fallback
      narrative non-empty).
- [ ] `epochs.jsonl` is still append-only: after a rollup, the folded epoch
      records are byte-for-byte still present in the file.
- [ ] Rendering: covered epochs do not appear under "Recent epochs";
      chapters appear oldest-first; single-epoch sessions render no ledger.

## Test plan

- `tally_merge_sums_and_recaps` in `epochs.rs` — pure.
- `rollup_triggers_only_above_threshold` — 10 → none, 11 → one chapter
  (structured fallback path; no model needed).
- `rollup_chapter_fields_union_and_sum`.
- `rollup_folds_once_per_call` — 30 uncovered epochs, one call → one
  chapter.
- `ledger_excludes_covered_epochs` — the no-double-count assertion.
- `render_with_chapters_and_recent` — full layout string assertions
  (contains `Session ledger:`, `Chapter`, `Recent epochs:`; covered epoch's
  seq absent from the recent list).
- `rollup_appends_never_rewrites` — file content before rollup is a prefix
  of file content after.

## End-to-end verification

Fixture-driven, same harness as phase 05's E2E: seed 11 epoch records into a
temp-HOME `.epochs.jsonl`, run `maybe_rollup` + `render_context_block`
through the extracted `pub(crate)` entry, and quote the appended chapter
JSON line plus the rendered block in the completion log.

## Authorizations

None.

## Out of scope

- No L3 rollups (chapters-of-chapters) — future work if tens of chapters
  ever matter in practice.
- No changes to when compaction fires (phase 03 owns thresholds).
- No async — phase 08.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
