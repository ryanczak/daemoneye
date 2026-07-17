# Phase 10b: Opt-in compaction → memory extraction

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-06 (`summarize_once`), phase-08 (async epoch build), phase-10a
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=feature, size=m

> **NOT YET RE-VERIFIED FOR DISPATCH.** This is 10b of the 10a/10b split of the
> original phase-10. Its Spec carries the memory-extraction half of that draft.
> Before dispatching, the architect (`/rexymcp:architect next` once 10a is
> `done`) MUST re-verify the Current-state anchors and fix the known issues
> flagged below. Do not dispatch this doc as-is.

## Goal

Behind an off-by-default flag, let the interactive epoch build distill 0–3
durable facts from the about-to-be-dropped turns into the persistent memory
system (category `"knowledge"`, source `"compaction"`), so multi-month sessions
accumulate knowledge the memory prompt tiers already surface. Best-effort:
never blocks or fails the epoch build.

## Architecture references

- `docs/design/context-management.md#38-ghost-coverage-and-memory-extraction`

## Pre-flight

1. Read `docs/dev/STANDARDS.md`.
2. Read this entire phase doc.
3. Clean branch, no uncommitted changes.
4. **Re-verify the Current-state anchors and resolve the KNOWN ISSUES below.**

## Known issues to resolve before dispatch (found 2026-07-16)

- **`format_messages_for_narrative` is PRIVATE** (`digest.rs:80`, `fn` not
  `pub fn`). The extraction fn (in `epochs.rs`) needs the transcript text of the
  dropped span. Either make it `pub(crate)`, or have the extraction fn format
  its own user text. Pick one and pin it — do not tell the executor to "reuse"
  a private fn it can't call.
- **`build_memory_namespaces` was NOT found** by grep in `memory_prompt.rs` /
  `memory/mod.rs`. Re-locate the real namespace-resolution API (or the constant
  for the global/default namespace) and quote it. Do not reference a symbol
  whose existence isn't confirmed.
- **`add_memory` path:** `crate::daemon::executor::knowledge::memory::add_memory`
  (`executor/knowledge/memory.rs:10`) is the tool-handler wrapper; it calls
  `crate::memory::add_memory(key, &stamped, cat, namespace)` (:30). Decide
  whether extraction calls the wrapper or `crate::memory::add_memory` directly,
  and quote the exact signature + how `source: "compaction"` is stamped.
- **Hook point:** the interactive epoch build is now in the phase-08 async task
  `run_compaction` (`context/background.rs`), after `append_epoch`
  (`background.rs:182`). Extraction runs there (still off the hot path). It must
  NOT run on the `ask.rs` emergency path (that path is model-call-free by
  design) nor the 10a ghost path.

## Spec (carried from original phase-10 — refine against the above)

### 1. Config flag — `CompactionConfig` (`config/types.rs:157`)

```rust
/// When true, each (interactive, async) epoch build asks the digest model to
/// propose 0–3 durable facts, written to persistent memory (category
/// "knowledge", source "compaction"). Off by default — one small-model call
/// per epoch, and it writes to shared memory.
#[serde(default)]
pub extract_memories: bool,
```

### 2. Extraction fn — `src/daemon/context/epochs.rs`

```rust
/// Best-effort: never blocks or fails the epoch build. Returns the number of
/// memories written (for the log).
pub async fn extract_memories_from_epoch(
    session_id: &str,
    record: &EpochRecord,
    dropped: &[Message],
    config: &Config,
) -> u32
```

- Gated on `config.compaction.extract_memories`; early-return 0 when false.
- Prompt via `summarize_once` (`digest.rs:160`), system prompt pinned verbatim:

  ```
  You extract durable operational knowledge from an SRE assistant's
  conversation. From the transcript chunk, output 0 to 3 facts that will
  still be true and useful weeks from now (host quirks, root causes,
  fixed configurations, learned procedures). Output STRICT JSON only:
  [{"name":"kebab-case-slug","content":"one to three sentences"}] or [].
  No prose. Do NOT record transient state (current disk %, running PIDs),
  speculation, or anything already obvious from the runbooks.
  ```

- **Factor the parse+write out of the async fn** as a synchronous, testable
  `fn apply_extraction(json: &str, session_id: &str, config: &Config) -> u32`
  so tests need no model.
- Parse strictly with `serde_json`; on parse failure or > 3 entries, log DEBUG
  and write nothing (never retry).
- Validate each `name` with the memory layer's slug rules; **skip** any name
  that already exists in the namespace (create-if-absent; no overwrites — dedupe
  via a memory existence check).
- Write with `source: "compaction"`, category `"knowledge"`, namespace resolved
  per the Known-issues item.
- `log_event("memory_extracted", {"session", "epoch_seq", "count"})` when
  count > 0.

### 3. Call site — `context/background.rs` `run_compaction`

After `append_epoch` (`background.rs:182`), `let _ =
extract_memories_from_epoch(&snapshot.session_id, &record, &snapshot.messages[..tail_start], config).await;`
(still inside the async task, off the interactive hot path). Do NOT add it to
the `ask.rs` emergency path or the 10a ghost path.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] `extract_memories = false` (default): no memory writes, no summarizer call
      during epoch builds (**negative case**).
- [ ] Flag on + mocked/failed summarizer: epoch build still succeeds, zero
      memories written (best-effort pinned).
- [ ] Valid extraction JSON → memory files exist with `source: "compaction"`
      frontmatter, pass the memory layer's validation; a duplicate name is
      skipped, not overwritten (**negative case**).
- [ ] Malformed JSON / 4+ facts → nothing written (**negative case**).

## Test plan

FS tests take `TEST_HOME_LOCK` + temp HOME.

- `extract_parses_strict_json_and_writes` — feed `apply_extraction` a canned
  two-fact JSON; assert two memory files with `source: "compaction"`.
- `extract_rejects_malformed_and_excess` — bad JSON and a 4-fact array → 0.
- `extract_skips_existing_name` — pre-create a name; assert not overwritten.
- `extract_flag_off_writes_nothing`.

## End-to-end verification

Run `apply_extraction` with a valid two-fact JSON fixture; quote `ls` of the
memory directory and one file's frontmatter showing `source: "compaction"`.

## Authorizations

None.

## Out of scope

- No extraction from ghost epochs (10a ghost path stays model-call-free).
- No memory *updates* or confidence re-scoring — create-if-absent only.
- No changes to the memory schema.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
