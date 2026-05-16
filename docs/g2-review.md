# G2 Memory Namespacing — Code Review

*Reviewed 2026-05-15 against `multi-agent` branch. 610 unit tests + 12 integration tests pass. Clippy clean.*

---

## Summary

The core namespace architecture is correct and well-executed. `memory_dir_for_namespace()` routes reads and writes correctly. The CRUD API (`add_memory`, `read_memory`, `delete_memory`, `list_memories`, `list_memories_with_tags`) is consistently namespace-aware. Namespace context threading through `ArtifactCtx` and the `executor/mod.rs` build block is clean. `migrate_namespace()` is called at daemon startup and is idempotent. The 11 namespace-specific unit tests in `memory_tests.rs` cover the required cases from the plan.

Three defects require fixes before G3 starts. Two are high-severity correctness bugs that silently produce wrong behavior for agent-namespaced ghost shells.

---

## Defects

### 1. `load_session_memory_block` reads only from global, ignoring the namespaces it was given — HIGH

**File:** `src/memory.rs:537–549`

The function accepts `namespaces: &[&str]` and correctly *scans* those directories to build an entry list. But when it reads the actual file content to build the block, it unconditionally reads from the global directory:

```rust
// Line 539 — hardcoded "global" despite accepting namespaces parameter
let path = memory_dir_for_namespace("global", &MemoryCategory::Session).join(format!("{}.md", key));
```

An agent ghost shell that writes session memories to its own namespace will have those keys collected in the scan, but the read path at line 539 looks in the wrong directory and silently produces nothing. The comment on line 538 ("prefer global for session") is incorrect rationale — session memories are per-session context, not global facts, and an agent's session memories are explicitly *not* in global.

**Fix:** carry the namespace alongside the key through the scan loop so the read uses the correct path.

```rust
// Collect (key, namespace, mtime) tuples
let mut entries: Vec<(String, String, std::time::SystemTime)> = Vec::new();
for ns in namespaces {
    let dir = memory_dir_for_namespace(ns, &MemoryCategory::Session);
    // ... scan loop produces (stem, ns.to_string(), mtime) entries
}
// In the read loop:
let path = memory_dir_for_namespace(&ns, &MemoryCategory::Session).join(format!("{}.md", key));
```

---

### 2. `ftsearch_memories` and `assemble_turn_relevant_memory` hardcode `&["global"]` — HIGH

**Files:** `src/daemon/memory_prompt.rs:206`, `src/daemon/memory_prompt.rs:345`

Two separate places hard-code the namespace scope:

```rust
// Line 206 — assemble_turn_relevant_memory
let all_memories = list_memories_with_tags(None, &["global"]).unwrap_or_default();

// Line 345 — ftsearch_memories
let all_memories = list_memories_with_tags(None, &["global"]).unwrap_or_default();
```

Both functions are the heart of the tiered memory prompt — the dynamic turn-relevant block that surfaces contextually relevant memories per AI turn. For an agent ghost shell, these calls need to search the agent's namespace in addition to global. As implemented, an agent that has built up domain knowledge in its own namespace will never see that knowledge injected into its prompts via the tiered system, even though all the routing infrastructure exists.

Both functions are currently called without namespace context because they don't accept it as a parameter. The calling chain needs to be extended: the ghost shell's `ArtifactCtx.namespaces` (or a derived slice) should flow into these functions.

**Fix:** add a `namespaces: &[&str]` parameter to `assemble_turn_relevant_memory` and `ftsearch_memories`, threading the value from the executor context. Default to `&["global"]` at all existing non-agent call sites.

---

### 3. `search_repository` does not search agent memory namespaces — MEDIUM

**File:** `src/daemon/executor/knowledge.rs:589`

```rust
pub(super) fn search_repository(query: &str, kind: &str) -> String {
    let results = crate::search::search_repository(query, kind, 2);
    crate::search::format_results(&results)
}
```

`crate::search::search_repository` only searches the global memory directory. The plan (G2.3 exit criterion, design doc §Memory Namespacing) explicitly states: "search_repository — includes agent namespace in the search scope."

The `search_repository` executor function receives no namespace context. The `ArtifactCtx` is available at the call site but is not passed through. This means an agent that stores knowledge in its own namespace cannot find it via `search_repository` — only via `read_memory` / `list_memories` by exact key.

**Fix:** pass `artifact_ctx.namespaces` to `search_repository` executor function, and extend `crate::search::search_repository` (or add a `search_repository_in_namespaces` variant) to scan the provided namespace directories for memory results.

---

## Missing Exit Criterion

### 4. `g2_namespace_isolation` integration test not in `tests/integration.rs` — LOW

The plan (G2 exit criteria) specifies: `integration::agent_memory_namespace_isolation`. The namespace tests were implemented in `memory_tests.rs` as unit tests, not in the integration test file. The unit tests are equivalent in coverage and more thorough, but the plan commitment was for a named integration test.

This is a documentation gap rather than a correctness problem. Either add an `g2_namespace_isolation` test to `tests/integration.rs` that exercises the cross-module path (executor → memory → file), or update the plan's exit criteria to reflect that this is covered by unit tests.

---

## What Is Done Well

- **`memory_dir_for_namespace` routing is correct.** Global goes to `~/.daemoneye/memory/<cat>/`; agent namespaces go to `~/.daemoneye/agents/<ns>/memory/<cat>/`. The path structure matches what `agents::agent_dir()` expects.
- **Namespace context threading in executor is clean.** The `memory_namespaces_owned` build block in `executor/mod.rs:125–145` is correct: agent namespace first, `read_namespaces` extras, then global appended if absent. Interactive sessions unconditionally get `&["global"]`.
- **`read_memory` executor correctly multi-searches.** `knowledge.rs:519–525` loops namespaces in order and returns the first hit, which is the right precedence (agent before global).
- **Migration is safe and correctly placed.** `migrate_namespace()` is idempotent, only touches files with frontmatter, skips files that already have the field, and runs at daemon startup before any memory operations.
- **All plan-specified unit tests are present and correct.** `write_agent_reads_agent`, `write_agent_invisible_to_global`, `fallback_to_global`, `fts5_namespace_filter` (covered by `list_memories_scopes_to_namespaces`), `migrate_namespace_adds_missing`, `migrate_namespace_skips_already_migrated`, `delete_memory_deletes_from_correct_path` — all present in `memory_tests.rs` and passing.
- **`build_frontmatter_omits_global_namespace` is a good design call.** Writing `namespace: global` to every global memory file would add noise to human-readable files. Omitting it (with parse defaulting to `"global"`) is cleaner.

---

## Fix Priority

| # | Severity | Must fix before G3? |
|---|---|---|
| 1 | High | Yes — agent session memories silently vanish from context blocks |
| 2 | High | Yes — tiered memory prompt is namespace-blind for agent ghost shells |
| 3 | Medium | Yes — search_repository was explicitly listed in the G2 exit criteria |
| 4 | Low | No — unit tests provide equivalent coverage |
