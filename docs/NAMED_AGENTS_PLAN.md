# Named Agents: Implementation Plan

*Drafted 2026-05-15. Companion design document: `NAMED_AGENTS.md`.*

---

## Engineering Standards

These standards apply to every phase. An implementing agent must satisfy all of them before a phase is considered complete. They are not suggestions.

### Definition of Done

A phase is done when **all** of the following are true:

1. `cargo build` succeeds with zero errors.
2. `cargo clippy --all-targets -- -D warnings` exits zero. This is a hard CI gate — no `#[allow(...)]` suppressions without a comment explaining why the suppression is correct and permanent.
3. `cargo test` passes with no regressions. All tests listed under the phase's **Tests** section are implemented and passing.
4. Every new public function, struct, and enum in non-test code has correct Rust doc coverage where a caller would benefit from it. Trivial getters and newtype wrappers do not require docs.
5. The feature behaves correctly end-to-end against the exit criteria stated at the bottom of the plan. Exit criteria are behavioral checks, not just compilation checks.
6. No dead code is introduced. `cargo clippy` will catch most cases; also check for unreachable match arms and `pub` items used only within the same module.
7. CLAUDE.md is updated if the phase adds a new source file to the key files table, a new AI tool, a new IPC variant, or a new global static.

### Code Quality

**Error handling**
- Use `anyhow::Result` for fallible functions that cross module boundaries. Do not use bare `unwrap()` or `expect()` in non-test production code.
- Mutex lock sites must use `.unwrap_or_log()` (the `UnpoisonExt` trait from `src/util.rs`). Never use `.unwrap()` or `.expect()` on a `LockResult`.
- Do not add error handling for scenarios that cannot happen. Trust Rust's type system and framework guarantees. Validate only at real boundaries (user input, file I/O, IPC deserialization).

**Comments**
- Write no comments by default. Add a comment only when the WHY is non-obvious: a hidden constraint, a subtle invariant, a specific bug workaround. "This function saves the config" is not a useful comment.
- Do not write comments that reference the current task, PR, or issue number — those belong in commit messages, not source code.

**Naming and structure**
- Follow the existing module layout: `src/agents/` for agent CRUD and policy; executor arms go in `src/daemon/executor/`; IPC variants go in `src/ipc.rs`. Do not introduce new top-level modules without a clear reason.
- New AI tools follow the exact checklist in CLAUDE.md §"Adding a new AI tool". Every step in the checklist must be completed — missing one step causes a runtime panic or silent incorrect behavior.
- Prefer editing existing files to creating new ones. Create a new file only when the code clearly belongs in its own module.

**Concurrency**
- All new async code runs inside the existing tokio runtime. Do not spawn threads with `std::thread::spawn` for async work.
- New broadcast or mpsc channels follow the pattern established by `BG_DONE_TX`: declare as `OnceLock<...>` in `src/daemon/mod.rs`, initialize once at daemon startup.
- Do not hold a mutex guard across an `.await` point. Scope guards tightly; drop before any async call.

**Security**
- Any content written to disk that originates from an AI model response (briefings, mailbox results, summaries) must pass through `ai::filter::mask()` before the write. No exceptions.
- Agent configs are sensitive (they define tool policy and auto-approve lists). The `create_agent` and `delete_agent` AI tools are approval-gated, same as `edit_file`. Do not make them silent tools.
- The model does not choose its own namespace, tool policy, or spawn depth. These are daemon-imposed from `GhostConfig`. Any place where the model's output could influence these values is a security defect.

**Backwards compatibility**
- All new frontmatter fields (`agent:` in runbooks, `namespace:` in memories) must be `Option<...>` or have a default value so existing files parse without error.
- All new `GhostConfig` fields must have sensible defaults so existing ghost shell code paths that do not populate these fields continue to work.
- No existing IPC `Request` or `Response` variants may be removed or have non-`#[serde(default)]` fields added. Schema evolution is additive only.

### Test Coverage

**Unit tests**
- Every new pure function with non-trivial logic has at least one unit test. "Non-trivial" means: branching logic, string manipulation, state transitions, or any function whose output is not immediately obvious from its inputs.
- Tests live in a `#[cfg(test)] mod tests { ... }` block at the bottom of the same file, following the existing pattern.
- Tests that mutate the `HOME` environment variable must hold `crate::TEST_HOME_LOCK` for the duration of the mutation to prevent races.

