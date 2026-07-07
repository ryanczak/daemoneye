# Phase 10: Ghost-session compaction coverage and opt-in memory extraction

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-03 (planner/elision), phase-05 (epochs), phase-06 (summarize_once)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Close the two remaining coverage gaps: ghost sessions get elision +
structured-only epoch compaction inside their turn loop (design defect D13 —
today a tool-heavy ghost can exceed a 32k local-model window with no relief),
and — behind an off-by-default flag — epoch creation can distill durable
facts into the persistent memory system, so multi-month sessions accumulate
knowledge the prompt tiers already know how to surface.

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
5. Re-verify the **Current state** anchors — every prior M4 phase has
   landed; the ghost loop and epoch entry points may have drifted from the
   line numbers below.

## Current state

- The ghost turn loop is `trigger_ghost_turn` in `src/daemon/ghost.rs`. Per
  iteration it clones the session's history and sends it straight to the
  backend with **no compaction of any kind**:

  ```rust
  // ghost.rs ~482-500 (drafted-time)
  let (chat_messages, loaded_tools) = { /* lock, clone entry.messages */ };
  ...
  .chat(&system_clone, chat_messages, ai_tx, true, loaded_tools)
  ```

  Assistant/tool messages are pushed back onto `entry.messages` under the
  lock (~`:466`, `:969`).
- Ghost sessions have a `SessionEntry` in the same store (`is_ghost:
  true`), so phase 02's `token_scale`, phase 03's planner, and phase 05's
  `compact_with_epochs` all apply mechanically. `last_prompt_tokens` IS
  updated for ghosts wherever their usage events flow through the shared
  write-back — re-verify; if the ghost loop bypasses it, rely on the
  phase-02 estimate path exclusively (it needs no observed tokens).
- Ghost model resolution: the runbook may pin a model
  (`GhostConfig`/`active_model`); `config.resolve_model(...)
  .context_window()` gives the window — a ghost on a local 32k model is the
  motivating case.
- Phase 08 deliberately guards `spawn_compaction` with `!is_ghost` — ghost
  compaction is **synchronous inside the loop** (a ghost turn has no human
  waiting; the structured-only path makes it cheap with no model call).
- Memory writes: the AI-tool handler in
  `src/daemon/executor/knowledge/memory.rs` is the canonical
  `add_memory` call site — mirror its arguments (name, content, category,
  namespace, and the G2 schema fields including `source`) exactly. The
  memory layer enforces size caps, masking, and locking internally.
- `summarize_once(system, user_text, model_entry)` exists in
  `src/daemon/digest.rs` (phase 06).
- Config: `CompactionConfig` (phase 03/06) is where `extract_memories`
  lands.

## Spec

### 1. Ghost working-set guard — in `src/daemon/ghost.rs`

Extract a helper in `src/daemon/context/mod.rs` so ghost and any future
caller share one implementation:

```rust
/// Synchronous, model-call-free working-set control for autonomous
/// sessions. Applies (by escalating pressure):
///   >= elide_at_pct   → aggressive elision
///   >= compact_at_pct → structured-only epoch (narrative=None) + rollup
///                       (structured-fallback narrative) + compact
/// Returns the possibly-compacted messages plus a bool for "compacted"
/// (caller rewrites the session file when true — same needs_compaction
/// contract as the interactive path).
pub fn enforce_ghost_working_set(
    session_id: &str,
    messages: Vec<Message>,
    entry_scale: f64,
    started_at: DateTime<Utc>,
    context_window: u32,
    config: &Config,
) -> (Vec<Message>, bool)
```

Pressure uses the phase-02 estimate exclusively
(`estimate_history_tokens * scale` vs `context_window`) — do not depend on
`last_prompt_tokens` for ghosts.

Call it in `trigger_ghost_turn` each iteration, on the cloned history
**before** `.chat(…)`, writing back the compacted vec + session file when
compaction occurred. All lock discipline as elsewhere
(`.unwrap_or_log()`, no `.await` under a guard — the helper is fully
synchronous by design, so this is easy to keep true).

Ghost epochs are ordinary epoch records (same file, same `seq` space); the
`[Session Context]` head works identically for the ghost's next iteration.

### 2. Memory extraction flag — `CompactionConfig`

```rust
/// When true, each epoch build asks the digest model to propose 0–3
/// durable facts, written to persistent memory (category "knowledge",
/// source "compaction"). Off by default — it adds one small-model call
/// per epoch and writes to shared memory.
#[serde(default)]
pub extract_memories: bool,
```

### 3. Fact extraction — `src/daemon/context/epochs.rs`

