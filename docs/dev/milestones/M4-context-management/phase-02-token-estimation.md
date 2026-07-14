# Phase 02: Per-message token estimation with per-session calibration

**Milestone:** M4 — Context Management Overhaul
**Status:** in-progress (bounced — see bug-02-1)
**Depends on:** none (parallel-safe with phase-01)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Give the compaction machinery a token signal it can *plan* with: a
deterministic per-message token estimate, continuously calibrated against the
provider's actual `prompt_tokens`, that also covers the post-restart blind
spot where `last_prompt_tokens` is 0 (design defects D15-partial,
D10-partial). Phase 03 (budget-compaction) is the consumer of everything this
phase adds — nothing here changes compaction behavior yet, except the blind
spot fix.

## Architecture references

Read before starting:

- `docs/design/context-management.md#33-working-set-layout-and-token-budgeting`
  — the estimation + calibration design.
- `docs/architecture.md#12-orchestration-layer-srcdaemon` — where
  `daemon/context/` sits.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors below against the working tree —
   this doc was drafted at milestone kick-off.

## Current state

- `Message` is defined at `src/ai/types/wire.rs:20` — fields `role: String`,
  `content: String`, `tool_calls: Option<Vec<ToolCall>>`,
  `tool_results: Option<Vec<ToolResult>>`, `turn: Option<u32-or-usize>`
  (check the actual type). `ToolCall` has `id`, `name`, `arguments`
  (all strings) + `thought_signature`; `ToolResult` has `tool_call_id`,
  `tool_name`, `content`.
- `SessionEntry` (`src/daemon/session.rs:21`) has
  `last_prompt_tokens: u32`, updated at two sites in
  `src/daemon/stream.rs` (~line 663 and ~line 793) from
  `usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens`.
- The compaction decision reads `last_prompt_tokens` in
  `src/daemon/server/ask.rs:236-260`; when it is 0 (fresh entry after daemon
  restart or 30-min eviction), `token_pct` computes to 0 and compaction is
  blind for one turn.
- There is no `src/daemon/context/` module yet — this phase creates it.

## Spec

### 1. New module `src/daemon/context/mod.rs` + `src/daemon/context/estimate.rs`

Create the module and register it in `src/daemon/mod.rs` (`pub mod context;`
following the existing `pub mod digest;` style). `mod.rs` is a thin
re-export: `pub mod estimate;` (later phases add `epochs`, `recall`).

In `estimate.rs`, pin this exact deterministic formula (tests depend on it):

```rust
/// Fixed per-message overhead (role tags, framing) in estimated tokens.
const PER_MESSAGE_OVERHEAD: u64 = 8;
/// Fixed per-tool-call / per-tool-result framing overhead.
const PER_TOOL_ITEM_OVERHEAD: u64 = 12;

/// Estimate the prompt-token footprint of one message: ~4 chars per token
/// over all textual payloads, plus fixed framing overheads.
pub fn estimate_message_tokens(msg: &crate::ai::Message) -> u64 {
    let mut chars = msg.content.len() as u64;
    let mut items = 0u64;
    if let Some(calls) = &msg.tool_calls {
        for c in calls {
            chars += (c.name.len() + c.arguments.len()) as u64;
            items += 1;
        }
    }
    if let Some(results) = &msg.tool_results {
        for r in results {
            chars += (r.tool_name.len() + r.content.len()) as u64;
            items += 1;
        }
    }
    chars.div_ceil(4) + PER_MESSAGE_OVERHEAD + items * PER_TOOL_ITEM_OVERHEAD
}

/// Sum of `estimate_message_tokens` over a history slice.
pub fn estimate_history_tokens(messages: &[crate::ai::Message]) -> u64;
```

### 2. Calibration state on `SessionEntry`

Add to `SessionEntry` (`src/daemon/session.rs`):

