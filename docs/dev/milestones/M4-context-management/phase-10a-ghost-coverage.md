# Phase 10a: Ghost-session working-set compaction coverage

**Milestone:** M4 — Context Management Overhaul
**Status:** review
**Depends on:** phase-03 (planner/elision), phase-05 (epochs), phase-08 (ladder shape)
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Give autonomous ghost sessions the same context relief interactive sessions
have (design defect D13): a tool-heavy ghost on a small local model (e.g. 32k)
today sends its full, ever-growing history to `.chat` every iteration with
**no compaction of any kind** and eventually blows the window. Add a
**synchronous, model-call-free** working-set guard that elides and, above the
compact threshold, builds a structured-only epoch (no narrative call) inside
the ghost turn loop.

This is 10a of the 10a/10b split of the original phase-10 (10b = opt-in memory
extraction, drafted separately). Kept apart because they are independent
subsystems (ghost loop vs. epoch-build + memory) and this executor stalls on
multi-subsystem phases.

## Architecture references

Read before starting:

- `docs/design/context-management.md#38-ghost-coverage-and-memory-extraction`
- `docs/architecture.md#3-the-ghost-shell-subsystem` — the turn loop this
  phase instruments.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors — every prior M4 phase landed; the
   ghost loop and epoch entry points may have drifted from the line numbers
   below (re-grep by symbol).

## Current state

*(Anchors verified 2026-07-16 against HEAD after phase-09.)*

- **Ghost turn loop:** `trigger_ghost_turn` (`src/daemon/ghost.rs:287`). Each
  iteration clones the session history under the store lock, drops the lock,
  then spawns `.chat` with **no compaction**:

  ```rust
  // ghost.rs:485-503 (verified)
  let (chat_messages, loaded_tools) = {
      let store = sessions.lock().unwrap_or_log();
      let Some(entry) = store.get(session_id) else { break; };
      (
          entry.messages.clone(),
          entry.loaded_tools.iter().cloned().collect::<Vec<String>>(),
      )
  };
  // ... then:
  tokio::spawn(async move {
      client_clone.chat(&system_clone, chat_messages, ai_tx, true, loaded_tools).await
  });
  ```

  This is the insertion point: run the guard on `chat_messages` **after the
  lock drops, before the spawn**, then (if it compacted) write the result back.
- **Ghost persistence is append-only + archived:** ghost messages are written
  via `append_session_message` (`ghost.rs:213/470/983`), which since phase 04
  also appends to `<id>.archive.jsonl`. So the full history is preserved in the
  archive, and rewriting the *working* file on compaction is safe (loses
  nothing). `write_session_file` is NOT yet imported in `ghost.rs` — add it.
- **Available at the call site:** `model_entry` is resolved once before the
  loop (`ghost.rs:416`, `config.resolve_model(ghost_active_model.as_deref())`)
  → `model_entry.context_window()`. `entry.started_at` (:235) and
  `entry.token_scale` (:249, default 1.5) are on the entry — grab them in the
  same lock block as the clone.
- **Ghosts are NOT token-calibrated** (the stream.rs write-back that updates
  `last_prompt_tokens`/`token_scale` is the interactive path). So pressure MUST
  use the phase-02 estimate exclusively: `estimate_history_tokens`
  (`context/estimate.rs:29`) × `token_scale`, never `last_prompt_tokens`.
- **Epoch build primitives (all synchronous):** `DIGEST_THRESHOLD`
  (`digest.rs:24`), `elide_old_tool_results` (`digest.rs:342`),
  `planned_tail_start_by_budget` (`digest.rs:250`), `synthesized_tail_start`
  (`digest.rs:269`), `repair_tail_head` (`digest.rs:313`); `epochs.rs`:
  `EpochRecord` (:63), `read_epochs` (:90), `append_epoch` (:113),
  `first_turn_of`/`last_turn_of` (:441/:450), `tally_span` (:511),
  `scan_artifacts_span` (:590), `render_context_block` (:322),
  `compact_with_epochs` (:462).
