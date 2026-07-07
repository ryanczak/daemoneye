# Phase 05: Epoch records — immutable per-span compaction history

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-01 (segment reader), phase-03 (budget cut), phase-04 (archive)
**Estimated diff:** ~500 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Replace the single regenerated `[Session Digest]` with an **append-only chain
of epoch records**: each compaction turns the dropped span into one immutable
record (narrative + tally **for that span only**), persisted to
`<id>.epochs.jsonl` and rendered into a regenerated working-set head. This
kills the O(session-lifetime) rescan (D5), the unbounded artifact scan (D6),
the pinned stale first message (D7), and the oldest-first narrative
truncation (D3, partial — chapters land in phase 06).

## Architecture references

Read before starting:

- `docs/design/context-management.md#32-epoch-chain--hierarchical-summaries-instead-of-one-regenerated-digest`
- `docs/design/context-management.md#33-working-set-layout-and-token-budgeting`
  — the two-message regenerated head.
- `docs/design/context-management.md#6-invariants` — epochs are append-only.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors — phases 01–04 have landed;
   `compact_with_digest` now takes an explicit `tail_start` (phase 03) and
   `tally_events` streams segments (phase 01).

## Current state

**The load-bearing constraints (front-loaded):**

1. Every provider requires the message list to start with a `user` message
   and alternate roles; the current compacted layout is
   `[original msg0 (user), digest (assistant), tail (starts on clean user
   turn)]`. This phase keeps that exact **shape** — two head slots then the
   tail — but both head slots become **synthetic and regenerated at every
   compaction**. Do not change the shape; only the contents.
2. `epochs.jsonl` is append-only. A bug is fixed by appending a corrective
   record, never by editing. No code path may rewrite the file.

Key existing code:

- `src/daemon/digest.rs::tally_events(session_id, since)` — post-phase-01 it
  streams via `for_each_event_between(Some(since), None, …)` into the
  private `EventTally`. This phase adds an `until` bound and makes the tally
  serializable.
- `src/daemon/digest.rs::build_session_digest(session_id, since,
  message_count, narrative)` — formats the digest text; superseded by epoch
  rendering (delete after migration; its `log_event("session_digest_*")`
  telemetry is replaced by one `epoch_created` event).
- `src/daemon/digest.rs::scan_artifacts(since)` — mtime scan; gains the same
  `until` bound (a `SystemTime` range check).
- `src/daemon/digest.rs::build_narrative_summary(messages, model_entry)` —
  kept as-is, except its input formatter
  `format_messages_for_narrative` (`digest.rs:312-358`) truncates
  **oldest-first** today:

  ```rust
  if out.len() >= NARRATIVE_INPUT_CHAR_BUDGET {
      out.push_str("\n[…truncated to fit summarizer budget…]\n");
      break;      // <-- drops the NEWEST dropped turns
  }
  ```

- The compaction call site is the `should_digest` block in
  `src/daemon/server/ask.rs:269-319`; `sessions` store gives
  `started_at`; `session_id` may be `None` (then only `trim_history` runs —
  keep that fallback).
- `Message.turn: Option<...>` (`src/ai/types/wire.rs:27`) provides turn
  numbers; `None` on legacy messages.
- Config: `config.context.environment` (string) and
  `crate::daemon::daemon_hostname()` are available for the regenerated
  header (see `src/daemon/prompt.rs:108-115` for how the first-turn prompt
  uses them).

## Spec

### 1. `src/daemon/context/epochs.rs` — types and persistence

```rust
/// Serializable per-span event tally. List fields are CAPPED at
/// `TALLY_LIST_CAP` (10) entries; `_count` fields carry the true totals.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EpochTally {
    pub commands_ok: u32,
    pub commands_fail: u32,
    pub failed_cmds: Vec<(String, i32)>,      // capped
    pub files_edited_count: u32,
    pub files_edited: Vec<String>,            // capped
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub alerts_count: u32,
    pub alerts: Vec<String>,                  // capped
    pub ghost_starts: u32,
    pub ghost_completions: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpochRecord {
    pub seq: u32,
    /// "epoch" now; "chapter" arrives in phase 06.
    pub kind: String,
    pub turn_start: u32,   // 0 when unknown (legacy messages)
    pub turn_end: u32,
    pub ts_start: chrono::DateTime<chrono::Utc>,
    pub ts_end: chrono::DateTime<chrono::Utc>,
    pub msg_count: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub narrative: Option<String>,
    pub tally: EpochTally,
    /// "runbook:name" / "script:name" / "memory:key [category]" strings.
    pub artifacts: Vec<String>,
    /// Phase 06: seq range this chapter covers. None for plain epochs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub covers: Option<(u32, u32)>,
}

pub fn epochs_file(id: &str) -> PathBuf;              // sessions_dir()/<id>.epochs.jsonl
pub fn read_epochs(id: &str) -> Vec<EpochRecord>;     // empty on absent/unreadable
pub fn append_epoch(id: &str, rec: &EpochRecord);     // append-only; WARN on failure
```