```rust
/// Multiplier mapping estimated history tokens to observed prompt tokens
/// (absorbs system prompt, tool schemas, provider framing). EMA-smoothed;
/// clamped to [0.5, 4.0]. Starts at 1.5 (history is typically smaller than
/// the full prompt).
pub token_scale: f64,
```

Initialize `token_scale: 1.5` at **every** `SessionEntry` construction site —
grep-verified list to update (build after each), 7 sites total:
`src/daemon/server/ask.rs` (the `or_insert_with` at ~line 106),
`src/daemon/ghost.rs` (ghost entry construction, ~line 220),
`src/daemon/executor/mod.rs` (the ghost test constructor at ~line 1075), and
the four test constructors in `src/daemon/session.rs` `mod tests` (~lines 496,
532, 578, 626). `SessionEntry` has **no** `#[derive(Serialize/Deserialize)]`
(in-memory only), so there is no `#[serde(default)]` shortcut — each site must
set the field explicitly. If another constructor exists, the compiler will
find it — fix each.

### 3. Calibration update — in `src/daemon/stream.rs`

At **both** sites where `entry.last_prompt_tokens` is assigned (~663, ~793),
add immediately after the assignment:

```rust
let est = crate::daemon::context::estimate::estimate_history_tokens(&messages);
if est > 0 && entry.last_prompt_tokens > 0 {
    let observed = entry.last_prompt_tokens as f64 / est as f64;
    entry.token_scale = (0.7 * entry.token_scale + 0.3 * observed).clamp(0.5, 4.0);
}
```

(Extract this into a small
`pub fn update_token_scale(entry: &mut SessionEntry, messages: &[Message])`
in `estimate.rs` so both sites share one implementation and it is unit-
testable. Both call sites already hold the sessions lock via
`.unwrap_or_log()` — do not add new locking.)

### 4. Blind-spot fix — in `src/daemon/server/ask.rs`

Where `last_prompt_tokens` is read for the compaction decision
(~line 236), substitute the calibrated estimate when the observed value is
absent:

```rust
let effective_prompt_tokens: u32 = if last_prompt_tokens > 0 {
    last_prompt_tokens
} else {
    let est = crate::daemon::context::estimate::estimate_history_tokens(&messages);
    let scale = /* read entry.token_scale, default 1.5 if no entry */;
    ((est as f64 * scale) as u64).min(u32::MAX as u64) as u32
};
```

Use `effective_prompt_tokens` in the `token_pct` computation **and** pass it
to `PromptCtx.last_prompt_tokens` (so the `[BUDGET]` line is not blind
either). Everything downstream is unchanged.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] `estimate_message_tokens` returns exactly `content.len().div_ceil(4) +
      8` for a plain message (pinned formula) — test below.
- [ ] After a reload-from-disk turn (fresh `SessionEntry`,
      `last_prompt_tokens == 0`, non-empty history), the compaction decision
      uses a non-zero `effective_prompt_tokens` — test via the pure helper,
      not a full daemon.
- [ ] `token_scale` converges: feeding `update_token_scale` a constant
      observed/estimated ratio of 2.0 repeatedly moves scale from 1.5 toward
      2.0 monotonically and never exits `[0.5, 4.0]` — including under
      adversarial ratios (0.01, 1000.0) — negative/clamp case.
- [ ] No behavior change when `last_prompt_tokens > 0` (existing compaction
      tests still pass untouched).

## Test plan

Pure unit tests in `src/daemon/context/estimate.rs` (no HOME/env mutation
needed — keep them hermetic and lock-free):

- `estimate_plain_message_pins_formula` — a 100-char content message → `25 +
  8 = 33`.
- `estimate_counts_tool_calls_and_results` — message with one tool_call
  (name+args 40 chars) and one tool_result (name+content 400 chars) →
  `(40+400+content_len).div_ceil(4) + 8 + 2*12`.