**Integration tests**
- Each phase specifies integration test cases in its **Tests** section. These must be added to `tests/integration.rs` (or a new `tests/integration_agents.rs` file if the existing file exceeds ~600 lines).
- Integration tests import production types from `daemoneye::*`. No local re-declarations of `Request`, `Response`, or any other production struct.
- Tests that require a live tmux session or a real AI API key are marked `#[ignore]` with a comment explaining the requirement.

**Test naming**
- Unit test names are `snake_case` and describe the scenario, not the function: `roundtrip_empty_allow_list`, not `test_tool_policy`.
- Integration test names are prefixed with the phase slug: `g1_agent_config_roundtrip`, `g2_namespace_isolation`.

**Minimum counts**
- Phase G1 must not decrease the total passing test count from its baseline. Each subsequent phase must add at least as many tests as it adds source files.
- `cargo test` must not have any new `#[ignore]` tests without the comment explaining the external dependency.

---

## Guiding Principles

1. **Runbooks are unchanged unless opted in.** Every existing ghost shell, runbook, and schedule continues to work. `agent:` is additive.
2. **Agents are config, not process.** No new daemon threads or persistent agent processes. An agent is a config profile applied at ghost-shell spawn time.
3. **Tool policy is enforced by the daemon.** Policy is not a system prompt instruction — it is enforced in `execute_tool_call()`. The model cannot talk its way out.
4. **Phases are independently shippable.** Each phase leaves the codebase in a working state. Phase G2 is useful without G5.

---

## Phase G1 — Agent Foundation (3–5 days)

### Goal
Agents can be created, listed, read, and deleted. The interactive AI can manage agents via tools. Ghost shells can be spawned with an agent. No memory namespacing yet; tool policy enforcement is not yet wired.

### G1.1 — `AgentConfig` struct and storage

**New file: `src/agents/mod.rs`**

```rust
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub prompt: String,          // role-defining system prompt addition
    pub model: Option<String>,   // model key from [models.*] config; None = default
    pub memory_namespace: String,// defaults to agent name
    pub max_turns: Option<u32>,  // per-invocation turn budget; None = daemon default
    pub auto_approve_read_only: bool,
    pub auto_approve_scripts: Vec<String>,
}
```

Storage: `~/.daemoneye/agents/<name>/config.toml`. One directory per agent; directory name is the slug (kebab-case, validated like session names). `list_agents()` walks the directory. `load_agent()` / `save_agent()` / `delete_agent()` follow the pattern of `runbook.rs`.

**New file: `src/agents/tools.rs`** — tool policy config (separate struct, added in G4).

**Validation:** agent name must be `[a-z0-9-]+`, 1–48 chars, no leading/trailing dash. Same validation as `validate_session_name()` in `session_store.rs`.

### G1.2 — `daemoneye agent` CLI subcommand

Extend `src/main.rs` and `src/cli/commands.rs`:

```
daemoneye agent list                    # tabular: name, model, description
daemoneye agent show <name>             # full config dump
daemoneye agent create <name>           # opens $EDITOR with a starter config.toml
daemoneye agent delete <name>           # confirm prompt
daemoneye agent briefing <name>         # show/edit/delete the last briefing
```

### G1.3 — AI tools: `create_agent` / `read_agent` / `list_agents` / `delete_agent`

Add to `src/ai/tools.rs` TOOLS slice (all backends auto-covered). Follow the `write_runbook` / `read_runbook` pattern. These are approval-gated (same as `edit_file`) — agent configs are sensitive.

Add `AiEvent` and `PendingCall` variants per the checklist in CLAUDE.md.

Executor: `src/daemon/executor/knowledge.rs` (alongside runbook/memory tools). Include `session_origin` stamping.

### G1.4 — `spawn_ghost_shell` tool gains `agent` parameter

In `src/ai/types.rs`: add `agent: Option<String>` to `PendingCall::SpawnGhostShell { ... }`.

In `src/ai/tools.rs`: add `agent` property to the `spawn_ghost_shell` tool JSON definition. Description: "Name of a named agent to use as the executor identity. Inherits prompt, model, tool policy, and memory namespace from the agent config."

In `src/daemon/executor/mod.rs`: when `agent` is present, load the `AgentConfig` and merge it into the `GhostConfig` before `GhostManager::start_session()`.

### G1.5 — Runbook frontmatter `agent:` field

In `src/runbook.rs`: add `agent: Option<String>` to the runbook frontmatter struct. When a ghost shell is triggered from a runbook with `agent: <name>`, load the agent config and merge into `GhostConfig` before spawn. If the named agent does not exist, log a warning and proceed with defaults (do not fail).

