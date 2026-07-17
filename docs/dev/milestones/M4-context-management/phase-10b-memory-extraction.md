# Phase 10b: Opt-in compaction → memory extraction

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-06 (`summarize_once`), phase-08 (async epoch build), phase-10a
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Behind an off-by-default flag, let the interactive **async** epoch build distill
0–3 durable facts from the about-to-be-dropped turns into the persistent memory
system (category `"knowledge"`, source `"compaction"`), so multi-month sessions
accumulate knowledge the memory prompt tiers already surface. Best-effort:
never blocks or fails the epoch build.

This is 10b of the 10a/10b split. It touches an independent subsystem from 10a
(epoch-build + memory, not the ghost loop). **10b's approval closes M4.**

## Architecture references

- `docs/design/context-management.md#38-ghost-coverage-and-memory-extraction`

## Pre-flight

1. Read `docs/dev/STANDARDS.md`.
2. Read this entire phase doc.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

*(Anchors verified 2026-07-16 against HEAD after phase-10a. The known issues
from the pre-split draft are RESOLVED below — every symbol here is confirmed.)*

- **Hook point — `run_compaction`** (`src/daemon/context/background.rs:91`):
  `async fn run_compaction(snapshot: &CompactionSnapshot, sessions: &SessionStore, config: &Config) -> Result<(), String>`.
  It plans the cut (`tail_start`), builds the epoch narrative, then appends the
  epoch:

  ```rust
  // background.rs:182 (verified)
  epochs::append_epoch(&snapshot.session_id, &record);
  ```

  `config`, `snapshot.session_id`, `snapshot.messages`, `tail_start`, and
  `record` are all in scope right after this line. **This is the insertion
  point** — still inside the async task, off the interactive hot path.

- **Summarizer — `summarize_once`** (`src/daemon/digest.rs:160`):
  `pub async fn summarize_once(system: &str, user_text: &str, model_entry: &crate::config::ModelEntry) -> Option<String>`.
  Returns `None` on any failure/timeout (best-effort). The digest model is
  `config.resolve_model(Some("digest"))` — the exact call `run_compaction`
  already uses at `background.rs:162` for the narrative.

- **Transcript formatting — `format_messages_for_narrative`**
  (`src/daemon/digest.rs:80`): currently **private** (`fn`, not `pub fn`).
  Task 0 below makes it `pub(crate)` so the extraction fn can format the dropped
  span's user text. (One-word visibility widening; no behavior change.)