### 2. Per-span tally — rework `tally_events` / `scan_artifacts`

- `tally_events(session_id, since, until) -> EpochTally` — move to
  `context/epochs.rs` (leave a `pub use` shim in `digest.rs` if other code
  references it; drafted-time grep shows the compaction block is the only
  caller). Window both bounds through `for_each_event_between(Some(since),
  Some(until), …)`. Keep every existing match arm's semantics; on push to a
  capped list, stop at `TALLY_LIST_CAP` but keep counting `_count`.
- `scan_artifacts(since, until)` — same signature change; an artifact
  qualifies when `since <= mtime < until`. Return the flat
  `Vec<String>` tag form above (fold the current
  `ArtifactChanges` struct into it; schedules keep their
  `"schedule:name (kind)"` tag).

### 3. Building an epoch at compaction — in the `should_digest` block (`ask.rs`)

Replacing the digest construction:

```rust
let epochs = read_epochs(session_id);
let span_start = epochs.last().map(|e| e.ts_end).unwrap_or(started_at);
let span_end = chrono::Utc::now();
let dropped = &messages[..tail_start];               // narrative input (skip synthetic head if present)
let narrative = /* build_narrative_summary as today, honoring narrative_enabled */;
let record = EpochRecord {
    seq: epochs.last().map(|e| e.seq + 1).unwrap_or(1),
    kind: "epoch".into(),
    turn_start: first_turn_of(dropped), turn_end: last_turn_of(dropped),
    ts_start: span_start, ts_end: span_end,
    msg_count: dropped.len() as u32,
    narrative,
    tally: tally_events(session_id, span_start, span_end),
    artifacts: scan_artifacts(span_start, span_end),
    covers: None,
};
append_epoch(session_id, &record);
log_event("epoch_created", json!({"session": …, "seq": …, "turns": …, "msgs": …}));
```

Details: `first_turn_of`/`last_turn_of` take the min/max of `Message.turn`
over the slice, defaulting to 0. When compacting a working set that already
has a synthetic head (see §4), exclude the two head messages from the
narrative input and `msg_count` — only real turns are epoch content.

`session_id == None` or `started_at` missing keeps today's `trim_history`
fallback unchanged.

### 4. Regenerated two-slot head — `compact_with_epochs` in `context/epochs.rs`

Replaces `compact_with_digest` (delete it and migrate its tests):

```rust
/// Layout: [synthetic user "[Session Context] …", synthetic assistant ack,
/// tail…]. `tail_start` comes from the phase-03 planner and MUST be a
/// clean/repaired boundary already.
pub fn compact_with_epochs(
    messages: Vec<Message>,
    rendered_context: &str,
    tail_start: usize,
) -> Vec<Message>
```

Pinned head contents:

- **Slot 0 (user):**

  ```
  [Session Context — regenerated at compaction; turns 1..{turn_end} summarized]
  Environment: {config.context.environment} · Daemon host: {daemon_hostname()}

  {rendered_context}

  Older turns are preserved in the session archive.
  ```

- **Slot 1 (assistant):**
  `Continuing session — the context above covers everything before turn
  {tail_first_turn}.`

- Both have `tool_calls: None, tool_results: None, turn: None`.

`rendered_context` comes from:

```rust
/// Render the epoch chain for the working-set head. Phase 05: the last
/// RENDER_EPOCHS (8) epochs, newest last — each as
/// "Epoch {seq} (turns {a}–{b}): {narrative | tally one-liner}".
/// Older epochs appear only as one count line
/// ("…{n} earlier epochs — chapter rollups arrive in a later phase").
/// Phase 06 replaces that count line with ledger + chapters.
pub fn render_context_block(epochs: &[EpochRecord]) -> String
```