### G1.6 — `GhostConfig` integration

In `src/config.rs` (or a new `src/agents/ghost.rs`): add `merged_from_agent: Option<String>` to `GhostConfig` for logging/audit purposes. Merge order for fields with conflicts (both runbook and agent specify `model`): runbook wins, agent is the default.

**Tests (G1):**
- `agents::tests::roundtrip` — save and load an `AgentConfig`, assert equality.
- `agents::tests::name_validation` — valid and invalid name slugs.
- `agents::tests::list_empty` and `list_populated` — directory walk.
- Integration: `spawn_ghost_shell_with_agent` — verify merged `GhostConfig` picks up agent model and prompt (using a mock agent, no live spawn).

---

## Phase G2 — Memory Namespacing (2–3 days)

### Goal
Agent-scoped ghost shells read/write to an agent memory namespace. Global memories remain accessible as a fallback. The interactive session is unaffected.

### G2.1 — Memory namespace field

In `src/memory/mod.rs`: add `namespace: String` to the `Memory` struct (default `"global"`). Add `namespace` to the on-disk frontmatter format. Existing memories without a `namespace` field parse as `"global"`.

Migration: `src/memory/migrate.rs` gets a `migrate_namespace()` helper (idempotent) that adds `namespace: global` to existing entries missing the field.

### G2.2 — Namespace-aware CRUD

`add_memory()` — accepts `namespace: &str`. Writes to `~/.daemoneye/agents/<namespace>/memory/` when namespace != `"global"`, else to the existing `~/.daemoneye/memory/` path.

`read_memory()` / `list_memories()` — accept `namespaces: &[&str]`. When called from an agent ghost shell, pass `&[agent_name, "global"]`. Returns each memory with its namespace annotated.

`delete_memory()` — operates on whichever path holds the memory.

**FTS5 index** (`src/memory/index.rs`): add `namespace TEXT NOT NULL DEFAULT 'global'` column. `migrate_schema()` adds the column via ALTER TABLE if absent. Queries optionally filter by namespace list.

### G2.3 — Namespace context threading

`GhostConfig` gains `memory_namespace: Option<String>`. When `memory_namespace` is `Some(ns)`, the executor passes `namespaces: &[ns, "global"]` to all memory tool calls in that ghost shell.

Executor memory tool dispatch (in `executor/knowledge.rs`) reads `GhostConfig.memory_namespace` from context rather than accepting it from the model. The model does not choose its own namespace — the daemon imposes it.

### G2.4 — `read_namespaces` for cross-agent reads

`AgentConfig` gains:
```toml
read_namespaces = ["postgres-dba"]  # in addition to own namespace + global
```

When threading namespace context, include extra namespaces from this list.

### G2.5 — Interactive session protection

The interactive session (`handle_ask` path, non-ghost) always uses `namespaces: &["global"]`. Named agent namespaces are invisible to interactive sessions. This is intentional: interactive work should not be silently shaped by agent memory that the operator cannot see.

**Tests (G2):**
- `memory::namespace::write_agent_reads_agent` — write to agent namespace, read with agent context, confirm returned.
- `memory::namespace::write_agent_invisible_to_global` — write to agent namespace, read with global-only context, confirm absent.
- `memory::namespace::fallback_to_global` — agent-context read of a key that exists only in global returns the global entry.
- `memory::namespace::fts5_namespace_filter` — FTS5 search with namespace filter returns only matching-namespace entries.

---

## Phase G3 — Tool Policy Enforcement (2–3 days)

### Goal
Agents can define an `allow` or `deny` tool list. The daemon enforces it before dispatch. The model sees a structured error for denied tools.

### G3.1 — `ToolPolicy` struct

**New file: `src/agents/policy.rs`** (extend existing `src/daemon/policy.rs` or keep separate):

```rust
pub struct ToolPolicy {
    pub allow: Option<Vec<String>>,  // if Some, only these tools permitted
    pub deny: Option<Vec<String>>,   // if Some, all tools except these permitted
    // allow and deny are mutually exclusive; validation rejects both-set
}

impl ToolPolicy {
    pub fn permits(&self, tool_name: &str) -> bool { ... }
}
```

`AgentConfig` gains a `tools: Option<ToolPolicy>` field, loaded from a `[tools]` section in `config.toml`.

### G3.2 — Enforcement in `execute_tool_call()`

In `src/daemon/executor/mod.rs`, before the existing `GhostPolicy` check:

```rust
if let Some(policy) = &ghost_config.tool_policy {
    if !policy.permits(pending_call.tool_name()) {
        return Ok(ToolCallOutcome::Result(
            format!("Tool '{}' is not permitted for this agent.", pending_call.tool_name())
        ));
    }
}
```

`GhostConfig` gains `tool_policy: Option<ToolPolicy>`, merged from agent config at spawn time.

### G3.3 — Merge with `GhostPolicy`

`ToolPolicy` (agent-level) and `GhostPolicy` (runbook-level, existing) are both applied. They are independent checks — both must pass. `GhostPolicy` governs sudo and script whitelisting; `ToolPolicy` governs which tool names are callable. Neither overrides the other.

### G3.4 — System prompt annotation

When a ghost shell has a non-default `ToolPolicy`, the system prompt includes a section:

```
## Tool Restrictions
This session has a restricted tool policy. The following tools are available: [list].
Attempting to call any other tool will result in a denial.
```

This is informational — the model learns not to try denied tools. The enforcement remains in the daemon regardless of what the model knows.

**Tests (G3):**
- `policy::allows_unlisted_when_deny_mode` — deny list of 2 tools; a third tool is permitted.
- `policy::denies_listed_when_allow_mode` — allow list of 2 tools; a third tool is denied.
- `policy::validation_rejects_both_set` — config with both `allow` and `deny` fails validation.
- `executor::tool_policy_deny_returns_error` — mock executor call against a denied tool returns the structured error string without executing the tool.

---

## Phase G4 — Persistent Briefing State (2–3 days)

### Goal
After each clean ghost shell exit, the agent writes a rolling briefing summary. That briefing is injected into the next invocation as prior context.

### G4.1 — Briefing generation

In `src/daemon/ghost.rs`, after the ghost turn loop exits cleanly (all turns complete, no error, not force-killed):

```rust
if config.memory_namespace.is_some() {
    generate_and_save_briefing(&agent_name, &messages, &ai_client, &config).await;
}
```

`generate_and_save_briefing()`:
1. Builds a compact prompt: system prompt = "Summarize this session in ≤ 500 tokens for the next invocation of this agent. Include: key findings, actions taken, open questions, and what the next run should prioritize."; user turn = last N messages as a flattened transcript.
2. Calls the agent's model with `use_tools: false`, `max_tokens: 600`.
3. Runs the masking filter on the result.
4. Writes to `~/.daemoneye/agents/<name>/briefing.md` (overwrites).
5. Logs a `briefing_written` event to `events.jsonl`.

The briefing generation call is best-effort — failure is logged but does not affect the ghost shell's exit code or status reporting.

### G4.2 — Briefing injection

In `src/daemon/prompt.rs` (or `src/daemon/ghost.rs` in the first-turn prompt builder): when a ghost shell has `memory_namespace = Some(agent_name)` and `~/.daemoneye/agents/<agent_name>/briefing.md` exists:

```
## Previous Session Summary
<contents of briefing.md>
```

Injected after the main system prompt, before the runbook task description.

### G4.3 — CLI access

`daemoneye agent briefing <name>` — print the briefing to stdout.
`daemoneye agent briefing <name> --clear` — delete the briefing file.

This is a read-only view; operators can edit the briefing by opening the file directly. Document that path.

### G4.4 — IPC surface (optional, Phase G4 extension)

`Request::GetAgentBriefing { name }` / `Response::AgentBriefing { name, content: Option<String> }` — allows the interactive AI to query a briefing via `read_agent` tool (extend the tool to include briefing content). Not strictly required for the core feature but completes the introspection story.

**Tests (G4):**
- `ghost::briefing::writes_on_clean_exit` — mock ghost run exits cleanly; assert briefing file written.
- `ghost::briefing::skips_on_error_exit` — mock ghost run exits with error; assert briefing file absent.
- `ghost::briefing::injects_on_next_run` — briefing file present; assert prompt contains `## Previous Session Summary` block.
- `ghost::briefing::masking_applied` — synthetic model output containing a mock secret; assert briefing file has it masked.

---

## Phase G5 — Agent-to-Agent Delegation (4–6 days)

### Goal
A coordinator ghost shell can spawn specialist agent ghost shells, wait for their results, and synthesize them. Parent/child accounting is tracked. Depth is capped at 2.

### G5.1 — Spawn depth tracking

