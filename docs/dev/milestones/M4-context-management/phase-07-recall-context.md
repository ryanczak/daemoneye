# Phase 07: `recall_context` — retrieve archived turns

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
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
