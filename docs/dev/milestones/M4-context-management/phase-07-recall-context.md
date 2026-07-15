# Phase 07: `recall_context` — retrieve archived turns

**Milestone:** M4 — Context Management Overhaul
**Status:** done
**Depends on:** phase-04 (archive), phase-05 (epoch turn ranges)
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Turn compaction from amnesia into cache eviction (design defect D2): a new
silent AI tool `recall_context` lets the model retrieve archived turns from
the **current session** by substring query or turn range. Epoch summaries
carry turn ranges (phase 05), so the model can navigate summary → originals.
Elision placeholders and the working-set head finally get to name the tool.

## Architecture references

Read before starting:

- `docs/design/context-management.md#34-recall_context--eviction-becomes-a-cache-miss-not-amnesia`
- `docs/design/context-management.md#13-known-stale-doc-note` — grep scan,
  NOT FTS5; no new dependencies.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors; in particular locate the current
   `search_repository` variants/arms — this phase mirrors that tool's
   plumbing shape everywhere.

## Current state

- The add-a-tool surface (post-M2 file layout):
  - `src/ai/types/pending.rs` — `PendingCall` enum: one variant per tool,
    plus arms in `to_tool_call()`, `id()`, `tool_name()`, `summary()`,
    `should_emit_tool_feedback()` (see `PendingCall::ListPanes` at
    `pending.rs:166` and its arms at `:404`, `:487`, `:516`, `:586` for a
    minimal silent-tool example).
  - `src/ai/types/events.rs` — `AiEvent` enum: one variant per tool.
  - `src/ai/tools/defs.rs` — `pub static TOOLS: &[ToolDef]` (`defs.rs:7`);
    all three provider backends render from this slice
    (`render_gemini(TOOLS)` — no per-backend entry needed).
  - `src/ai/tools/dispatch.rs` — `dispatch_tool_event()` maps a parsed tool
    call to the `AiEvent` variant.
  - `src/daemon/stream.rs` — the streaming match turns `AiEvent::X` into
    `PendingCall::X`.
  - `src/daemon/executor/mod.rs` — `execute_tool_call()` dispatches
    `PendingCall::X` to a handler; knowledge-ish silent tools live in
    `src/daemon/executor/knowledge/` (`search_repository` is the closest
    analogue — find its arm and mirror the shape).
  - `assets/prompts/sre.toml` — the canonical system prompt documents every
    tool (`src/config` loads it via `include_str!`); a `builtin_sre_prompt_parses`
    test validates it.
- Phase 04 delivered `archive_file(id)` + append-only archive JSONL of
  `Message` records; phase 05 delivered epoch records with
  `turn_start`/`turn_end` and the head line
  `"Older turns are preserved in the session archive."`.
- Masking: `mask_sensitive` (`src/ai/filter.rs`) is applied to outbound
  sensitive text — `handle_ask` masks the user query
  (`src/daemon/server/ask.rs:379`).
- Output caps: `config.limits.tool_result_chars`
  (`src/config/types.rs`, `default_tool_result_chars`) is the existing
  tool-output cap convention.
- `PendingCall` variants for silent tools return `true` from
  `should_emit_tool_feedback()` so the executor emits
  `ToolStarted`/`ToolFinished`.

## Spec

### 1. Recall engine — `src/daemon/context/recall.rs`

```rust
pub struct RecallArgs {
    pub query: Option<String>,
    pub turn_start: Option<u32>,
    pub turn_end: Option<u32>,
}

/// Search / slice the session archive. Streams the archive line-by-line
/// (BufReader — the archive can be huge; never read_to_string).
///
/// Modes:
/// - query only: case-insensitive substring over `content` and
///   `tool_results[].content`; returns up to MAX_MATCHES (8) match blocks,
///   each "turn {n} ({role}): …±200-char excerpt around the match…".
///   Multiple matches within one message collapse to one block.
/// - turn range only: the messages whose `turn` falls in
///   [turn_start, turn_end] verbatim (role-prefixed), oldest first.
/// - query + range: substring search restricted to the range.
/// - neither: Err("recall_context requires a query and/or a turn range").
///
/// Output is passed through mask_sensitive and truncated at a char
/// boundary to `limits.tool_result_chars` with a
/// "[…truncated — narrow the turn range or refine the query…]" suffix.
/// Messages with `turn: None` (legacy) are searchable by query but
/// unreachable by range; a range query notes
/// "(legacy messages without turn numbers were skipped)" when any were.
pub fn recall(session_id: &str, args: &RecallArgs,
              limits: &LimitsConfig) -> Result<String, String>;
```