- **`maybe_rollup` (`epochs.rs:176`) is ASYNC and makes a model call** when
  `narrative_enabled` is true (now the default) — building a chapter narrative.
  Therefore the ghost path **must NOT call `maybe_rollup`**: it would break the
  model-call-free requirement and force the helper async. Skip rollup entirely
  (ghost epoch chains are bounded by `max_ghost_turns`, default 20). This is the
  one deliberate divergence from the interactive ladder.
- **`context/mod.rs`** currently only declares submodules
  (`background`/`epochs`/`estimate`/`recall`) — the new helper goes in its own
  file `context/ghost_ws.rs`, NOT in `mod.rs`.

## Spec

### 1. New helper — `src/daemon/context/ghost_ws.rs` (new file)

Add `pub mod ghost_ws;` to `context/mod.rs`.

```rust
/// Synchronous, model-call-free working-set control for autonomous (ghost)
/// sessions. No `.await`, no network/model call, no lock held — operates on the
/// owned `messages` and returns the possibly-compacted vec plus a `compacted`
/// flag (true → the caller must rewrite the working session file).
///
/// Ladder (estimate-based pressure only; ghosts are not token-calibrated):
///   token_pct >= compact_at_pct  → aggressive elision + structured-only epoch
///                                  (narrative = None) + compact. NO rollup
///                                  (maybe_rollup can make a model call).
///   token_pct >= elide_at_pct    → soft elision only.
///   below elide_at_pct, or below DIGEST_THRESHOLD messages → strict no-op.
pub fn enforce_ghost_working_set(
    session_id: &str,
    messages: Vec<Message>,
    token_scale: f64,
    started_at: chrono::DateTime<chrono::Utc>,
    context_window: u32,
    config: &Config,
) -> (Vec<Message>, bool)
```

**Worked example — this is the interactive emergency path (`ask.rs:335-410`)
reduced to a pure function.** Build the ghost helper by mirroring this shape,
dropping the `session_id: Option` / lock / `Response` plumbing and the
`maybe_rollup` call:

```rust
// pressure: phase-02 estimate only (NOT last_prompt_tokens)
let token_pct = if context_window > 0 {
    ((crate::daemon::context::estimate::estimate_history_tokens(&messages) as f64
        * token_scale) / context_window as f64 * 100.0) as u32
} else {
    0
};
let above_floor = messages.len() >= crate::daemon::digest::DIGEST_THRESHOLD;
if !above_floor || token_pct < config.compaction.elide_at_pct {
    return (messages, false); // strict no-op
}
let mut messages = messages;
if token_pct >= config.compaction.compact_at_pct {
    // structured-only epoch (mirror ask.rs:335-405, no narrative, no rollup)
    crate::daemon::digest::elide_old_tool_results(&mut messages, true);
    let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;
    let tail_start = crate::daemon::digest::planned_tail_start_by_budget(&messages, budget, token_scale)
        .or_else(|| crate::daemon::digest::synthesized_tail_start(&messages, budget, token_scale));
    if let Some(ts) = tail_start {
        let prior = epochs::read_epochs(session_id);
        let span_start = prior.last().map(|e| e.ts_end).unwrap_or(started_at);
        let span_end = chrono::Utc::now();
        let dropped = &messages[..ts];
        let record = epochs::EpochRecord {
            seq: prior.last().map(|e| e.seq + 1).unwrap_or(1),
            kind: "epoch".into(),
            turn_start: epochs::first_turn_of(dropped),
            turn_end: epochs::last_turn_of(dropped),
            ts_start: span_start,
            ts_end: span_end,
            msg_count: dropped.len() as u32,
            narrative: None, // structured-only — ghosts never call the model here
            tally: epochs::tally_span(session_id, span_start, span_end),
            artifacts: epochs::scan_artifacts_span(span_start, span_end),
            covers: None,
        };
        epochs::append_epoch(session_id, &record);
        let chain = epochs::read_epochs(session_id);
        let rendered = epochs::render_context_block(&chain);
        let env = config.context.environment.clone();
        let host = crate::daemon::utils::daemon_hostname();
        messages = epochs::compact_with_epochs(messages, &rendered, &env, &host,
            record.turn_end as usize, 0, ts);
        if 2 < messages.len() {
            crate::daemon::digest::repair_tail_head(&mut messages[2..]);
        }
        return (messages, true);
    }
    // no viable cut — the aggressive elision above still changed content in place
    return (messages, true);
}
// elide-only tier
let elided = crate::daemon::digest::elide_old_tool_results(&mut messages, false);
(messages, elided > 0)
```