`GhostConfig` gains `spawn_depth: u8` (default 0). When a ghost shell spawns a sub-ghost via `spawn_ghost_shell`, the child's `spawn_depth = parent.spawn_depth + 1`. If `spawn_depth >= 2`, the tool call returns an error: `"Delegation depth limit reached (max: coordinator + 1 level of specialists)."` Enforcement is in `executor/mod.rs` before `ToolCallOutcome::SpawnGhostSession`.

### G5.2 — Mailbox result passing

**New file: `src/agents/mailbox.rs`**

Path: `~/.daemoneye/agents/<agent_name>/mailbox/<job_id>.json`

```rust
pub struct MailboxResult {
    pub job_id: String,
    pub agent: String,
    pub task: String,
    pub status: MailboxStatus,  // Pending | Complete | Failed
    pub result: Option<String>, // final AI response text
    pub error: Option<String>,
    pub completed_at: Option<u64>,
}
```

When a ghost shell completes (clean or error), it writes its final response to its mailbox file before exit.

### G5.3 — `await_agent_result` AI tool

New tool (silent, no approval gate):

```
await_agent_result(job_id: str, timeout_secs: int = 300) -> MailboxResult
```

Polls `~/.daemoneye/agents/<agent_name>/mailbox/<job_id>.json` for `status != Pending`. Returns the full result when complete or a timeout error. Uses `tokio::time::timeout` wrapping a `sleep(2s)` poll loop. The coordinator ghost shell calls this after spawning specialists.

Add `PendingCall::AwaitAgentResult`, `AiEvent::AwaitAgentResult`, tool definition, and executor arm per the CLAUDE.md checklist.

`should_emit_tool_feedback()` returns `true` (silent tool — shows spinner with elapsed time while waiting).

### G5.4 — Capacity accounting

`check_ghost_capacity()` in `src/daemon/ghost.rs` counts active ghosts regardless of parent/child relationship. A coordinator + 3 specialists consume 4 slots. The concurrency cap (`max_concurrent_ghosts`, default 3) applies to the total. This is intentional: coordinator patterns should be designed with the cap in mind, or the cap should be raised for teams using them.

Add `spawn_depth` and `parent_job_id: Option<String>` to the ghost lifecycle events written to `events.jsonl` so the relationship is visible in audit logs and the future replay CLI.

### G5.5 — Job ID surface

`spawn_ghost_shell` tool response gains a `job_id` field in its result JSON. The coordinator uses this job ID in subsequent `await_agent_result` calls.

The `GhostManager::start_session()` function returns a `job_id` (currently a tmux pane ID; can use that or generate a UUID). Surface this through the tool result.

**Tests (G5):**
- `agents::mailbox::write_and_read_result` — write a `MailboxResult`, read it back, assert equality.
- `agents::mailbox::status_transitions` — assert status can advance from Pending → Complete but not regress.
- `executor::depth_limit_enforced` — mock ghost at depth 2 attempts `spawn_ghost_shell`; assert tool error returned.
- `executor::depth_0_allows_spawn` — mock ghost at depth 0; assert spawn proceeds normally.

---

## Phase G6 — Polish & Integration (2–3 days)

### Goal
Connect all phases, surface agents in `daemoneye status`, add to the sre.toml prompt, update CLAUDE.md and ARCHITECTURE.md.

### G6.1 — `daemoneye status` integration

`DaemonStatus` (IPC) gains `active_agents: Vec<(String, String)>` — each tuple is `(agent_name, currently_running_job_id_or_idle)`. Shown in `daemoneye status` output under a new `Agents` section.

### G6.2 — Agent listing in the interactive AI context

When the daemon starts, it loads the agent registry. The `get_terminal_context` tool response (or the system prompt) includes an `## Available Agents` block (analogous to how the system prompt today documents available runbooks). The AI knows what agents exist without needing to call `list_agents` first.

### G6.3 — sre.toml agent documentation section

Add a `## Named Agents` section to `assets/prompts/sre.toml` documenting:
- Agent × runbook composability
- When to use `agent:` in a runbook vs. inline prompt
- When to call `spawn_ghost_shell` with `agent:` vs. without
- The coordinator pattern and depth limit
- How briefing state works

### G6.4 — Memory migration for existing entries

Run `migrate_namespace()` at daemon startup (same path as the G2 schema migration). No existing behavior changes — all entries acquire `namespace: global`.

### G6.5 — CLAUDE.md updates

- Add `src/agents/` to the key files table.
- Extend "Adding a new AI tool" checklist to note the agent tool dispatch path.
- Document `spawn_depth` and `mailbox` conventions.
- Update `GhostConfig` field list.

### G6.6 — Integration tests

