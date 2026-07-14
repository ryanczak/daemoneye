# Phase 02: Per-message token estimation with per-session calibration

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
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
