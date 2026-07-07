# Phase 03: Token-budget compaction with hysteresis

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-02 (token estimation)
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Replace the fixed 16-message compaction tail with a **token-budgeted** cut:
crossing `compact_at_pct` (60%) compacts the working set down to
`target_pct` (40%) of the context window, so each compaction frees ≥ 20% of
the window and cannot re-fire every turn (design defects D8, D12). Also:
synthesize a boundary when no clean one exists instead of skipping compaction
(D9), make the thresholds configurable via a new `[compaction]` section, and
stop the `[BUDGET]` line telling interactive sessions to wrap up on token
pressure (D14).

## Architecture references

Read before starting:

- `docs/design/context-management.md#33-working-set-layout-and-token-budgeting`
  — the budgeting + hysteresis + synthesized-boundary design.
- `docs/design/context-management.md#2-failure-catalog` — D8, D9, D12, D14.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors — phases 01/02 have landed since
   this doc was drafted.

## Current state

**The load-bearing constraint (front-loaded):** every result of a compaction
or trim MUST preserve tool_call ↔ tool_result pairing — a `tool_results`
message whose producing `tool_calls` assistant message was dropped is
rejected by all three provider backends. Today this is guaranteed by
`next_clean_turn_start` (`src/daemon/session.rs:202-212`) and, when it fails,
by **skipping compaction entirely** (`src/daemon/digest.rs:626-631`). This
phase keeps the invariant but replaces "skip" with "repair" — see Spec §4.

Key existing code:

- `src/daemon/digest.rs:28-41` — the constants this phase replaces/augments:

  ```rust
  pub const DIGEST_THRESHOLD: usize = 20;
  const TAIL_KEEP: usize = 16;
  const ELIDE_THRESHOLD_CHARS: usize = 3000;
  const ELISION_TAIL_KEEP: usize = 8;
  ```

- `src/daemon/digest.rs:612-618` — `planned_tail_start`:

  ```rust
  pub fn planned_tail_start(messages: &[Message]) -> Option<usize> {
      if messages.len() <= TAIL_KEEP + 2 {
          return None;
      }
      let raw_tail_start = messages.len().saturating_sub(TAIL_KEEP);
      crate::daemon::session::next_clean_turn_start(messages, raw_tail_start)
  }
  ```

- `src/daemon/server/ask.rs:251-267` — the decision block:

  ```rust
  const ELISION_PCT: u32 = 50;
  const DIGEST_PCT: u32 = 60;
  let context_window = config.resolve_model(session_active_model.as_deref()).context_window();
  let token_pct = ...;
  let at_safety_cap = history_cap.is_some_and(|cap| messages.len() >= cap);
  let above_floor = messages.len() >= DIGEST_THRESHOLD;
  let should_digest = above_floor && (token_pct >= DIGEST_PCT || at_safety_cap);
  let should_elide_only = !should_digest && above_floor && token_pct >= ELISION_PCT;
  ```

- `src/daemon/prompt.rs:193-200` — the `[BUDGET]` warning that says
  "NEAR LIMIT. Summarize progress, persist critical state to memory, and
  wrap up." driven by `max_pct = turn_pct.max(history_pct).max(token_pct)`.
- Phase 02 delivered `crate::daemon::context::estimate::
  {estimate_message_tokens, estimate_history_tokens}` and
  `SessionEntry.token_scale`.
- Config: the section name `[context]` is **taken** (`ContextConfig` with an
  `environment` field, used at `src/daemon/prompt.rs:110`) — the new section
  is `[compaction]`.

## Spec

### 1. `CompactionConfig` — in `src/config/types.rs`

Follow the `DigestConfig` declaration pattern (`src/config/types.rs:96-104`)
exactly:

```rust
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CompactionConfig {
    /// Elide oversized old tool_results at this % of the context window.
    #[serde(default = "default_elide_at_pct")]
    pub elide_at_pct: u32,      // default 50
    /// Build a digest and cut the working set at this %.
    #[serde(default = "default_compact_at_pct")]
    pub compact_at_pct: u32,    // default 60
    /// Post-compaction working-set target as % of the context window.
    #[serde(default = "default_target_pct")]
    pub target_pct: u32,        // default 40
    /// Synchronous emergency compaction threshold (consumed in phase 08;
    /// parsed now so configs written today keep working).
    #[serde(default = "default_emergency_pct")]
    pub emergency_pct: u32,     // default 85
}
```