- **Memory write — `crate::memory::add_memory`** (`src/memory.rs:368`):
  `pub fn add_memory(key: &str, value: &str, category: MemoryCategory, namespace: &str) -> Result<()>`.
  It **writes `value` verbatim** to `<memory_dir>/<key>.md` (no frontmatter
  synthesis, no index sync). `MemoryCategory::Knowledge` (`src/memory.rs:11`),
  `MemoryCategory::from_str("knowledge")` → `Some(Knowledge)`. Call this
  **low-level fn directly**, NOT the executor wrapper
  (`executor::knowledge::memory::add_memory`, which needs an `ArtifactCtx` we
  don't have here).

- **Namespace:** there is **no** `build_memory_namespaces` symbol. The default /
  global namespace is the **string literal `"global"`** (see
  `memory_dir_for_namespace`, `src/memory.rs`, and every executor call site's
  `.unwrap_or("global")`). Compaction extraction is not agent-scoped, so it
  writes to `"global"`.

- **`source: "compaction"` stamping — IMPORTANT:** the memory frontmatter schema
  (`ParsedFrontmatter` / `build_frontmatter`, `src/memory.rs:72/180`) has **no
  `source` field**, and "No changes to the memory schema" is out of scope.
  Because `add_memory` writes `value` verbatim, stamp `source` by **hand-writing
  a frontmatter block** into the `value` string — see the worked example in
  Task 2. `parse_memory_frontmatter` tolerates unknown frontmatter lines (it only
  reads known prefixes), so this round-trips cleanly with no schema change.

- **Dedup / existence check — `crate::memory::read_memory`** (`src/memory.rs:387`):
  `pub fn read_memory(key: &str, category: MemoryCategory, namespace: &str) -> Result<String>`
  returns `Err` when the file is absent. So
  `crate::memory::read_memory(name, MemoryCategory::Knowledge, "global").is_ok()`
  == "already exists" → skip (create-if-absent, no overwrite).

- **Key validation — `validate_memory_key`** is internal to `add_memory` (rejects
  empty / `/` / `\0` / `.` / `..`); `add_memory` returns `Err` on a bad key, so
  invalid slugs are naturally skipped by checking the `Result`.

- **Config — `CompactionConfig`** (`src/config/types.rs:157`) has a **hand-written
  `Default` impl** (`:201`). Task 1 adds the new field to BOTH the struct AND the
  `Default` impl (a `#[serde(default)]` bool defaults to `false` for parsing, but
  the manual `Default::default()` still needs the field or the build breaks).

## Spec

### 0. Widen `format_messages_for_narrative` visibility — `src/daemon/digest.rs:80`

Change `fn format_messages_for_narrative` → `pub(crate) fn format_messages_for_narrative`.
Nothing else.

### 1. Config flag — `CompactionConfig` (`src/config/types.rs:157`)

Add to the struct:

```rust
/// When true, each (interactive, async) epoch build asks the digest model to
/// propose 0–3 durable facts, written to persistent memory (category
/// "knowledge", source "compaction"). Off by default — one small-model call
/// per epoch, and it writes to shared memory.
#[serde(default)]
pub extract_memories: bool,
```

Add `extract_memories: false,` to the hand-written `Default` impl (`:201`).

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

- Gated on `config.compaction.extract_memories`; **early-return 0 when false**
  (no summarizer call — this is the negative case the tests pin).
- Format the dropped span with the now-`pub(crate)`
  `crate::daemon::digest::format_messages_for_narrative(dropped)`.
- Call `crate::daemon::digest::summarize_once(SYSTEM, &user_text,
  config.resolve_model(Some("digest"))).await`. On `None`, return 0.
- System prompt pinned verbatim:

  ```
  You extract durable operational knowledge from an SRE assistant's
  conversation. From the transcript chunk, output 0 to 3 facts that will
  still be true and useful weeks from now (host quirks, root causes,
  fixed configurations, learned procedures). Output STRICT JSON only:
  [{"name":"kebab-case-slug","content":"one to three sentences"}] or [].
  No prose. Do NOT record transient state (current disk %, running PIDs),
  speculation, or anything already obvious from the runbooks.
  ```

- **Factor the parse+write out** as a synchronous, testable fn so tests need no
  model:

  ```rust
  /// Parse strict JSON, write create-if-absent memories, return count written.
  fn apply_extraction(json: &str, session_id: &str, epoch_seq: u32) -> u32
  ```

  `apply_extraction` does the work; `extract_memories_from_epoch` is the thin
  async wrapper (gate → format → summarize → `apply_extraction`).

- In `apply_extraction`:
  - Parse with `serde_json::from_str::<Vec<ExtractedFact>>` where
    `ExtractedFact { name: String, content: String }`.
  - On parse failure **or `> 3` entries**, `log::debug!(...)` and return 0
    (never retry, never partial-write the over-limit array).
  - For each fact: skip if `content` is empty; skip if
    `read_memory(&name, MemoryCategory::Knowledge, "global").is_ok()` (already
    exists); build the stamped body (below) and call
    `crate::memory::add_memory(&name, &body, MemoryCategory::Knowledge, "global")`
    — an `Err` (e.g. invalid slug) is skipped, not fatal. Count each successful
    write.

- **Stamped body — worked example** (this is how `source: "compaction"` is
  written without a schema change):

  ```rust
  let body = format!(
      "---\nsource: \"compaction\"\ncreated: \"{}\"\n---\n{}\n",
      chrono::Utc::now().to_rfc3339(),
      fact.content.trim(),
  );
  ```

  (`chrono::Utc::now()` is fine in daemon production code — it is only forbidden
  in rexyMCP *workflow scripts*.)

- When count > 0:
  `crate::daemon::utils::log_event("memory_extracted", serde_json::json!({"session": session_id, "epoch_seq": epoch_seq, "count": count}))`.

### 3. Call site — `run_compaction` (`src/daemon/context/background.rs`)

Immediately after `epochs::append_epoch(&snapshot.session_id, &record);`
(`background.rs:182`):

```rust
let _ = epochs::extract_memories_from_epoch(
    &snapshot.session_id,
    &record,
    &snapshot.messages[..tail_start],
    config,
)
.await;
```

Still inside the async task. Do **NOT** add it to the `ask.rs` emergency path
(model-call-free by design) or the 10a ghost path.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] `extract_memories = false` (default): `extract_memories_from_epoch`
      early-returns 0 with **no summarizer call and no memory write**
      (**negative case** — assert by exercising the gate with no backend).
- [ ] Flag on + summarizer returns `None` (mocked/failed): epoch build still
      succeeds, zero memories written (best-effort pinned).
- [ ] Valid two-fact JSON → two memory files exist under the global `knowledge`
      dir, each containing `source: "compaction"` in its frontmatter and passing
      `parse_memory_frontmatter` round-trip; a pre-existing name is **skipped,
      not overwritten** (**negative case**).
- [ ] Malformed JSON **or** a 4+-fact array → nothing written (**negative case**).

## Test plan

FS tests take `crate::TEST_HOME_LOCK` + a temp HOME (memory writes go under
HOME via `config_dir()`). Drive `apply_extraction` directly (no model needed).

- `extract_parses_strict_json_and_writes` — feed `apply_extraction` a canned
  two-fact JSON; assert two files under the global knowledge dir, each with
  `source: "compaction"` frontmatter; return value == 2.
- `extract_rejects_malformed_and_excess` — bad JSON → 0; a 4-fact array → 0
  (nothing written for either).
- `extract_skips_existing_name` — pre-create one of the two names via
  `crate::memory::add_memory`; assert the pre-existing file is unchanged and the
  return count is 1 (only the new one written).
- `extract_flag_off_writes_nothing` — call `extract_memories_from_epoch` with
  `extract_memories = false`; assert return 0 and no files created (this also
  proves the gate short-circuits before any summarizer call).

## End-to-end verification

Not a live-daemon check. In the completion log, quote the output of
`extract_parses_strict_json_and_writes` and an `ls` of the global knowledge
memory dir plus one written file's first lines showing the
`source: "compaction"` frontmatter.

## Authorizations

None. (`format_messages_for_narrative` visibility widening is an in-crate change,
not a STANDARDS §5 item.)

## Out of scope

- No extraction from ghost epochs (the 10a ghost path stays model-call-free).
- No memory *updates*, confidence re-scoring, or overwrites — create-if-absent
  only.
- **No changes to the memory frontmatter schema** — `source` is stamped as a raw
  frontmatter line in the written body, not a new parsed field.
- No changes to `summarize_once`, the digest model resolution, or the epoch
  record shape.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