Excerpt semantics, pinned: ±200 **chars** around the first match,
char-boundary safe (same UTF-8 discipline as phase 03's truncation), with
`…` at clipped ends.

### 2. Tool plumbing — follow the checklist, mirroring `search_repository`

1. `pending.rs`: `PendingCall::RecallContext { id: String, query:
   Option<String>, turn_start: Option<u32>, turn_end: Option<u32>,
   thought_signature: Option<String> }` + arms in `to_tool_call()`
   (arguments as JSON), `id()`, `tool_name()` (`"recall_context"`),
   `summary()` (e.g. `query="disk pressure" turns=120..180`),
   `should_emit_tool_feedback()` → **true** (silent tool).
2. `events.rs`: `AiEvent::RecallContext { … }` matching the variant.
3. `defs.rs`: `ToolDef` entry — name `recall_context`, description:

   ```
   Retrieve archived conversation turns from the current session that were
   compacted out of the live context. Search by substring query, by turn
   range (epoch summaries in [Session Context] give turn ranges), or both.
   Use when you need details an epoch summary or an "[elided: …]"
   placeholder refers to.
   ```

   Params: `query` (string, optional), `turn_start` (integer, optional),
   `turn_end` (integer, optional). Follow the optional-param convention of
   the existing `ToolDef`s in `defs.rs` (copy an entry that has optional
   integer params). Decide core vs deferred placement by mirroring
   `search_repository`'s placement in the core/deferred split — same tier.
4. `dispatch.rs`: `dispatch_tool_event()` arm parsing the three args.
5. `stream.rs`: `AiEvent::RecallContext` → `PendingCall::RecallContext` in
   the streaming match (both places the match appears, if the ghost loop has
   its own — grep `AiEvent::SearchRepository` and mirror every arm).
6. `executor/mod.rs` (or `executor/knowledge/` if `search_repository` lives
   there): arm calling `recall(session_id, …)` and returning
   `ToolCallOutcome::Result`. The executor has the session id in scope at
   the `search_repository` arm — use the same source. When `session_id` is
   unavailable, return `Err`-shaped result text
   (`"Error: recall_context requires a session"`), never panic.
7. `assets/prompts/sre.toml`: document the tool in the tools section (match
   the existing per-tool doc style; the `builtin_sre_prompt_parses` test
   gates syntax).

### 3. Wording updates now that the tool exists

- Phase 04's elision placeholder gains the tool name:
  `[…; archived — retrieve with recall_context (turn {n}).]`
  (`src/daemon/digest.rs`).
- Phase 05's head line becomes: `Older turns are preserved in the session
  archive — retrieve originals with recall_context(query, turn_start,
  turn_end).` (`src/daemon/context/epochs.rs` head template).
- Ghost tool policies: `recall_context` is read-only and safe — add it
  wherever `search_repository` appears in default allow-lists
  (`grep -rn "search_repository" src/agents/ src/daemon/policy.rs` and
  mirror; if allow-lists are user-authored only, no change — verify and note
  in the completion log).

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean;
      `builtin_sre_prompt_parses` still green.
- [ ] Query mode returns an excerpt containing the match with `turn {n}`
      attribution; **negative:** a query matching only elided placeholder
      text in the *working set* but present in full in the archive returns
      the archived full text (that is the whole point — test seeds an
      archive whose content the working file lacks).
- [ ] Range mode returns verbatim messages within the range and skips
      legacy `turn: None` messages with the notice.
- [ ] No-args call returns the usage error string.
- [ ] Output is masked: a seeded archive line containing a fake AWS key
      (use the same fixture pattern as existing `filter.rs` tests) comes
      back masked.
- [ ] Output truncates at `tool_result_chars` with the truncation suffix,
      UTF-8-safely.
- [ ] `should_emit_tool_feedback()` returns true for the variant (the CLI
      shows `ToolStarted`/`ToolFinished` — covered by the enum unit test
      pattern in `pending.rs` if one exists; else assert directly).
- [ ] All three provider renders include the tool (existing
      TOOLS-slice render tests in `schema.rs`/`dispatch.rs` cover count —
      update any pinned tool-count assertions).

## Test plan

- `recall_query_finds_archived_content` in `recall.rs` — the
  working-set-lacks-it/archive-has-it scenario above.
- `recall_range_returns_verbatim_and_skips_legacy`.
- `recall_requires_query_or_range` — error case.
- `recall_masks_sensitive_output`.
- `recall_truncates_at_cap_utf8_safe` — multi-byte content at the cap
  boundary.
- `recall_excerpt_is_bounded` — a 50k-char matched message yields ≤ ~400
  chars + markers for its block.
- `pending_recall_context_arms` in `pending.rs` — `tool_name()`,
  `summary()`, `should_emit_tool_feedback() == true`.
- Dispatch parse test in `dispatch.rs` mirroring the existing per-tool parse
  tests (grep `search_repository` there for the pattern).

## End-to-end verification

1. Seed a temp-HOME archive with a distinctive string in an early turn, run
   the recall engine through the executor arm (integration-test style), and
   quote the returned block in the completion log.
2. Real-model E2E (tool visible to the LLM) requires a configured backend;
   if unavailable in the executor environment, verify instead that
   `render_anthropic`/`render_gemini` output (dump via the existing schema
   tests) contains the `recall_context` definition, and quote it.

## Authorizations

None. (No new dependencies — grep scan, not FTS5.)

## Out of scope

- FTS5 / rusqlite indexing (design doc §8).
- Cross-session recall — `recall_context` reads only the current session's
  archive.
- Regex queries — substring only in this phase.
- Approval gating — the tool is silent/read-only by design.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-07-14

The prior run hard-failed on a **no-progress stall**: 20 tool calls, all
read-only, **zero edits** — it read the entire §2 plumbing surface
(`pending.rs`, `events.rs`, `defs.rs`, `args.rs`, `stream.rs`,
`executor/mod.rs`, `filter.rs`, `session.rs`, some repeatedly) before writing
a single line. Refinement for this attempt:

**Write §1 (`src/daemon/context/recall.rs`) FIRST, before reading the §2
plumbing surface.** The recall engine is self-contained — it depends only on
`crate::daemon::session::archive_file(id)` (a `BufReader` line stream over the
archive JSONL), `crate::ai::filter::mask_sensitive`, and
`crate::config::LimitsConfig` (for `tool_result_chars`). You can implement and
unit-test it (the `recall_*` tests in the Test plan) as a standalone module
**without touching any of the tool-plumbing files.** Get `recall.rs` compiling
green with its tests, *then* do §2 by mirroring `search_repository` one file at
a time (build after each site). This front-loads a real edit and keeps each step
small instead of holding the whole 7-file surface in context at once.

(Calibration context: the governor's no-progress threshold was also raised
20 → 40 for this project, so you have more room — but do not rely on it; make an
early edit.)

### Update — 2026-07-14 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** First failure, thorough spec, and two concrete refinements
address the observed no-progress stall (governor threshold raised 20→40 for
this project; a Notes-for-executor block instructing an early self-contained
recall.rs write) — takeover would skip the model data point and the anti-pattern
warns against it on a first failure.

### Update — 2026-07-14 (escalation)

**Chosen lever:** session takeover
**Rationale:** Second `NoProgressStall` (now at 40 turns, not 20) after the one
refinement — the same failure class recurred, and the executor left a
near-complete implementation (recall.rs + full plumbing) broken by a
self-inflicted unclosed-delimiter in args.rs it couldn't hunt down. Resume would
re-encounter the same thrash; the work is on disk and the blocker is a specific
findable syntax error, so takeover is efficient.

### Update — 2026-07-14 (complete, architect takeover)

**Summary:** The executor's 2nd run left a near-complete implementation: §1
`context/recall.rs` (engine + tests) and §2 plumbing (pending/events/args/defs/
dispatch/stream/ghost/executor — both the stream and ghost `AiEvent` matches
wired per §2.5) all landed and compiled. The takeover finished it:
- Added the missing `PendingCall::RecallContext` arm to
  `should_emit_tool_feedback()` (it fell through to `false`; the silent-tool
  test caught it).
- Completed the §3 wording updates the executor stalled before doing: the
  `digest.rs` elision placeholder and the `epochs.rs` head line now name
  `recall_context`, and `sre.toml` documents the tool (§2.7).
- Fixed a byte/char index bug in `build_excerpt` (it found the match as a byte
  offset then indexed a `Vec<char>` with it — a multibyte-unsafe window; the
  spec pins char-safety) and added `build_excerpt_is_multibyte_safe`.
- Gave the recall FS tests proper `TEST_HOME_LOCK` + temp-HOME isolation via an
  RAII guard; they had no isolation and raced (failing in parallel) / wrote into
  the real `~/.daemoneye`.
- §3.3 ghost policies: no-op — `search_repository` appears only in policy
  *tests*, not a hardcoded default allow-list, so `recall_context` (equally
  read-only) needs no allow-list edit, exactly as the spec's conditional says.

**Acceptance criteria:** all met, each with a passing test — query mode with
turn attribution + archive-over-working-set (`recall_query_finds_archived_content`),
range verbatim + legacy skip notice (`recall_range_returns_verbatim_and_skips_legacy`),
no-args usage error (`recall_requires_query_or_range`), masking
(`recall_masks_sensitive_output`), UTF-8-safe truncation
(`recall_truncates_at_cap_utf8_safe`), bounded excerpt (`recall_excerpt_is_bounded`)
+ multibyte-safe (`build_excerpt_is_multibyte_safe`), silent-tool feedback flag
(`should_emit_tool_feedback_silent_tools_true`), and three-provider render
(`recall_context` in `TOOLS`; the render/count tests use `TOOLS.len()`).

**Commands:**

```
cargo fmt --all --check    → clean
cargo build                → Finished, 0 warnings
cargo clippy --all-targets --all-features -- -D warnings → clean
cargo test                 → 893 passed; 0 failed (unit) + 27 passed (integration)
```

**End-to-end verification:** The recall engine is exercised over a **real
archive file** by the hermetic FS tests (real tempdir `HOME`, real
`archive_file` write + `recall` read): `recall_query_finds_archived_content`
seeds a distinctive early-turn string into the archive and asserts recall returns
it. Tool visibility to the model is verified by `recall_context` being present in
`TOOLS` (defs.rs) — the render tests render `TOOLS` for all three providers. The
phase doc's E2E allows this render-check substitution when no live model is
configured.

**Files changed:**
- `src/daemon/context/recall.rs` — new engine (executor) + build_excerpt fix + test isolation (architect)
- `src/ai/types/pending.rs` — RecallContext variant + arms (executor) + should_emit arm (architect)
- `src/ai/types/events.rs`, `src/ai/tools/{args,defs,dispatch}.rs`, `src/daemon/{stream,ghost,executor/mod,context/mod}.rs` — plumbing (executor)
- `src/daemon/digest.rs`, `src/daemon/context/epochs.rs`, `assets/prompts/sre.toml` — §3 wording + tool doc (architect)

### Review verdict — 2026-07-14

- **Verdict:** escalated
- **Bounces:** 2 no-progress stalls (1st: 20 turns, zero edits; 2nd: 40 turns after a near-complete impl) → refined re-dispatch (governor threshold 20→40 + write-first guidance) → 2nd-failure takeover
- **Executor:** AEON-7/Qwen3.6-27B-AEON (engine + full plumbing, correct) → Claude (direct) completed §3 wording + fixed should_emit arm, build_excerpt byte/char bug, and recall-test HOME isolation
- **Scope deviations:** executor's `recall()` uses `LimitsConfig::default()` (config isn't threaded into `execute_tool_call`; the default cap applies) — accepted, not worth a wide-blast-radius signature change.
- **Calibration:** The new `NoProgressStall` governor **validated in the wild** — caught both stalls (at 20, then 40) instead of 167-529-turn runaways. But default 20 was too tight for a `size=l` ~7-file phase → raised to 40 per-project. The write-first refinement worked (2nd run wrote a near-complete impl). The executor's recurring test-isolation bug (HOME leak, seen phases 06 + 07) is a candidate STANDARDS fold if it appears a 3rd time.