Wire `#[serde(default)] pub compaction: CompactionConfig` into `Config` +
`Default` impl. Validate in the existing config-validate path: warn (not
error) if `target_pct >= compact_at_pct` and fall back to defaults for the
pair.

### 2. Budget-based cut — in `src/daemon/digest.rs`

Add (keeping `planned_tail_start` temporarily as a thin wrapper —
see migration note below):

```rust
/// Plan the compaction cut so the *kept* tail fits within
/// `budget_tokens` estimated tokens (post-scale). Walks backward from the
/// end accumulating `estimate_message_tokens`, stops before exceeding the
/// budget, then advances to the next clean turn boundary.
/// Guarantees: keeps at least MIN_TAIL_MESSAGES (4) and drops at least one
/// full turn; returns None when the history is too short to do both.
pub fn planned_tail_start_by_budget(
    messages: &[Message],
    budget_tokens: u64,
    token_scale: f64,
) -> Option<usize>
```

Semantics, pinned:

- Walk `i` from `messages.len()-1` down; accumulate
  `(estimate_message_tokens(m) as f64 * token_scale) as u64`; stop at the
  last `i` where the running sum ≤ `budget_tokens`, but never let
  `i > messages.len() - MIN_TAIL_MESSAGES` (tail floor) and never let
  `i < 2` (must leave room for the two head slots).
- Then `next_clean_turn_start(messages, i)`; if that lands at/after
  `messages.len() - 1` (nothing meaningful dropped… tail floor breached),
  fall through to the synthesized boundary (§4).
- `budget_tokens` computed by the caller as
  `context_window * target_pct / 100`, saturating.

Migration note: `compact_with_digest` keeps its signature but takes the
tail-start as a parameter now —
`compact_with_digest(messages, digest, tail_start)` — so the planner and the
compactor cannot disagree. Update the two call sites (`ask.rs` compaction
block; the narrative-slice computation at `ask.rs:285` uses the same
`tail_start`). Update `digest.rs` unit tests accordingly — the behavioral
assertions (first message preserved, digest at index 1, tail starts on a
clean user turn, no orphan tool_result) all stay; only the "result length ==
2 + TAIL_KEEP" style count assertions change to budget-based expectations.

### 3. Rewire the decision block — `src/daemon/server/ask.rs`

Replace the `ELISION_PCT`/`DIGEST_PCT` consts with
`config.compaction.elide_at_pct` / `.compact_at_pct`. Use
`effective_prompt_tokens` (phase 02) for `token_pct`. In the
`should_digest` arm, compute:

```rust
let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;
let tail_start = planned_tail_start_by_budget(&messages, budget, token_scale)
    .or_else(|| synthesized_tail_start(&messages, budget, token_scale));
```

`DIGEST_THRESHOLD` (message floor) and the `max_history` safety cap keep
their current roles unchanged.

### 4. Synthesized boundary — in `src/daemon/digest.rs`

New fallback used only when no clean boundary exists in the budget region:

```rust
/// Last-resort cut when no clean turn boundary exists: cut at the raw
/// budget index, then REPAIR the tail head instead of giving up —
/// strip `tool_results` from any leading messages whose producing
/// tool_call was dropped, replacing each stripped message's content with
/// "[tool results from a compacted turn were elided]".
fn synthesized_tail_start(...) -> Option<usize>
```

Pinned repair semantics (`repair_tail_head(&mut Vec<Message>)`, pure,
unit-tested):

- While the first tail message is `role == "user"` with non-empty
  `tool_results`: set `tool_results = None`; if `content` is empty, set the
  placeholder content above.
- A leading `assistant` message is acceptable **only if** it carries no
  `tool_calls` awaiting results inside the dropped region; if the first tail
  message is an assistant message with `tool_calls`, keep it — its results
  follow *inside* the tail and pairing is intact. (Negative test pins this:
  an assistant+tool_calls head whose results are in the tail must NOT be
  stripped.)
- The existing warn-and-skip branch
  (`compact_with_digest: no clean turn boundary found — skipping`) is
  deleted; log at INFO that a boundary was synthesized instead.

### 5. Graduated elision — `elide_old_tool_results`

Add an `aggressive: bool` parameter:

- `aggressive == false` (the ≥ `elide_at_pct` path): oversized results
  (> `ELIDE_THRESHOLD_CHARS`) are **truncated head+tail** — keep the first
  1000 and last 500 chars around a `"[… {n} chars truncated …]"` marker —
  instead of fully replaced.
