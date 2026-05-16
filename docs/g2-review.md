# G2 Memory Namespacing — Code Review

*Reviewed 2026-05-15 against `multi-agent` branch after bug-fix pass. 610 unit + 12 integration (1 ignored) tests pass. Clippy clean.*

---

## Summary

Bug 1 and Bug 3 from the initial review are correctly fixed. Bug 2 is not fixed — the changes were made to a file that is not compiled. One new defect was introduced in `search.rs` (low severity). The integration test gap remains.

---

## Bug Fix Assessment

### Bug 1 — `load_session_memory_block` reads wrong directory: **FIXED** ✓

`memory.rs:499–544`. The scan loop now collects `(key, namespace, mtime)` tuples and the read loop uses `memory_dir_for_namespace(ns, ...)` with the per-entry namespace. Correct.

---

### Bug 2 — Tiered memory prompt hardcodes `&["global"]`: **NOT FIXED**

`src/daemon/memory_prompt.rs` is not declared as a module anywhere in the codebase. The Rust compiler never sees it.

```
$ grep -rn "memory_prompt" src/
(no output)
```

`daemon/mod.rs` declares: `auto_name`, `background`, `digest`, `executor`, `ghost`, `hook`, `policy`, `prompt`, `scheduled`, `server`, `session`, `stats`, `stream`, `utils` — `memory_prompt` is absent. The file compiles in isolation (confirmed by test) but is excluded from the binary.

The namespace parameter was added to `assemble_turn_relevant_memory`, `ftsearch_memories`, `find_by_tag_overlap`, and `expand_relates_to`. These changes are real but they have zero runtime effect. The hardcoded `&["global"]` issue is still present in the compiled binary — it's just in a different call to `list_memories_with_tags` inside the same dead file.

**Required fix:** Add `pub mod memory_prompt;` to `src/daemon/memory_prompt.rs`'s declaration in `daemon/mod.rs`. After that, the namespace parameter threading in the file should work correctly, but the callers of `assemble_ambient_memory` and `assemble_turn_relevant_memory` also need to supply a namespace slice. Currently there are no callers of these functions outside `memory_prompt.rs` — either those call sites don't exist yet (G5 is only partially wired in) or they were lost. This needs to be investigated before the fix can be validated.

---

### Bug 3 — `search_repository` ignores agent namespaces: **FIXED** ✓

`search.rs:28–69`. `search_repository_with_namespaces` correctly builds memory directory paths from the namespace list, matching `memory_dir_for_namespace` routing (`global` → `~/.daemoneye/memory/<cat>/`, agent → `~/.daemoneye/agents/<ns>/memory/<cat>/`). The executor at `knowledge.rs:604` passes `artifact_ctx.namespaces`, and `executor/mod.rs:410` wires it through. The original `search_repository` wrapper defaults to `&["global"]` for backward compatibility. Correct.

---

## New Defect Introduced

### search.rs: global memory directories added unconditionally — LOW

`search.rs:54,57,60`: the condition `|| *ns == "global"` causes all three global memory category directories to be pushed even when they don't exist:

```rust
if mem_base.join("session").exists() || *ns == "global" {
    dirs.push((mem_base.join("session"), "memory/session".to_string()));
}
```

For agent namespaces, the `exists()` check correctly avoids pushing empty dirs. For global, all three are always pushed. This is harmless in practice since subsequent `read_dir` calls fail gracefully when the directory is absent, but it makes the search slightly less efficient on fresh installs with no memories and is inconsistent with how agent paths are handled.

The correct pattern is either: check existence for all namespaces, or remove the check entirely (let `read_dir` handle missing dirs). Not blocking, but should be made consistent.

---

## Gap 4 — Integration test: **UNCHANGED**

`g2_namespace_isolation` integration test still not in `tests/integration.rs`. Unit coverage in `memory_tests.rs` is comprehensive (8 namespace-specific tests). Acceptable as-is if the plan exit criteria is updated to reflect this.

---

## What Still Needs to Happen for G2 to Be Closed

| # | Item | Action |
|---|---|---|
| 1 | ~~`load_session_memory_block` reads wrong directory~~ | Fixed ✓ |
| 2 | `memory_prompt.rs` not compiled — Bug 2 unfixed | Add `pub mod memory_prompt;` to `daemon/mod.rs`; verify callers pass namespace slice |
| 3 | ~~`search_repository` ignores agent namespaces~~ | Fixed ✓ |
| 4 | `search.rs` unconditional global dir push | Minor cleanup, not blocking |
| 5 | Integration test gap | Update plan exit criteria or add test |

G2 cannot be considered closed until item 2 is resolved. The other items are minor.
