# G5 — Agent-to-Agent Delegation: Audit Report

**Branch:** `named-agent`  
**Date:** 2026-05-16  
**Reviewer:** Claude Sonnet 4.6

---

## Summary

G5 adds coordinator-to-specialist ghost shell delegation via three mechanisms: a mailbox store for result handoff, a `spawn_depth` guard that caps the hierarchy at two levels, and an `await_agent_result` tool that polls the mailbox. The structural pieces are sound, but two bugs in the `await_agent_result` dispatch path mean the feature cannot work end-to-end as designed in the coordinator → specialist scenario. Both are straightforward to fix.

---

## What Was Reviewed

| File | Role |
|---|---|
| `src/agents/mailbox.rs` | MailboxResult struct, write/read helpers, masking |
| `src/agents/mod.rs` | `pub mod mailbox` declaration |
| `src/ipc.rs` | `GhostConfig.spawn_depth` / `parent_job_id` fields |
| `src/daemon/executor/mod.rs` | Depth check; `AwaitAgentResult` dispatch |
| `src/daemon/executor/knowledge.rs` | `spawn_ghost()`, `await_agent_result()` |
| `src/daemon/executor/foreground.rs` | `GhostCtx` destructuring (updated) |
| `src/daemon/ghost.rs` | `write_mailbox_on_exit()`, `trigger_ghost_turn()` refactor |
| `src/daemon/stream.rs` | `AiEvent::AwaitAgentResult` → `PendingCall` conversion |
| `src/ai/types.rs` | `PendingCall::AwaitAgentResult`, `AiEvent::AwaitAgentResult` |
| `src/ai/tools.rs` | `await_agent_result` tool definition and dispatch |
| `src/runbook.rs` | `GhostConfig` initializer with new fields |
| `tests/integration.rs` | Three G5 integration tests |

---

## Defects

### D1 — Critical: `await_agent_result` polls the wrong mailbox

**Location:** `src/daemon/executor/mod.rs:557–573`

```rust
PendingCall::AwaitAgentResult { job_id, timeout_secs, .. } => {
    let agent_name = session_id
        .and_then(|sid| {
            sessions.lock().ok()?
                .get(sid)
                .and_then(|e| e.ghost_config.as_ref())
                .and_then(|gc| gc.agent.clone())
        })
        .unwrap_or_else(|| "global".to_string());
    knowledge::await_agent_result(job_id, *timeout_secs, &agent_name, &memory_namespaces).await
}
```

The `agent_name` is resolved from the **calling session's** `gc.agent` — the coordinator's own identity. But `write_mailbox_on_exit` writes to the **child's** `gc.agent` mailbox directory (`~/.daemoneye/agents/<child_agent>/mailbox/<job_id>.json`).

Concrete failure: coordinator (agent `"coordinator"`) spawns specialist (agent `"analyst"`) → specialist writes `agents/analyst/mailbox/<job_id>.json` → coordinator calls `await_agent_result(job_id=...)` → executor looks in `agents/coordinator/mailbox/<job_id>.json` → file not found → 5-minute timeout, every time.

**Fix:** The tool result returned by `spawn_ghost_shell` should include the child's agent name, or `await_agent_result` needs an optional `agent_name` parameter. The simplest fix is to include `agent: <name>` in the tool result string so the coordinator AI can pass it to `await_agent_result`, and add an `agent_name` param to the tool schema.

Alternatively, the mailbox could be keyed by `job_id` alone (under a shared `agents/mailbox/` path), making it agent-agnostic. This is cleaner but a larger change.

---

### D2 — Significant: `parent_job_id` threaded incorrectly

**Location:** `src/daemon/executor/mod.rs:167–178`, `src/daemon/executor/knowledge.rs:1001`

```rust
let (spawn_depth, parent_job_id): (u8, Option<String>) = if let Some(sid) = session_id {
    store.get(sid)
        .and_then(|e| e.ghost_config.as_ref())
        .map(|gc| (gc.spawn_depth, gc.parent_job_id.clone()))  // <-- gc.parent_job_id
        .unwrap_or((0, None))
} else { (0, None) };
```

```rust
// in spawn_ghost():
ghost_config.parent_job_id = parent_job_id.map(|s| s.to_string());
```

`parent_job_id` is read from `gc.parent_job_id` — the *current* session's own parent (i.e., the grandparent of the child being spawned). When the coordinator's `gc.parent_job_id` is `None` (it was started by a user session, not another ghost), the specialist is spawned with `parent_job_id = None`. The specialist's parent ID should be `session_id` (the coordinator's session), not `gc.parent_job_id`.

The correct line is:
```rust
// in the executor, before calling spawn_ghost:
let effective_parent_job_id = session_id.map(|s| s.to_string());
```
and pass `effective_parent_job_id.as_deref()` instead of `parent_job_id.as_deref()`.

This bug means all `parent_job_id` fields in child ghosts will be `None` unless the spawning session is itself a depth-1 ghost with a known parent, making the lineage chain impossible to reconstruct.