- `aggressive == true` (the ≥ `compact_at_pct` path, called before digesting,
  and the future emergency path): current behavior — full placeholder.
- The placeholder text keeps its current wording in this phase (phase 04
  makes it honest); byte-savings return value semantics unchanged.
- Truncation must be **char-boundary safe**: slicing a multi-byte UTF-8
  string at byte 1000 panics. Use `char_indices()` (or
  `s.floor_char_boundary`-equivalent manual scan) — pin a test with
  multi-byte content (e.g. a string of `é`).

### 6. `[BUDGET]` rewording — `src/daemon/prompt.rs:193-200`

Token pressure no longer produces "wrap up" advice for interactive sessions:

- Compute the warning from `turn_pct` only (ghost turn budget) plus, when
  `token_pct >= 50`, append the *informational* clause
  `" — context compaction will run automatically"` instead of behavioral
  instructions.
- Ghost sessions keep the existing wrap-up wording driven by `turn_pct`
  (their turn budget is real). `history_pct` drops out of the warning
  entirely (the safety cap is the compactor's business).
- Update the prompt-format unit tests in `prompt.rs` if any pin the old
  wording (grep for `"NEAR LIMIT"`).

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] Hysteresis: with `context_window = 10_000`, `token_scale = 1.0`, and a
      history of uniform ~250-token messages at 65% pressure, one compaction
      pass yields an estimated working set ≤ 40% + one message of slack
      (test below).
- [ ] Anti-thrash: immediately re-running the decision logic on the
      compacted result with the same window does NOT trigger another digest
      (negative case).
- [ ] Orphan-safety: for **every** compaction path (clean boundary,
      synthesized boundary, repair), the result contains no `tool_results`
      whose `tool_call_id` lacks a preceding `tool_calls` entry — reuse the
      exhaustive checker loop from the existing
      `compact_skips_orphan_tool_result_at_boundary` test
      (`src/daemon/digest.rs:779-841`) as a shared test helper.
- [ ] The pathological all-tool-result history that today returns unchanged
      (`compact_skipped_when_no_clean_boundary` test) now **compacts** via
      the synthesized boundary and passes the orphan checker (this existing
      test is rewritten, not deleted).
- [ ] `[compaction]` TOML round-trips; a config file with only
      `compact_at_pct = 70` parses with the other fields defaulted;
      `target_pct >= compact_at_pct` warns and falls back.
- [ ] No `"NEAR LIMIT"` wrap-up text is emitted for a non-ghost session at
      high token pressure (negative grep in prompt tests).

## Test plan

- `budget_cut_respects_target` in `digest.rs` — as in acceptance criteria.
- `budget_cut_keeps_min_tail` — a history of two enormous messages still
  keeps `MIN_TAIL_MESSAGES`.
- `no_rethrash_after_compaction` — decision fn on compacted output → no
  digest.
- `synthesized_boundary_repairs_orphans` — the all-tool-result pathological
  history compacts; orphan checker passes; leading stripped message carries
  the placeholder content.
- `synthesized_boundary_keeps_paired_assistant_head` — **negative case**: an
  assistant+tool_calls tail head whose results follow inside the tail is NOT
  stripped.
- `elide_soft_truncates_head_tail` — non-aggressive elision keeps first
  1000 + last 500 chars with the marker; `elide_aggressive_full_placeholder`
  — aggressive keeps current behavior.
- `elide_truncation_is_utf8_safe` — multi-byte content does not panic and
  produces valid UTF-8.
- `compaction_config_defaults_and_validation` in `config/mod.rs` tests —
  round-trip + fallback-on-invalid.

## End-to-end verification

Config-file artifact check (the running binary loads `[compaction]`):

1. Write `compact_at_pct = 70` under `[compaction]` in a temp-HOME
   `config.toml`, start the daemon, run `daemoneye status` (or grep
   `daemon.log` for the config-load line) proving the value was parsed —
   quote the output.
2. Not applicable beyond that — compaction itself requires a >60%-full live
   session; the behavior is pinned by the unit suite above. State this in
   the completion log.

## Authorizations

None.

## Out of scope

- Do NOT touch the digest *content* (tally/narrative) — phases 05/06.
- Do NOT implement the emergency path — `emergency_pct` is parsed but unused
  until phase 08 (this is the authorized exception to
  "wired-in state needs a consumer": the config key is documented as
  phase-08-consumed in its doc comment).
- Do NOT move compaction off the request path — phase 08.
- Do NOT change session-file persistence — phase 04.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
