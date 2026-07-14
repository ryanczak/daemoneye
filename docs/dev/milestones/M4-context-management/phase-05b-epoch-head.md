# Phase 05b: Epoch chain — regenerated head, keep-newest narrative, retire the digest

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-05a (epoch persistence + `tally_span`/`scan_artifacts_span`)
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=feature, size=l

> **Split note:** this is the second half of the re-split phase-05. 05a landed
> the additive persistence layer (`context/epochs.rs`: `EpochRecord`,
> `EpochTally`, `epochs_file`/`read_epochs`/`append_epoch`, `tally_span`,
> `scan_artifacts_span`). This phase does the **rewire**: build an epoch at each
> compaction, render a regenerated two-slot head, keep the newest turns in the
> narrative input, and delete the old single-digest path.

## Goal

Replace the single regenerated `[Session Digest]` message with the append-only
epoch chain from 05a: each compaction appends one immutable `EpochRecord` and
regenerates a two-message working-set head from the chain. Unpin the stale first
message (D7), stop the oldest-first narrative truncation (D3-partial), and end
the whole-history rescan (D5).

## Architecture references

Read before starting:

- `docs/design/context-management.md#32-epoch-chain--hierarchical-summaries-instead-of-one-regenerated-digest`
- `docs/design/context-management.md#33-working-set-layout-and-token-budgeting`
  — the two-message regenerated head.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above **and** the 05a completion Update Log
   (for the exact `epochs.rs` function signatures as landed).
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors below against the working tree.

## Current state (re-validated 2026-07-14 against the phase-03 takeover code)

**Load-bearing constraints (front-loaded):**

1. Providers require the message list to start with a `user` message and
   alternate roles. The compacted layout is exactly `[slot0, slot1, tail…]`
   where the tail already begins on a clean/repaired boundary from the phase-03
   planner. This phase keeps that **shape**; it only changes the two head slots
   from `[original msg0, digest]` to two **synthetic regenerated** messages.
2. `epochs.jsonl` is append-only (05a) — never rewrite it.
3. Tool-call ↔ tool-result pairing must survive; reuse phase-03's
   `repair_tail_head` exactly as the current code does.

**The compaction call site — `should_digest` block in `src/daemon/server/ask.rs`
(currently lines ~296–367).** This is the phase-03 takeover structure you must
reconcile with — quoted verbatim so you rewire in place rather than reinvent:

```rust
let elided = crate::daemon::digest::elide_old_tool_results(&mut messages, true);
let started_at = /* Some(entry.started_at) or None */;
if let Some(since) = started_at {
    let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;
    let tail_start = crate::daemon::digest::planned_tail_start_by_budget(&messages, budget, session_token_scale)
        .or_else(|| crate::daemon::digest::synthesized_tail_start(&messages, budget, session_token_scale));
    let narrative = if config.digest.narrative_enabled {
        if let Some(ts) = tail_start { if ts > 1 {
            let slice = &messages[1..ts];                 // <-- CHANGE: &messages[..ts] (msg0 is no longer special)
            let model_entry = config.resolve_model(Some("digest"));
            crate::daemon::digest::build_narrative_summary(slice, model_entry).await
        } else { None } } else { None }
    } else { None };
    let digest = crate::daemon::digest::build_session_digest(...);   // <-- DELETE; replace with epoch build + render
    match tail_start {
        Some(ts) => {
            messages = crate::daemon::digest::compact_with_digest(messages, &digest, ts);  // <-- compact_with_epochs
            if 2 < messages.len() { let tail = &mut messages[2..];
                crate::daemon::digest::repair_tail_head(tail); }                            // <-- KEEP unchanged
            log::info!("Compaction (digest): ...");
        }
        None => { log::info!("Compaction: ... no viable tail start ... keeping history as-is"); }  // <-- KEEP
    }
} else { /* trim_history fallback — KEEP unchanged */ }
```

Keep `planned_tail_start_by_budget` / `synthesized_tail_start` / `repair_tail_head`
/ the `None` fallback / the `trim_history` (`session_id == None`) fallback exactly
as they are. Only the narrative-slice bound, the digest construction, and the
compactor call change.

**Other anchors:**
- `src/daemon/digest.rs::build_session_digest` (`:433`) — delete after rewire;
  its `log_event("session_digest_*")` telemetry is replaced by one
  `log_event("epoch_created", …)`. Its per-tally wording fragments
  (`digest.rs:494–563`: "Commands executed…", "Files edited…", "Token usage…",
  "Alerts received…", "Ghost shells…") are the source to reuse for the epoch
  one-liner in `render_context_block`.
- `src/daemon/digest.rs::compact_with_digest` (`:~700`, 3-arg pure cutter from
  phase 03) — delete after `compact_with_epochs` replaces it; migrate its tests
  (`compact_preserves_first_and_tail`, `compact_tail_starts_on_user_turn`,
  `compact_noop_when_tail_start_infeasible`, `planned_tail_start_matches_*`,
  and the orphan/synthesized tests) to `compact_with_epochs`.