For an epoch without a narrative, the one-liner is built from the tally
(commands ok/fail, files edited count, alerts count) — reuse the current
digest's formatting fragments (`digest.rs:507-563`) for the wording.

The original `messages[0]` is **not** special-cased anymore — it falls into
the dropped span like any message (it is in the archive, phase 04). Fixes D7.

### 5. Keep-newest narrative truncation — `format_messages_for_narrative`

When the serialized transcript would exceed `NARRATIVE_INPUT_CHAR_BUDGET`,
keep the **newest** messages: iterate the slice in reverse accumulating
serialized chunks until the budget, then emit in chronological order with a
leading `[…older dropped turns omitted from summarizer input…]` marker.
Pinned test: with 5 oversized messages, the output contains the *last*
message's content and not the first (today's test
`format_messages_for_narrative_truncates_at_budget` asserts the opposite
retention — rewrite it).

### 6. Migration / compatibility

- A session whose working file still holds an old-style
  `[Session Digest …]` assistant message needs no migration: at its next
  compaction the digest message is dropped into the archive like any other
  message, and its content feeds the narrative input (rolling continuity
  preserved).
- `DIGEST_THRESHOLD`, elision, the decision thresholds — all unchanged from
  phase 03.
- Update `assets/prompts/sre.toml` if it references "[Session Digest]"
  (grep; drafted-time check found the compaction notice only in code).
  The `Response::SystemMsg` compaction notice text in `ask.rs:566-571`
  changes to: `"↩ Session history compacted ({} → {} messages) — epoch {}
  recorded; older turns in the session archive"`.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] Two successive compactions produce `seq 1` and `seq 2` records whose
      spans do not overlap (`epoch2.ts_start == epoch1.ts_end`) and whose
      tallies count only their own span's events (test below — the D5 fix).
- [ ] `epochs.jsonl` after two compactions contains both records; the file
      was only ever opened in append mode (`grep -rn "epochs_file" src/`
      shows no truncating writer — reviewer-verifiable).
- [ ] The compacted head is `[user "[Session Context]…", assistant
      "Continuing session…", clean-boundary tail…]`; the original first
      message is gone from the working set but present in the archive.
- [ ] Tally list fields never exceed 10 entries while `_count` fields exceed
      10 (test with 15 failed commands — the D5 unbounded-join fix).
- [ ] Keep-newest truncation: summarizer input over budget retains the
      newest message, drops the oldest (rewritten test).
- [ ] `render_context_block` with 12 epochs renders the last 8 + one
      "…4 earlier epochs" line (test below).
- [ ] Orphan-safety checker (phase 03's shared helper) passes on
      `compact_with_epochs` output.

## Test plan

FS tests take `TEST_HOME_LOCK` + temp HOME.

- `epoch_records_append_and_read_roundtrip` in `epochs.rs`.
- `epoch_spans_are_disjoint_and_tallies_scoped` — seed dated event fixtures
  in two time windows; two epoch builds; each tally sees only its window.
- `tally_lists_capped_counts_exact` — 15 failures → `failed_cmds.len() ==
  10`, `commands_fail == 15`.
- `compact_with_epochs_head_shape` — role/user, role/assistant, tail user;
  original msg0 absent; orphan checker green.
- `render_context_block_caps_at_eight`.
- `narrative_input_keeps_newest` — rewrite of the existing truncation test.
- `scan_artifacts_until_bound_excludes_newer` — artifact mtime after `until`
  is excluded (**negative case** for the span bound).

## End-to-end verification

Drive the compaction path via the integration harness (no live model:
`narrative_enabled = false` gives the structured-only path):

1. Construct a session JSONL + fixture events in a temp HOME, invoke the
   compaction block twice via its public entry (or a `#[cfg(test)]`-gated
   harness function if the block is not separately callable — prefer
   extracting the epoch-build into a testable `pub(crate) fn` over adding
   cfg-gates), then `cat` the resulting `.epochs.jsonl` and quote both
   records in the completion log.

## Authorizations

- [ ] May touch `docs/architecture.md` §1.2 (orchestration layer): add one
      sentence noting compaction history is an epoch chain per
      `docs/design/context-management.md`.

## Out of scope

- Chapters / ledger / rollups — phase 06 (the `covers` field and the
  "…n earlier epochs" line are their hook points).
- The recall tool — phase 07.
- Async execution — phase 08 (everything here stays synchronous in the
  `should_digest` block).
- Memory extraction — phase 10.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