```rust
/// Best-effort: never blocks or fails the epoch build. Returns the number
/// of memories written (for the log).
pub async fn extract_memories_from_epoch(
    session_id: &str,
    record: &EpochRecord,
    dropped: &[Message],
    config: &Config,
) -> u32
```

- Prompt via `summarize_once`, system prompt pinned verbatim:

  ```
  You extract durable operational knowledge from an SRE assistant's
  conversation. From the transcript chunk, output 0 to 3 facts that will
  still be true and useful weeks from now (host quirks, root causes,
  fixed configurations, learned procedures). Output STRICT JSON only:
  [{"name":"kebab-case-slug","content":"one to three sentences"}] or [].
  No prose. Do NOT record transient state (current disk %, running PIDs),
  speculation, or anything already obvious from the runbooks.
  ```

  User text: the phase-05 keep-newest narrative input formatter's output
  for `dropped` (reuse `format_messages_for_narrative`).
- Parse strictly with `serde_json`; on parse failure or > 3 entries, log
  DEBUG and write nothing (never retry).
- Validate each `name` with the same slug rules the memory layer enforces
  (mirror the `add_memory` handler's validation); **skip** any name that
  already exists in the namespace (no overwrites — dedupe by existence
  check via the memory read API).
- Write via the exact `add_memory` path the tool handler uses
  (`executor/knowledge/memory.rs` — mirror category `"knowledge"`,
  namespace = the session's memory namespace (ghosts: their agent
  namespace; interactive: global — reuse
  `build_memory_namespaces`' first entry), `source: "compaction"`).
- Call sites: interactive epoch build (phase 05/08 path — in the async task
  it runs after `append_epoch`, still off the hot path) and NOT the ghost
  structured-only path (ghosts must stay model-call-free here — their
  runbook policy may forbid model side-effects; note this in a comment).
- Log `log_event("memory_extracted", {"session", "epoch_seq", "count"})`
  when count > 0.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] A ghost history driven over `compact_at_pct` (small
      `context_window_tokens` override in the test config) compacts inside
      `enforce_ghost_working_set`: result estimate ≤ target, epoch record
      appended, orphan checker green, and **no network/model call occurred**
      (structured-only — test runs with no backend configured).
- [ ] Below `elide_at_pct` the helper is a strict no-op (**negative case**:
      returns the input vec unchanged, no epoch appended).
- [ ] `extract_memories = false` (default): no memory writes, no summarizer
      call during epoch builds (**negative case**).
- [ ] With the flag on and a mocked/failed summarizer: epoch build still
      succeeds, zero memories written (best-effort pinned).
- [ ] Valid extraction JSON → memory files exist with
      `source: "compaction"` frontmatter and pass the memory layer's own
      validation; a duplicate name is skipped, not overwritten (**negative
      case**).
- [ ] Malformed JSON / 4+ facts → nothing written.

## Test plan

FS tests take `TEST_HOME_LOCK` + temp HOME.

- `ghost_guard_noop_below_threshold` in `context/mod.rs` tests.
- `ghost_guard_compacts_structured_only` — tiny window override; assert
  compacted + epoch appended + `narrative.is_none()` on the record.
- `ghost_guard_output_orphan_free` — reuse the shared checker.
- `extract_parses_strict_json_and_writes` — feed the parser/writer a canned
  summarizer output (factor the parse+write out of the async fn as
  `fn apply_extraction(json: &str, …) -> u32` so no model is needed).
- `extract_rejects_malformed_and_excess`.
- `extract_skips_existing_name`.
- `extract_flag_off_writes_nothing`.

## End-to-end verification

1. Ghost path: with a temp-HOME config pinning
   `context_window_tokens = 2000` on a dummy model entry and
   `narrative_enabled = false`, drive `enforce_ghost_working_set` with a
   fixture history through the integration harness; quote the appended
   epoch JSON and the before/after message counts.
2. Memory path: run `apply_extraction` with a valid two-fact JSON fixture;
   quote `ls` of the memory directory and one file's frontmatter showing
   `source: "compaction"`.

## Authorizations

- [ ] May touch `docs/architecture.md` §3 (Ghost Shell subsystem): one
      sentence — ghost turn loops enforce the working set synchronously
      (structured-only), per the M4 design.

## Out of scope

- No narrative model calls anywhere in the ghost path.
- No extraction from ghost epochs (interactive epochs only, this phase).
- No memory *updates* or confidence re-scoring — create-if-absent only.
- No changes to ghost turn budgets, policies, or mailbox behavior.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