- `src/daemon/digest.rs::format_messages_for_narrative` (`:299`) — the
  oldest-first truncation is the `if out.len() >= NARRATIVE_INPUT_CHAR_BUDGET
  { … break; }` at `:339`.
- The old `digest::tally_events` (`:68`) / `scan_artifacts` (`:144`) become
  **unused** once `build_session_digest` is deleted and the epoch build calls
  05a's `tally_span`/`scan_artifacts_span`. Delete them too (and their now-dead
  private `EventTally`/`ArtifactChanges` structs), migrating
  `tally_events_reads_dated_segments` to `tally_span`.
- The `Response::SystemMsg` compaction notice — `src/daemon/server/ask.rs:627`:
  `"↩ Session history compacted ({} messages → {}) — full context preserved in digest"`.
- `config.context.environment` and `crate::daemon::daemon_hostname()` for the
  regenerated header (see `src/daemon/prompt.rs:~108` for usage).

## Spec

### 1. `compact_with_epochs` — in `context/epochs.rs`

```rust
/// Layout: [synthetic user "[Session Context] …", synthetic assistant ack,
/// messages[tail_start..]]. `tail_start` MUST already be a clean/repaired
/// boundary (phase-03 planner). Returns `messages` unchanged when `tail_start`
/// is infeasible (`< 2` or `>= len`) — same guard as the old compact_with_digest.
pub fn compact_with_epochs(messages: Vec<Message>, rendered_context: &str, tail_start: usize) -> Vec<Message>
```

Pinned head contents:

- **Slot 0 (`role="user"`):**
  ```
  [Session Context — regenerated at compaction; turns 1..{turn_end} summarized]
  Environment: {environment} · Daemon host: {host}

  {rendered_context}

  Older turns are preserved in the session archive.
  ```
  `{turn_end}` = the last turn covered by the newest epoch (0 if unknown).
  Environment/host are passed in by the caller (the function stays pure — take
  them as `&str` params, do not read config inside `epochs.rs`).
- **Slot 1 (`role="assistant"`):**
  `Continuing session — the context above covers everything before turn {tail_first_turn}.`
  where `tail_first_turn` = `messages[tail_start].turn` (or 0 / "the tail" if None).
- Both: `tool_calls: None, tool_results: None, turn: None`.

`messages[0]` is **not** special-cased — it lands in the dropped span like any
message (it is in the archive, phase 04). This is the D7 fix.

### 2. `render_context_block` — in `context/epochs.rs`

```rust
/// Render the epoch chain for the working-set head. The last RENDER_EPOCHS (8)
/// epochs, newest last — each as "Epoch {seq} (turns {a}–{b}): {line}" where
/// {line} is the narrative (trimmed to one paragraph) or, when absent, a tally
/// one-liner. Older epochs collapse to a single line:
/// "…{n} earlier epochs — chapter rollups arrive in a later phase."
/// (Phase 06 replaces that line with ledger + chapters.)
pub const RENDER_EPOCHS: usize = 8;
pub fn render_context_block(epochs: &[EpochRecord]) -> String
```

The tally one-liner reuses the wording fragments from the old
`build_session_digest` (commands ok/fail, files-edited count, alerts count,
ghost starts) — condensed to one line, e.g.
`"12 cmds (2 failed) · 3 files edited · 1 alert"`.

### 3. Build an epoch at compaction — rewire the `should_digest` block (`ask.rs`)

Replace the `build_session_digest` construction and the `compact_with_digest`
call (see the quoted block above):

```rust
use crate::daemon::context::epochs::{self, EpochRecord, EpochTally};
// after tail_start is computed and narrative built (with slice = &messages[..ts]):
if let Some(ts) = tail_start {
    let id = session_id.as_deref().unwrap_or("-");
    let prior = epochs::read_epochs(id);
    let span_start = prior.last().map(|e| e.ts_end).unwrap_or(since);
    let span_end = chrono::Utc::now();
    let dropped = &messages[..ts];
    let record = EpochRecord {
        seq: prior.last().map(|e| e.seq + 1).unwrap_or(1),
        kind: "epoch".into(),
        turn_start: first_turn_of(dropped),
        turn_end: last_turn_of(dropped),
        ts_start: span_start,
        ts_end: span_end,
        msg_count: dropped.len() as u32,
        narrative,
        tally: epochs::tally_span(id, span_start, span_end),
        artifacts: epochs::scan_artifacts_span(span_start, span_end),
        covers: None,
    };
    epochs::append_epoch(id, &record);
    log_event("epoch_created", serde_json::json!({"session": id, "seq": record.seq, "turns": [record.turn_start, record.turn_end], "msgs": record.msg_count}));
    let chain = epochs::read_epochs(id);
    let env = config.context.environment.clone();
    let host = crate::daemon::daemon_hostname();
    let rendered = epochs::render_context_block(&chain);
    let rendered_head = /* format Slot 0 body from env/host/rendered/turn_end */;
    messages = epochs::compact_with_epochs(messages, &rendered_head, ts);
    if 2 < messages.len() { let tail = &mut messages[2..]; crate::daemon::digest::repair_tail_head(tail); }
    log::info!("Compaction (epoch {}): tokens {}% — compacted {} → {} messages", record.seq, token_pct, pre_trim_len, messages.len());
}
```