---

### D3 — Minor: `task` field in `MailboxResult` carries internal metadata, not the user task

**Location:** `src/daemon/ghost.rs:81–90`

```rust
let task_desc = match &parent_job_id {
    Some(pid) => format!("ghost shell for session {} (depth {}, parent: {})", ...),
    None => format!("ghost shell for session {} (depth {})", ...),
};
```

The `task` field was designed to convey *what* the agent was asked to do. The coordinator AI will see `"ghost shell for session abc-123 (depth 2)"` — not the actual task description it passed to `spawn_ghost_shell`. The `message` argument from `spawn_ghost` should be stored (in the `SessionEntry` or passed through) and used as the task description.

---

### D4 — Minor: Depth limit integration tests verify struct state, not executor enforcement

**Location:** `tests/integration.rs:1068–1130`

`g5_depth_limit_enforced` checks `gc.spawn_depth >= 2` as a boolean assertion on constructed struct values. It doesn't exercise the actual executor gate at `executor/mod.rs:501`. Similarly, `g5_child_inherits_depth_and_parent` manually sets fields and asserts them — it doesn't call `spawn_ghost()`.

The executor check and the depth increment in `spawn_ghost()` should each have a test that invokes the relevant function with a mock or real session, confirming the gate fires and the child's depth is correctly set.

---

### D5 — Minor: Ghost shell prompt doesn't document coordinator tools

**Location:** `assets/prompts/ghost-shell.txt`, `assets/prompts/sre.toml`

`ghost-shell.txt` describes the autonomous remediation role but says nothing about `spawn_ghost_shell` or `await_agent_result`. A ghost acting as a coordinator has no system-prompt guidance on how to delegate, collect results, or use the two-level hierarchy. `sre.toml` mentions `spawn_ghost_shell` briefly but omits `await_agent_result` entirely.

This matters because the coordinator AI will compose tool calls based only on the tool schema and whatever appears in the system prompt. Without explicit guidance the coordinator may misuse the handoff (e.g., poll with `read_file` instead of `await_agent_result`, or not call `await_agent_result` at all).

---

### D6 — Minor: No upper bound on `timeout_secs`

**Location:** `src/ai/tools.rs:762–766`, `src/daemon/executor/knowledge.rs:1278`

The tool schema documents a default of 300 s but imposes no maximum. An AI could pass `timeout_secs: 86400`, blocking the AI turn for 24 hours. A practical cap (e.g., 3600 s) enforced in `await_agent_result` before the `tokio::time::timeout` call would prevent runaway waits.

---

## What Works Well

**Mailbox I/O** is solid: atomic tmp→rename write, masking applied on write (not on read), correct `Ok(None)` for absent files, and `serde_json::to_string_pretty` for human-readable output. The four unit tests in `mailbox.rs` cover the happy path, absent file, status transition, and masking.

**Depth guard** is placed correctly — before the `spawn_ghost` call in the executor, so an agent at depth 2 simply receives an error string and continues its turn. The error message is accurate: "Delegation depth limit reached (max: coordinator + 1 level of specialists)."

**`spawn_depth` increment** in `knowledge::spawn_ghost` (`depth + 1`) is correct: a user session (depth 0) spawns a coordinator at depth 1, which spawns a specialist at depth 2, which is blocked from spawning further.

**`write_mailbox_on_exit` lifecycle** is correct: called from `trigger_ghost_turn` after `do_ghost_turn` returns (both success and error paths), best-effort (failures logged but don't propagate). Including the last assistant message in `result` even on failure is a good call — the coordinator can see the ghost's partial work.

**`ToolStarted`/`ToolFinished` feedback** is correctly enabled for `AwaitAgentResult` (`should_emit_tool_feedback` returns `true`). Long-polling tools should show an elapsed timer.

**`GhostConfig` serde defaults** use `#[serde(default)]` correctly — wire-safe for old records that predate these fields.

**`runbook.rs` initializer** correctly hard-codes `spawn_depth: 0` and `parent_job_id: None` for runbook-loaded configs.

---

## Summary Table

| # | Severity | File | Issue |
|---|---|---|---|
| D1 | **Critical** | `executor/mod.rs:562` | `await_agent_result` looks up coordinator's mailbox instead of child's |
| D2 | **Significant** | `executor/mod.rs:172`, `knowledge.rs:1001` | `parent_job_id` carries grandparent ID, not current session ID |
| D3 | Minor | `ghost.rs:81` | `task` field in mailbox result is internal metadata, not the task message |
| D4 | Minor | `tests/integration.rs:1068` | Depth integration tests verify struct fields, not executor behavior |
| D5 | Minor | `ghost-shell.txt`, `sre.toml` | Coordinator tools undocumented in system prompts |
| D6 | Minor | `tools.rs:762`, `knowledge.rs:1278` | No upper bound on `timeout_secs` |