(`chrono::Utc::now()` is fine in production code — it is only forbidden in
rexyMCP *workflow scripts*, not in the daemon.)

### 2. Wire it into the ghost loop — `src/daemon/ghost.rs`

- Add `use crate::daemon::session::write_session_file;` (alongside the existing
  `append_session_message` import at `ghost.rs:10`).
- In the clone block (`:485`), also pull `started_at` and `token_scale` out of
  the entry.
- After the lock drops, before the `tokio::spawn`, run the guard and write back
  when it compacted:

  ```rust
  let (chat_messages, compacted) = crate::daemon::context::ghost_ws::enforce_ghost_working_set(
      session_id,
      chat_messages,
      token_scale,
      started_at,
      model_entry.context_window(),
      config,
  );
  if compacted {
      // In-memory write-back is REQUIRED — the next iteration re-clones
      // entry.messages, so the compaction must persist across iterations.
      {
          let mut store = sessions.lock().unwrap_or_log();
          if let Some(entry) = store.get_mut(session_id) {
              entry.messages = chat_messages.clone();
          }
      }
      // Working-file rewrite (archive preserves full history; ghost files are
      // append-only + archived since phase 04, so this loses nothing).
      write_session_file(session_id, &chat_messages);
  }
  ```

  Then the existing `tokio::spawn` sends `chat_messages` (now the compacted vec)
  to `.chat`. Keep `.unwrap_or_log()` on the lock; the guard itself holds no
  lock and is fully synchronous, so there is no `.await`-under-guard risk.

Ghost epochs are ordinary epoch records (same file, same `seq` space); the
`[Session Context]` head that `compact_with_epochs` produces works identically
for the ghost's next iteration.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean; no `.await` in
      `enforce_ghost_working_set` (it is a synchronous `fn`).
- [ ] A ghost history driven over `compact_at_pct` (via a small
      `context_window_tokens` override in the test config) compacts inside the
      helper: result estimate ≤ target, an epoch record is appended with
      `narrative == None`, the orphan checker is green, and **no model/network
      call occurs** (assert by running with no backend configured — the helper
      must not attempt one).
- [ ] Below `elide_at_pct` (or below `DIGEST_THRESHOLD` messages) the helper is
      a **strict no-op**: returns the input vec unchanged, `compacted == false`,
      no epoch appended (**negative case**).
- [ ] The elide-only tier (`elide_at_pct ≤ pct < compact_at_pct`) elides in
      place and returns `compacted == true` when anything was elided, without
      appending an epoch.
- [ ] `maybe_rollup` is NOT called from the ghost path (grep the new file — it
      must not reference `maybe_rollup`).

## Test plan

FS tests take `TEST_HOME_LOCK` + temp HOME (epoch append writes under HOME).
Reuse the real tool-message fixtures pattern — build messages with actual
`ToolResult`/`ToolCall` (see `digest.rs:544` or `session.rs`'s
`make_msg_with_tool_results` from phase-09), NOT bare `make_msg`, so the orphan
checker is meaningful.

- `ghost_guard_noop_below_threshold` — few messages / low pressure → returns
  input unchanged, `!compacted`, `read_epochs` empty.
- `ghost_guard_compacts_structured_only` — tiny `context_window` + enough
  messages; assert `compacted`, one epoch appended, `record.narrative.is_none()`,
  result len < input len.
