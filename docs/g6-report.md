# G6 — Polish & Integration: Audit Report

**Branch:** `named-agent`  
**Date:** 2026-05-16  
**Reviewer:** Claude Sonnet 4.6

---

## Summary

G6 is a polish and integration pass over the Named Agents feature: status observability, ghost-prompt awareness of available agents, prompt documentation, namespace migration verification, CLAUDE.md housekeeping, and additional tests. The mechanical changes are sound. Three targeted defects are documented below; the rest of the implementation is correct and complete.

---

## What Was Reviewed

| Sub-task | Files |
|---|---|
| G6.1 | `src/ipc.rs`, `src/daemon/server.rs`, `src/cli/status.rs` |
| G6.2 | `src/daemon/prompt.rs`, `src/daemon/ghost.rs` |
| G6.3 | `assets/prompts/sre.toml` |
| G6.4 | `src/daemon/mod.rs:386` (pre-existing `migrate_namespace()` call) |
| G6.5 | `CLAUDE.md` (doc-only) |
| G6.6 | `tests/integration.rs`, `src/ipc_tests.rs` |

---

## Defects

### D1 — Minor: `sre.toml` has two contradictory depth numbers in the same section

**Location:** `assets/prompts/sre.toml:356` vs. `:522–524`

Line 356 (correct):
```
a coordinator (depth 1) can spawn specialists (depth 2), but specialists cannot spawn further
```

Lines 522–524 (wrong):
```
A coordinator (depth 0) can spawn specialists (depth 1), but specialists cannot spawn further.
Attempting to spawn at depth 2 returns an error.
```

The implementation assigns `spawn_depth = 1` to a coordinator ghost (spawned from a user session at depth 0), and `spawn_depth = 2` to its specialists. "Attempting to spawn at depth 2 returns an error" is correct — a depth-2 specialist is blocked. But calling the coordinator "depth 0" and the specialist "depth 1" contradicts both the implementation and the accurate description four lines earlier in the same file.

`ghost-shell.txt:35–36` is also accurate: "you (depth 1) can spawn specialists (depth 2)".

**Fix:** Update lines 522–524 to match line 356: coordinator = depth 1, specialist = depth 2, blocked at depth 2.

---

### D2 — Minor: `active_agents` IPC field comment misnames the second tuple element

**Location:** `src/ipc.rs:594–596`

```rust
/// Active agent sessions: `(agent_name, job_id_or_idle)`.
#[serde(default)]
active_agents: Vec<(String, String)>,
```

The field is populated in `server.rs` as:

```rust
let job_id = entry
    .ghost_task_message
    .as_deref()
    .unwrap_or("unknown")
    .chars()
    .take(40)
    .collect();
agents.push((agent_name.clone(), job_id));
```

The second element is the task description (first 40 chars of `ghost_task_message`), not a job ID. The `status.rs` display confirms this: it shows `active (task description)` as the status string. The comment `job_id_or_idle` is wrong — the actual values are a task message snippet or the literal `"unknown"`.

**Fix:** Update the comment: `(agent_name, task_or_unknown)` — the second element is the first 40 chars of the task message passed to `spawn_ghost_shell`, or `"unknown"` if none was recorded.

---

### D3 — Minor: `g6_agent_memory_namespace_isolation` doesn't test what its name says

**Location:** `tests/integration.rs:1224–1272`

The test is documented as: "Verify that agent memory namespace isolation works: the `load_session_memory_block` function respects namespace filtering. Writes to an agent namespace are invisible when reading with global-only context."

What the test actually does:
1. Creates an agent config with namespace `"ns-isolation-agent"`.
2. Verifies the namespace field loaded back correctly.
3. Verifies the agent appears in `list_agents()`.
4. Deletes the agent.

No memories are written to the agent namespace. No namespace-filtered read is attempted. The isolation property that the docstring claims to verify is not exercised at all — the test is a subset of what `g6_agent_config_roundtrip` already covers (save → load → list → delete).

**Fix:** Either (a) rename and redocument the test to accurately describe what it does (agent CRUD with namespace field check), or (b) add the actual namespace isolation assertion: write a memory under the agent namespace, verify `build_memory_namespaces` for a regular session returns only `["global"]` and thus cannot read it, and verify a ghost session with that agent's namespace can.

---

## What Works Well

**G6.1 — Status observability:** The `active_agents` population logic in `server.rs` is correct: it iterates all session entries, filters by `ghost_config.agent.is_some()`, and sorts deterministically by agent name. The status display in `cli/status.rs` correctly merges `agents_defined` (registered on disk) with `active_agents` (running in memory), so idle agents still appear in the section — useful for quick confirmation that an agent profile exists. The `#[serde(default)]` on the new IPC field maintains backward compatibility.

**G6.2 — Ghost prompt injection:** `format_available_agents()` in `prompt.rs` is well-contained: it returns an empty string when no agents are registered (no extra blank sections), handles `list_agents()` errors gracefully, and formats consistently with the existing markdown style. Injection at the tail of the ghost system prompt (after `GHOST_SHELL_RULES`) is the right position — context-setting information belongs after operational rules, not before them.

**G6.4 — `migrate_namespace()`:** Correctly identified as pre-existing at `daemon/mod.rs:386`. No duplicate call introduced.

**G6.6 — Tests:**
- `g6_agent_config_roundtrip` exercises the full production CRUD path (`save_agent` → `load_agent` → field assertions → `delete_agent` → load fails), acquires `TEST_HOME_LOCK`, and cleans up correctly. This is a substantive integration test.
- `g6_tool_policy_enforced_in_ghost` correctly tests the `ToolPolicy` deny-list against a production `PendingCall` variant, verifying both that the blocked tool is denied and that unrelated tools are permitted.
- `response_daemon_status_roundtrip` in `ipc_tests.rs` was updated to include `active_agents` in the fixture (line 681). The match arm uses `..` to ignore the field, so the new field is not asserted after deserialization. Worth noting but not critical — the roundtrip for other fields is still sound, and the struct construct alone confirms the new field compiles and serializes.

---

## Summary Table

| # | Severity | Location | Issue |
|---|---|---|---|
| D1 | Minor | `sre.toml:522–524` | Coordinator labeled "depth 0", specialist "depth 1" — contradicts implementation and line 356 |
| D2 | Minor | `ipc.rs:594` | Comment says `job_id_or_idle`; actual second element is a task message snippet |
| D3 | Minor | `integration.rs:1224` | Test name/docstring claims namespace isolation; body only does agent CRUD |
