# Phase 05a: Epoch records — types, persistence, per-span tally

**Milestone:** M4 — Context Management Overhaul
**Status:** done
**Depends on:** phase-01 (segment reader), phase-03 (budget cut), phase-04 (archive)
**Estimated diff:** ~280 lines
**Tags:** language=rust, kind=feature, size=m

> **Split note:** phase-05 (epoch-records) was re-split into **05a** (this doc —
> the additive persistence + per-span tally layer) and **05b** (the compaction
> rewire: regenerated head, keep-newest narrative, deleting the old digest
> path). 05a is deliberately **purely additive** — it adds a new module and new
> functions and deletes/rewires **nothing**, so the build stays green at every
> step. 05b consumes what 05a lands. Do only 05a's scope here.

## Goal

Add the append-only epoch persistence layer and the **span-windowed** tally /
artifact-scan functions that 05b will call at compaction time. This is the
storage + measurement half of replacing the single regenerated `[Session
Digest]` with an append-only chain of immutable per-span epoch records (D5, D6).
Nothing in the compaction path changes yet — that is 05b.

## Architecture references

Read before starting:

- `docs/design/context-management.md#32-epoch-chain--hierarchical-summaries-instead-of-one-regenerated-digest`
- `docs/design/context-management.md#6-invariants` — epochs are append-only.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors below against the working tree.

## Current state

- `src/daemon/context/mod.rs` currently reads `pub mod estimate;` (phase 02).
  This phase adds `pub mod epochs;`.
- `src/daemon/digest.rs::tally_events(session_id, since)` (`:68`) returns the
  **private** `EventTally` struct (`digest.rs:~51`), scanning via
  `for_each_event_between(Some(since), None, &mut |v| …)`. Its match arms
  (`ai_turn`, `job_complete`, `job_start`, `gc_window`, `file_edit`,
  `webhook_alert`, `ghost_start`, `ghost_complete`) are the semantics to
  mirror. **Leave this function and `EventTally` untouched** — 05b deletes them.
- `src/daemon/digest.rs::scan_artifacts(since)` (`:144`) returns the private
  `ArtifactChanges` struct via mtime `>= since` checks over runbooks/scripts/
  memories/schedules dirs. **Leave it untouched** — 05b deletes it.
- `crate::daemon::utils::for_each_event_between(Some(since), Some(until), …)`
  already accepts an optional `until` upper bound (phase 01) — the windowed
  tally uses it directly.
- `Message.turn: Option<u32-or-usize>` (`src/ai/types/wire.rs:27`) — check the
  actual integer type; `None` on legacy messages.

## Spec

### 1. New module `src/daemon/context/epochs.rs`

Register it: add `pub mod epochs;` to `src/daemon/context/mod.rs` (after the
existing `pub mod estimate;`).

Types (pin these exactly — 05b and phase 06 depend on the field names):

```rust
use crate::ai::Message;
use std::path::PathBuf;

/// Cap on how many entries each list field of an EpochTally retains; the
/// paired `_count` field always carries the true total.
pub const TALLY_LIST_CAP: usize = 10;

/// Serializable per-span event tally. List fields are CAPPED at
/// `TALLY_LIST_CAP` entries; `_count` fields carry the true totals.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EpochTally {
    pub commands_ok: u32,
    pub commands_fail: u32,
    pub failed_cmds: Vec<(String, i32)>,   // capped
    pub files_edited_count: u32,
    pub files_edited: Vec<String>,         // capped
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub alerts_count: u32,
    pub alerts: Vec<String>,               // capped
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
    /// "runbook:name" / "script:name" / "memory:key [category]" /
    /// "schedule:name (kind)" strings.
    pub artifacts: Vec<String>,
    /// Phase 06: seq range this chapter covers. None for plain epochs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub covers: Option<(u32, u32)>,
}
```

Persistence (append-only — mirror the archive discipline from phase 04):

