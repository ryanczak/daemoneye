# Phase 08: Asynchronous compaction with emergency fallback

**Milestone:** M4 — Context Management Overhaul
**Status:** done (architect takeover — executor hard_failed twice on the ask.rs
rewire; architect finished the last mile).
**Depends on:** phase-05 (epoch build), phase-06 (rollups)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Get the epoch build (events scan + up-to-20 s narrative call) off the
interactive path (design defect D11): compaction runs as a background tokio
task after the turn completes and swaps the compacted working set in before
the next turn, with a staleness check. A synchronous **emergency path** at
`emergency_pct` (85%) — structured-only, no summarizer call — remains the
backstop. With the stall gone, `[digest] narrative_enabled` flips its
default to `true`.

## Architecture references

Read before starting:

- `docs/design/context-management.md#35-async-compaction`
- `docs/architecture.md#21-interactive-requestresponse` — the request flow
  that must not stall.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors against phases 05/06 as landed —
   in particular the exact shape of the epoch-build entry point they
   produced.

## Current state

**The load-bearing constraints (front-loaded):**

1. `SessionStore` is `Arc<Mutex<HashMap<…>>>` with a **std::sync::Mutex**
   (`src/daemon/session.rs:104`). A `MutexGuard` is not `Send` — you
   CANNOT hold the lock across an `.await`. The background task must do ALL
   async work (the narrative/rollup model calls) on **owned snapshots**
   first, then take the lock once, synchronously, for the swap. Structure
   the task as: clone-out → await → lock-check-swap-unlock.
2. Every lock site uses `.unwrap_or_log()` (`src/util.rs` `UnpoisonExt`) —
   never `.unwrap()`.
3. Clone what the task needs **before** spawning (session_id `String`,
   the message snapshot `Vec<Message>`, the resolved `ModelEntry` /
   `CompactionConfig` values). Never move borrowed references into
   `tokio::spawn`.

Key existing code (post-05/06):

- The synchronous compaction block in `src/daemon/server/ask.rs`
  (`should_digest` arm): elide → build epoch (narrative + tally) →
  `append_epoch` → `maybe_rollup` → `render_context_block` →
  `compact_with_epochs` → `needs_compaction = true` →
  `write_session_file` happens later in `stream.rs:671`.
- Long-running supervised tasks use `supervise(name, shutdown, factory)`
  (`src/daemon/mod.rs:~640`) — NOT needed here; per-compaction
  `tokio::spawn` is fine (one-shot task, failure tolerated).
- `SessionEntry` fields land in `src/daemon/session.rs:21` (many
  construction sites — the compiler finds them; phase 02 already walked
  them once).
- `Response::SystemMsg` is how compaction notices reach the client
  (`ask.rs:565-572`).

## Spec

### 1. New `SessionEntry` state — `src/daemon/session.rs`

```rust
/// True while a background compaction task for this session is running.
/// Prevents duplicate spawns; cleared by the task on completion/discard.
pub compaction_in_flight: bool,
/// Notice queued by a completed background compaction, delivered as a
/// SystemMsg at the start of the next turn.
pub pending_compaction_notice: Option<String>,
```

Defaults `false` / `None` at every construction site.

### 2. Split the decision — `src/daemon/server/ask.rs`

Rework the threshold ladder:

```
token_pct >= emergency_pct (85)  → SYNCHRONOUS structured-only compaction
                                   (aggressive elision + epoch with
                                   narrative=None + rollup + compact) —
                                   today's block minus the summarizer call.
token_pct >= compact_at_pct (60) → aggressive elision NOW (cheap, sync);
                                   set a `wants_background_compaction` flag
                                   for step 3. Do NOT cut the history here.
token_pct >= elide_at_pct (50)   → soft elision (unchanged).
max_history safety cap           → stays on the SYNCHRONOUS path (it exists
                                   precisely for when token info is absent).
```

At the top of the turn, drain `pending_compaction_notice` and send it as a
`SystemMsg` (alongside the existing compaction-notice send site).

### 3. Background task — `src/daemon/context/background.rs` (new)

```rust
/// Spawn the deferred compaction for `session_id` if none is in flight.
/// Called from the end-of-turn write-back in stream.rs.
pub fn spawn_compaction(session_id: String, sessions: SessionStore,
                        config_snapshot: /* owned pieces */)
```

Task body, pinned order:

1. **Snapshot** (lock, clone, unlock): `messages.clone()`,
   `turn_count`, `messages.len()`, `token_scale`, `started_at`; set
   `compaction_in_flight = true`. If already true, or the entry is a ghost
   (`is_ghost`), return without spawning.
2. **Async work** (no lock held): plan the cut (phase 03 planner on the
   snapshot), build the epoch (narrative allowed — this is the whole
   point), `append_epoch`, `maybe_rollup`, render, `compact_with_epochs`
   on the snapshot.
3. **Swap** (lock once, synchronous):
   - Entry gone (evicted)? → discard; done. (The epoch record already
     appended is harmless — it describes real history; the next load simply
     has one epoch whose messages are still in the working file. Document
     this in a comment.)
   - `entry.turn_count != snapshot_turn_count ||
     entry.messages.len() != snapshot_len` → **discard** the compacted vec
     (a turn ran while we worked). Clear `compaction_in_flight`; the next
     turn's end re-spawns with fresh data. The appended epoch stays; the
     re-spawned build MUST NOT double-create it — pin this by having step 2
     skip epoch creation when `read_epochs().last()` already covers
     `turn_end >= snapshot`'s last turn (idempotency guard, unit-tested).
   - Match → `entry.messages = compacted`, set
     `pending_compaction_notice = Some("↩ Session history compacted in the
     background ({before} → {after} messages) — epoch {seq} recorded")`,
     clear the flag.
4. **Persist** (still holding? NO —) after releasing the lock, call
   `write_session_file(&session_id, &compacted)` with the swapped vec
   (the same clone), then `log_event("compaction", …)` with an added
   `"mode": "background"` field. (File write outside the lock is safe: the
   per-turn append path only runs during a turn, and a turn arriving now
   would have failed the staleness check — a comment must state this
   ordering argument.)

### 4. Spawn point — `src/daemon/stream.rs`

At the end-of-turn write-back (where `entry.last_prompt_tokens` is updated
and the session file appended, ~line 657-678): if the turn's
`wants_background_compaction` was set (thread it through
`ConversationLoopCtx` — one new bool field) and the session is not a ghost,
call `spawn_compaction(…)`.

### 5. Narrative default flip — `src/config/types.rs`

`DigestConfig.narrative_enabled` default becomes `true`
(`#[serde(default = "default_true")]` with the fn, matching existing
default-fn style). Update the doc comment: cost is per-epoch and off the
interactive path as of this phase. Update any config tests pinning the old
default.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] Staleness: a task whose snapshot is one turn behind discards its
      result — the session's messages are untouched and
      `compaction_in_flight` is false afterward (test below).
- [ ] Idempotency: a discarded-then-retried build does not append a second
      epoch covering the same turns (test below).
- [ ] Emergency path: at ≥ 85% the turn compacts synchronously with
      `narrative == None` even when `narrative_enabled = true` (no
      summarizer call — assert via the no-model test environment: the
      structured path must not attempt a network call; reuse how phase 05
      tests run with `narrative_enabled = false`, but here the *flag is
      true* and the emergency path must still skip it — negative case).
- [ ] Duplicate-spawn guard: two spawn calls, one in-flight → second is a
      no-op.
- [ ] Eviction race: entry removed between snapshot and swap → clean
      discard, no panic (test with a store the test empties mid-flight).
- [ ] No `.await` while a `MutexGuard` is alive in the new code (clippy's
      `await_holding_lock` is part of `-D warnings` — it must stay green).
- [ ] `narrative_enabled` defaults to true (config test updated).

## Test plan

Use `#[tokio::test(start_paused = true)]` where timing matters
(`tokio = ["test-util"]` is already in dev-dependencies — see the
`supervise` tests in `src/daemon/mod.rs` for the idiom).

- `background_swap_applies_when_unchanged` in `context/background.rs` —
  seed store, run the task body fn (extract it as a testable
  `async fn run_compaction(…)` that `spawn_compaction` wraps), assert swap +
  notice + flag cleared.
- `background_swap_discards_on_new_turn` — bump `turn_count` after
  snapshot; assert discard.
- `epoch_build_idempotent_after_discard` — run twice over the same
  snapshot; one epoch record.
- `spawn_is_noop_when_in_flight`.
- `swap_discards_on_evicted_entry`.
- `emergency_path_skips_narrative_with_flag_on` — in `ask.rs`/`digest.rs`
  test scope.
- `notice_delivered_next_turn` — entry with a queued notice; the drain site
  returns it and clears the field (pure state test).

## End-to-end verification