- `integration::agent_config_roundtrip` — save and load agent config via production types.
- `integration::agent_memory_namespace_isolation` — write to agent ns; confirm invisible from global-only read.
- `integration::tool_policy_enforced_in_ghost` — ghost spawned with deny-list policy; tool call returns denial without executing.

---

## Phase G7 — Stretch Goals (future)

These are worth designing for but not blocking on initial Named Agents delivery.

### G7.1 — Agent registry as a shareable artifact

An agent config is a TOML file under `~/.daemoneye/agents/`. When git-backed knowledge sync (ROADMAP.md I9) lands, agent configs can be committed alongside runbooks and shared across a team. A team's "analyst" agent becomes a shared, version-controlled resource.

### G7.2 — Business-hours policy per agent

`AgentConfig` gains `enabled_during: Option<String>` (mirrors ROADMAP.md I6). When an agent is scheduled outside its enabled window, ghost spawn is declined with a log entry rather than running with degraded policy.

### G7.3 — Agent performance metrics

Extend `daemoneye costs` (ROADMAP.md R2) to break down token usage and cost by agent name. A `disk-monitor` agent running Haiku vs. an `incident-responder` running Opus should be separately visible in cost reporting.

### G7.4 — Second-pair-of-eyes for agents (ROADMAP.md I7 complement)

An agent with `requires_review: true` posts a notification before spawning. The human approves or denies via a reply to the tmux overlay message, a Slack button, or a `daemoneye agent approve <job_id>` CLI command. The ghost shell is held in `Pending` state until the decision arrives or the timeout expires.

---

## File Change Summary

| File | Change |
|---|---|
| `src/agents/mod.rs` | **New** — `AgentConfig`, CRUD, validation |
| `src/agents/policy.rs` | **New** — `ToolPolicy`, `permits()` |
| `src/agents/mailbox.rs` | **New** (G5) — `MailboxResult`, read/write helpers |
| `src/ai/tools.rs` | Add `create_agent`, `read_agent`, `list_agents`, `delete_agent`, `await_agent_result` tool defs |
| `src/ai/types.rs` | Add corresponding `PendingCall` and `AiEvent` variants |
| `src/config.rs` | `GhostConfig` gains `memory_namespace`, `tool_policy`, `spawn_depth`, `parent_job_id` |
| `src/daemon/executor/knowledge.rs` | Add agent tool dispatch; namespace threading |
| `src/daemon/executor/mod.rs` | Add `ToolPolicy` check; depth limit check; job_id surface |
| `src/daemon/ghost.rs` | Briefing generation/injection; mailbox write on exit |
| `src/daemon/prompt.rs` | Briefing injection into first-turn prompt |
| `src/daemon/server.rs` | `GetAgentBriefing` IPC handler; `available_agents` in status |
| `src/ipc.rs` | `Request::GetAgentBriefing`; `Response::AgentBriefing`; `DaemonStatus.active_agents` |
| `src/memory/mod.rs` | `namespace` field; namespace-aware CRUD signatures |
| `src/memory/index.rs` | `namespace` column; filtered queries |
| `src/memory/migrate.rs` | `migrate_namespace()` |
| `src/runbook.rs` | `agent: Option<String>` frontmatter field |
| `src/main.rs` | `daemoneye agent` subcommand routing |
| `src/cli/commands.rs` | `run_agent_*` CLI handlers |
| `assets/prompts/sre.toml` | `## Named Agents` section |
| `CLAUDE.md` | `src/agents/` in key files; updated `GhostConfig` fields; agent tool checklist note |

---

## Exit Criteria by Phase

| Phase | Definition of Done |
|---|---|
| G1 | `AgentConfig` round-trips to disk; `daemoneye agent list/show` works; `spawn_ghost_shell agent:` merges config; runbook `agent:` field loads and merges |
| G2 | Agent ghost shell writes to agent namespace; interactive session reads only global; FTS5 index filters by namespace; existing entries migrated |
| G3 | Denied tool call returns structured error without executing; `cargo test` passes including new policy tests; system prompt annotates restrictions |
| G4 | Briefing written after clean ghost exit; injected on next invocation; masked before write; `daemoneye agent briefing` shows content |
| G5 | Coordinator spawns specialist; specialist writes mailbox; coordinator reads result via `await_agent_result`; depth-2 spawn returns error |
| G6 | `daemoneye status` shows active agents; sre.toml documented; CLAUDE.md updated; integration tests pass; `cargo clippy --all-targets -- -D warnings` exits zero |