```rust
/// `sessions_dir()/<id>.epochs.jsonl`.
pub fn epochs_file(id: &str) -> PathBuf;
/// Read the whole epoch chain in order; empty Vec on absent/unreadable file
/// (never errors — a missing chain is a fresh session). Skip malformed lines.
pub fn read_epochs(id: &str) -> Vec<EpochRecord>;
/// Append one record as a single JSON line. Append-only: open with
/// OpenOptions::new().create(true).append(true). NEVER truncate/rewrite.
/// WARN + non-fatal on failure (mirror `append_session_message`).
pub fn append_epoch(id: &str, rec: &EpochRecord);
```

### 2. Span-windowed tally — `tally_span` in `epochs.rs`

Add a **new** function (do NOT modify `digest::tally_events`):

```rust
/// Tally events for one session in the half-open window `[since, until)`.
pub fn tally_span(
    session_id: &str,
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
) -> EpochTally
```

Mirror every match arm of `digest::tally_events` (same event names, same
belongs-to-session logic). Two differences:
- Window both bounds: `for_each_event_between(Some(since), Some(until), …)`.
- On push to a capped list (`failed_cmds`, `files_edited`, `alerts`): only push
  while `list.len() < TALLY_LIST_CAP`, but **always** increment the paired
  `_count` (`commands_fail` already counts; add `files_edited_count` and
  `alerts_count` increments on every matching event regardless of the cap).

### 3. Span-windowed artifact scan — `scan_artifacts_span` in `epochs.rs`

Add a **new** function (do NOT modify `digest::scan_artifacts`):

```rust
/// Artifacts whose mtime falls in `[since, until)`, as flat tag strings.
pub fn scan_artifacts_span(
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
) -> Vec<String>
```

Reuse `digest::scan_artifacts`'s directory walk logic (runbooks/scripts/
memories/schedules), but: (a) qualify an entry only when `since <= mtime <
until` (an mtime `>= until` is **excluded** — pin a negative test), and (b)
emit the flat tag form: `"runbook:{name}"`, `"script:{name}"`,
`"memory:{key} [{category}]"`, `"schedule:{name} ({kind})"`. You may factor a
shared private mtime-range directory helper; do not delete the original
`scan_artifacts`.

## Acceptance criteria

- [ ] `cargo test` passes; `cargo clippy --all-targets --all-features -- -D warnings` clean.
- [ ] `EpochRecord`/`EpochTally` round-trip through serde (write one line, read
      it back, fields equal).
- [ ] `epochs.jsonl` is only ever opened append/read: `grep -rn "epochs_file"
      src/` shows no truncating writer (reviewer-verifiable).
- [ ] `tally_span` over two disjoint time windows returns tallies that each
      count only their own window's events.
- [ ] A `tally_span` window with 15 failed commands yields `failed_cmds.len()
      == TALLY_LIST_CAP` (10) while `commands_fail == 15` (the D5 unbounded-list
      fix).
- [ ] `scan_artifacts_span` excludes an artifact whose mtime is `>= until`
      (negative case).
- [ ] `digest::tally_events`, `digest::scan_artifacts`, `build_session_digest`,
      and `compact_with_digest` are **unchanged** (this phase is additive).

## Test plan

FS tests take `crate::TEST_HOME_LOCK` + a temp `HOME` (idiom:
`src/daemon/server/catchup.rs`; also `tally_events_reads_dated_segments` in
`digest.rs` shows the events-dir fixture pattern).

- `epoch_records_append_and_read_roundtrip` in `epochs.rs`.
- `epoch_spans_are_disjoint_and_tallies_scoped` — seed dated event fixtures in
  two windows; `tally_span` on each sees only its window.
- `tally_lists_capped_counts_exact` — 15 `job_complete` failures →
  `failed_cmds.len() == 10`, `commands_fail == 15`.
- `scan_artifacts_span_until_bound_excludes_newer` — an artifact mtime after
  `until` is excluded (**negative case**).