`first_turn_of`/`last_turn_of` = min/max of `Message.turn` over the slice,
defaulting to 0 (put them as small private helpers in `epochs.rs` or `ask.rs`).
Keep the `None`-tail_start and `session_id == None` fallbacks unchanged.

Note the head-body string (`rendered_head`) is Slot 0's body per §1; the
function prepends the `[Session Context …]` framing — decide whether the framing
lives in `compact_with_epochs` (cleaner) or the caller, and keep it consistent
with the §1 pin. Prefer: `compact_with_epochs` builds the full Slot 0 text from
`rendered_context` + the framing, taking `environment`, `host`, `turn_end`,
`tail_first_turn` as params.

### 4. Keep-newest narrative truncation — `format_messages_for_narrative`

Rewrite the oldest-first truncation (`digest.rs:339`) to keep the **newest**
messages: iterate the slice in reverse, accumulate serialized chunks until
`NARRATIVE_INPUT_CHAR_BUDGET`, then emit in chronological order with a leading
`[…older dropped turns omitted from summarizer input…]` marker.

### 5. Delete the retired path + migrate tests

- Delete `build_session_digest`, `compact_with_digest`, `tally_events`,
  `scan_artifacts`, and the now-dead `EventTally` / `ArtifactChanges` structs.
- Migrate their tests to the epoch equivalents (see Current state list). The
  behavioral assertions that still apply (first/second head slot, tail on a
  clean user turn, no orphan tool_result, budget cut) move to
  `compact_with_epochs`; the `[Session Digest]`-string assertions are replaced
  by `[Session Context]` assertions.
- Change the `SystemMsg` notice (`ask.rs:627`) to:
  `"↩ Session history compacted ({} → {} messages) — epoch {} recorded; older turns in the session archive"`.
- `grep -rn "Session Digest" src/ assets/` must be empty afterward (update
  `assets/prompts/sre.toml` only if the grep finds it there).

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] Two successive compactions append `seq 1` then `seq 2` records whose spans
      abut (`epoch2.ts_start == epoch1.ts_end`) and whose tallies count only
      their own span (test).
- [ ] The compacted head is `[user "[Session Context]…", assistant "Continuing
      session…", clean-boundary tail…]`; the original first message is absent
      from the working set (test).
- [ ] `render_context_block` with 12 epochs renders the last 8 + one "…4 earlier
      epochs" line (test).
- [ ] Keep-newest truncation: over-budget summarizer input retains the newest
      message and drops the oldest (rewritten test).
- [ ] Orphan-safety: phase-03's `assert_no_orphan_tool_results` helper passes on
      `compact_with_epochs` output (clean, synthesized, and repaired paths).
- [ ] `grep -rn "Session Digest" src/ assets/` and `grep -rn "compact_with_digest\|build_session_digest" src/` are empty (retired-path fully removed).

## Test plan

FS tests take `TEST_HOME_LOCK` + temp HOME.

- `epoch_built_at_compaction_spans_abut` — extract the epoch-build into a
  `pub(crate) fn` so it is testable without a live daemon (prefer this over
  `#[cfg(test)]`-gating the block); two builds; spans abut, tallies scoped.
- `compact_with_epochs_head_shape` — user/assistant/tail-user; original msg0
  absent; orphan checker green.
- `render_context_block_caps_at_eight`.
- `narrative_input_keeps_newest` — rewrite of
  `format_messages_for_narrative_truncates_at_budget` (which currently asserts
  the opposite retention — flip it).

## End-to-end verification

Drive the compaction path via the integration harness (`narrative_enabled =
false` → structured-only, no live model):

1. Construct a session JSONL + fixture events in a temp HOME, invoke the
   epoch-build entry twice, then `cat` the resulting `.epochs.jsonl` and quote
   both records in the completion log.

## Authorizations

- [ ] May touch `docs/architecture.md` §1.2 (orchestration layer): add one
      sentence noting compaction history is an epoch chain per
      `docs/design/context-management.md`.

## Out of scope

- Chapters / ledger / rollups — phase 06 (`covers` and the "…n earlier epochs"
  line are their hook points).
- The recall tool — phase 07.
- Async execution — phase 08 (everything here stays synchronous in the
  `should_digest` block).
- Memory extraction — phase 10.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