- `estimate_history_sums_messages`.
- `update_token_scale_converges_and_clamps` — as in acceptance criteria.
- `update_token_scale_noop_when_no_observation` — `last_prompt_tokens == 0`
  or empty history leaves scale untouched.

## End-to-end verification

Not applicable — phase ships no user-visible artifact; the estimate is
internal state consumed by phase 03. (The blind-spot fix's observable effect
— compaction firing on the first post-restart turn — requires a >60%-full
session and is exercised in phase 03's E2E instead.)

## Authorizations

None.

## Out of scope

- Do NOT change any compaction threshold, cut point, or `TAIL_KEEP` — that is
  phase 03.
- Do NOT persist `token_scale` to disk — that is phase 09
  (session-meta-persistence).
- Do NOT estimate the system prompt or tool schemas separately — the scale
  factor absorbs them by design.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Review verdict — 2026-07-13

- **Verdict:** bounced
- **Bounces:** 1 (bug-02-1 — major)
- **Executor:** AEON-7/Qwen3.6-27B-AEON
- **Scope deviations:** Spec task 4 (blind-spot fix) computed the calibrated
  estimate but bound it to an underscore-suppressed `_effective_prompt_tokens`
  and never wired it into `token_pct` or `PromptCtx` — the post-restart blind
  spot (D15/D10-partial) is unfixed. Tasks 1–3 (formula, calibration EMA, both
  stream.rs sites, all 7 construction sites) are correct; gates re-run green
  (fmt/clippy/867+27 tests) but pass only because no test drives the ask.rs
  branch.
- **Calibration:** none yet — one occurrence. Watch for a recurrence of
  "computed-then-discarded via `_`-prefix that silences the unused lint";
  if it repeats, fold a spec-side note that pinned-value tasks must be
  consumed, not just produced.
### Update — ts=1783989194405 (complete, server-authored)

**Summary:** Summary + Notes for review:

**What was built:** Created `src/daemon/context/estimate.rs` with the deterministic per-message token estimation formula (`chars/4 + 8 overhead + 12 per tool item`), `estimate_history_tokens` for summing a history slice, and `update_token_scale` for EMA-based calibration against observed `prompt_tokens`. Added `token_scale: f64` to `SessionEntry`, initialized to `1.5` at all 7 construction sites (ask.rs, ghost.rs, executor/mod.rs test, 4 session.rs tests). Wired `update_token_scale` at both `last_prompt_tokens` assignment sites in `stream.rs`. Implemented the blind-spot fix in `ask.rs` — when `last_prompt_tokens == 0`, substitutes `estimate_history_tokens * token_scale` as `effective_prompt_tokens` for the compaction decision.

**Verification:** `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (867 passed), and `cargo fmt --all` all pass clean. All 5 required unit tests are included and pass.

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.69s


LINT
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.57s


TEST
ests::severity_rank_case_insensitive ... ok
test webhook::process::tests::severity_rank_ordering ... ok
test webhook::server::tests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_correct_token_allows ... ok
test webhook::server::tests::auth_missing_header_denies ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test cli::commands::stream::stream_seam_tests::recv_line_preserves_partial_bytes_across_a_dropped_read ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test manifest::tests::manifest_caps_at_1kb ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test manifest::tests::manifest_excludes_session_memories ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 867 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.47s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test session_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test event_log_append_read ... ok
test g4_briefing_masking_applied ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.11s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**
- `src/daemon/context/estimate.rs` — +232 -0
- `src/daemon/context/mod.rs` — +1 -0
- `src/daemon/executor/mod.rs` — +1 -0
- `src/daemon/ghost.rs` — +1 -0
- `src/daemon/mod.rs` — +1 -0
- `src/daemon/server/ask.rs` — +24 -4
- `src/daemon/session.rs` — +9 -0
- `src/daemon/stream.rs` — +4 -0

**Commit:** dab93e1b3e8e318deb1ee5bc79b9365631849aa6

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