## End-to-end verification

Not applicable — phase ships no runtime-loadable artifact on its own; the epoch
chain is written only from the compaction path, which 05b wires. State this in
the completion log. (The persistence functions are exercised by the hermetic FS
tests above.)

## Authorizations

None.

## Out of scope

- `compact_with_epochs`, `render_context_block`, the regenerated head — **05b**.
- Rewiring the `should_digest` block in `ask.rs` — **05b**.
- Deleting `compact_with_digest` / `build_session_digest` / the old
  `tally_events` / `scan_artifacts` — **05b** (they stay live until 05b rewires
  the caller).
- Keep-newest narrative truncation — **05b**.
- Chapters / ledger / rollups (`covers` is defined but unused here) — phase 06.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-14 (escalation)

**Chosen lever:** session takeover (minimal)
**Rationale:** Executor hard_failed via the verify-loop pathology
(`IdenticalToolCallRepetition` on `bash` — 6 identical grep/test calls) but had
already written complete, correct code (all 4 functions + 4 tests; builds clean;
3/4 tests passing). The sole failure was a 1-line **test-fixture** bug in
`tally_lists_capped_counts_exact`: it stamped events at ts 15:00:00 but set the
window to `[00:00, 01:00)`, so no events matched (`commands_fail == 0`, not 15).
No spec refinement applies (spec was fine; impl was correct), and a resume would
spend a full executor run — with re-loop risk on this phase's tests — on a fix
already fully diagnosed. Architect fixed the window (`until = since + 24h`) and
completed.

### Update — 2026-07-14 (complete, architect takeover)

**Summary:** The local executor authored `src/daemon/context/epochs.rs` (+514):
`EpochTally`/`EpochRecord`, `epochs_file`/`read_epochs`/`append_epoch` (append-
only), `tally_span` (windowed, capped lists + exact counts), `scan_artifacts_span`
(mtime `[since, until)` → flat tags) + `scan_dir_in_range` helper, and 4 tests —
all matching the spec. Architect's only change: corrected the time window in the
`tally_lists_capped_counts_exact` test fixture. Purely additive — `digest.rs`
(`tally_events`/`scan_artifacts`/`build_session_digest`/`compact_with_digest`)
is unchanged, confirming 05a's additive contract.

**Acceptance criteria:** all met — serde round-trip, append-only (grep
`epochs_file` shows no truncating writer), disjoint-span scoped tallies, cap-10
with exact `commands_fail == 15`, `scan_artifacts_span` excludes mtime `>= until`,
and the four digest.rs symbols unchanged.

**Commands:**

```
cargo fmt --all            → clean
cargo build                → Finished, 0 warnings
cargo clippy --all-targets --all-features -- -D warnings → clean
cargo test                 → 884 passed; 0 failed (unit) + 27 passed (integration)
```

**End-to-end verification:** N/A per the phase doc — 05a ships no runtime-loadable
artifact on its own (the epoch chain is written only from the compaction path,
wired in 05b); the persistence functions are exercised by the hermetic FS tests.

**Files changed:**
- `src/daemon/context/epochs.rs` — new module (executor) + 1-line test-window fix (architect)
- `src/daemon/context/mod.rs` — `pub mod epochs;` (executor)

### Review verdict — 2026-07-14

- **Verdict:** escalated
- **Bounces:** 1 hard_fail (executor verify-loop on a self-authored test bug); no bug docs — resolved by minimal takeover
- **Executor:** AEON-7/Qwen3.6-27B-AEON (wrote the module) → Claude (direct) fixed the test fixture and completed
- **Scope deviations:** none — additive contract honored; digest.rs untouched
- **Calibration:** verify-loop pathology recurs (documented from earlier milestones; the identical-call governor caught it at 6 calls). Distinct from the git-thrash pattern. The split (05a additive) worked as intended: no git-revert this time, and the executor's code survived intact for a trivial takeover.