Not applicable as a live-daemon check without a >60%-full session and a
running model; the async behavior is pinned by the paused-clock tests. In
the completion log, quote the `run_compaction` test output and the clippy
run proving `await_holding_lock` is clean.

## Authorizations

None.

## Out of scope

- Ghost sessions never spawn background compactions (`is_ghost` guard here;
  their synchronous coverage is phase 10).
- No debouncing/scheduling beyond one-task-per-session-in-flight.
- No change to elision thresholds or the budget planner (phase 03 owns
  them).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-15 (escalation)

**Chosen lever:** session takeover
**Rationale:** Two consecutive `hard_fail`s, both `NoProgressStall` on the
`ask.rs` step-2 rewire — the documented Qwen git-thrash/orient-paralysis
pathology that already forced takeover on phases 03/05b/06; the executor left
a near-complete tree (one missing struct field from building), so takeover is
a cheap last-mile finish and resume would only re-drop it onto the wall it
failed twice.

### Update — 2026-07-15 (architect takeover — complete)

**Executor:** Claude (direct)
**Verdict:** escalated

The executor's two runs left a near-complete scaffold: `background.rs` (+408),
the `SessionEntry` fields, the `ConversationLoopCtx` thread, and the config
narrative-default flip were all on disk and correct. Run 1 self-reverted
`ask.rs` (via `git show HEAD:… > /tmp && cp`) after a bad edit and thrashed;
run 2 could not orient on the partial tree. Architect finished the last mile:

- **`ask.rs` step-2 ladder** — reconstructed the reverted rewire. Fixed four
  defects the self-revert left: (1) `wants_background_compaction` was never
  declared/set/threaded (the E0063 build break); (2) the safety-cap arm was
  gated on `is_compact` so the `max_history` net was defeated when token info
  is absent — restored to `(is_emergency || at_safety_cap) && above_floor`;
  (3) the 50 % soft-elide branch was dropped and `is_elide` left unused
  (clippy `-D warnings` break) — restored; (4) `needs_compaction` no longer
  forced persistence for the in-place elision branches — re-added via
  `did_inline_elide`. Added the `pending_compaction_notice` drain + SystemMsg
  at the top of the turn.
- **`stream.rs` spawn site** — the executor held the `sessions` lock while
  calling `spawn_compaction`, which re-locks the same non-reentrant
  `std::sync::Mutex` → **deadlock**. Dropped the redundant lock (spawn_compaction
  guards `is_ghost`/in-flight internally).
- **`background.rs`** — converted all four lock sites to the `.unwrap_or_log()`
  invariant; removed the diverging `let _swap_result = { … }` block; gated the
  narrative model call on `narrative_enabled` (the executor called it
  unconditionally) via a new pure `epoch_narrative_allowed(is_emergency, flag)`
  helper; **fixed the idempotency guard** — it compared `turn_end` against the
  *whole snapshot's* last turn (never true, so it never fired and would
  double-create epochs) → now compares against the dropped span's last turn,
  computed after `tail_start`.
- **Tests** — the executor shipped 3 of the 7 required. Added the missing four:
  `background_swap_applies_when_unchanged`, `epoch_build_idempotent_after_discard`,
  `swap_discards_on_evicted_entry`, `emergency_path_skips_narrative_with_flag_on`.
- **Collateral hermeticity fix** — the new HOME-mutating tests exposed a
  pre-existing gap: `recall::recall_truncates_at_cap_utf8_safe` touched
  `~/.daemoneye` without holding `TEST_HOME_LOCK` (the lone recall test missing
  the `TestHome` guard). Added the guard; suite is now deterministic (3× clean).

**Gates (all green):**

```
running 7 tests
test daemon::context::background::tests::emergency_path_skips_narrative_with_flag_on ... ok
test daemon::context::background::tests::notice_delivered_next_turn ... ok
test daemon::context::background::tests::background_swap_discards_on_new_turn ... ok
test daemon::context::background::tests::spawn_is_noop_when_in_flight ... ok
test daemon::context::background::tests::epoch_build_idempotent_after_discard ... ok
test daemon::context::background::tests::swap_discards_on_evicted_entry ... ok
test daemon::context::background::tests::background_swap_applies_when_unchanged ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 893 filtered out
```

`cargo clippy --all-targets --all-features -- -D warnings` → clean (the
`await_holding_lock` lint, part of `-D warnings`, stays green — all `.await`s
in `run_compaction` occur before the swap guard is taken). Full suite: 900 unit
+ 27 integration passing, 3 consecutive clean runs.