- `ghost_guard_output_orphan_free` — replicate the `assert_no_orphan_tool_results`
  helper (phase-09 added one in `session.rs`; it is test-module-private, so
  replicate the ~8-line check) and assert it on the compacted result.
- `ghost_guard_elide_only_tier` — pressure between elide and compact; assert no
  epoch appended but `compacted` reflects whether elision changed anything.

Force a viable budget cut deterministically by setting a large `token_scale` in
the test (the `MIN_TAIL_MESSAGES` floor then lands the cut) — the same trick
`background.rs`'s `background_swap_applies_when_unchanged` test uses.

## End-to-end verification

Not a live-daemon check (no running ghost/model needed). In the completion log,
quote the `ghost_guard_compacts_structured_only` test output (showing the
appended epoch with `narrative: None` and before/after message counts) and the
clippy run proving the synchronous helper has no `await_holding_lock` issue.

## Authorizations

- [ ] May touch `docs/architecture.md` §3 (Ghost Shell subsystem): one sentence
      — ghost turn loops enforce the working set synchronously (structured-only,
      no model call), per the M4 design.

## Out of scope

- **No narrative/model call anywhere in the ghost path** — structured-only.
- **No `maybe_rollup`** in the ghost path (it can make a model call; ghosts stay
  model-call-free).
- No memory extraction — that is phase 10b.
- No changes to ghost turn budgets, policies, or mailbox behavior.
- No change to how ghosts persist messages beyond the compaction-time working
  file rewrite (still append-only + archived the rest of the time).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Update — 2026-07-17 00:00 (started)

**Started by:** executor (phase-10a)

Implemented `enforce_ghost_working_set` in `src/daemon/context/ghost_ws.rs` and wired it into the ghost turn loop in `src/daemon/ghost.rs`.

<!-- entries appended below this line -->
### Update — ts=1784247808554 (complete, server-authored)

**Summary:** All verification commands passed. Here's the summary:

**What was built:** Implemented the synchronous, model-call-free ghost working-set compaction guard (`enforce_ghost_working_set` in `src/daemon/context/ghost_ws.rs`) and wired it into the ghost turn loop in `ghost.rs`. The guard provides a three-tier ladder: below `elide_at_pct` → strict no-op; between `elide_at_pct` and `compact_at_pct` → soft elision only; above `compact_at_pct` → aggressive elision + structured-only epoch (narrative = None) + working-set compaction. When compaction occurs, the in-memory session store and the on-disk working file are both rewritten.

**Deviation from spec:** The spec's worked example showed the compacted path returning `(messages, true)` when no viable cut exists but elision changed content. The implementation matches this behavior — aggressive elision runs before the budget cut attempt, so even if no viable tail_start is found, elision alone triggers a rewrite.

**E2E verification:** All 4 unit tests pass (`ghost_guard_noop_below_threshold`, `ghost_guard_compacts_structured_only`, `ghost_guard_output_orphan_free`, `ghost_guard_elide_only_tier`). Clippy is clean with `-D warnings`. `cargo fmt` and `cargo test` both pass. The `maybe_rollup` grep confirms it appears only in a comment in `ghost_ws.rs` (not called). The `narrative: None` literal is present in the epoch record construction.

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.69s


LINT
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.58s


TEST
g ... ok
test webhook::server::tests::auth_missing_header_denies ... ok
test webhook::server::tests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_correct_token_allows ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test sys_context::tests::compact_memory_excludes_header_row ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test memory::tests::memory_without_frontmatter_has_no_tags ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 909 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.46s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test event_log_entry_format ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.61s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**
- `docs/dev/milestones/M4-context-management/README.md` — +1 -1
- `docs/dev/milestones/M4-context-management/phase-10a-ghost-coverage.md` — +7 -1
- `src/daemon/context/ghost_ws.rs` — +315 -0
- `src/daemon/context/mod.rs` — +1 -0
- `src/daemon/ghost.rs` — +25 -2

**Commit:** 464aec30218379de17b74f9c26eb484db9afcb81

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

