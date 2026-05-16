# Named Agents: Product Design

*Drafted 2026-05-15. Companion implementation plan: `NAMED_AGENTS_PLAN.md`.*

---

## The Problem

Ghost shells are DaemonEye's autonomous execution layer, but every ghost shell is born the same: blank identity, global memory pool, full toolset, default model. There is no concept of *who* is running the task — only *what* the task is.

This creates friction at scale. A security auditing task and a database remediation task may both run as ghost shells, but they require fundamentally different system prompts, different tool access, different knowledge domains, and different AI models. Today the only way to express that difference is to pack it all into the runbook — which conflates *task specification* (what to do) with *executor specification* (who does it, with what authority and context).

The result: runbooks become long, overly specific, and hard to share. Knowledge that should accumulate in a specialist's memory pool bleeds into the global pool instead. The best-fit model for a given domain has to be redeclared in every runbook that touches that domain.

Named Agents close this gap.

---

## What Named Agents Are

A Named Agent is a persistent, reusable executor identity. Where a runbook defines *what* to do, an agent defines *who does it* — the role, the knowledge, the tools, the model, and the memory that persist across every task that executor handles.

**Key properties:**

- **Custom system prompt** — a role definition, domain expertise, behavioral constraints, and personality layered on top of (or replacing) the default SRE prompt. An agent can be a skeptical security auditor, a careful database operator, a read-only analyst that never touches production, or a coordinator that delegates to specialists.
- **Dedicated model** — the best LLM for the role. Use Opus for complex incident response, Haiku for high-frequency watchdog checks, a local Ollama model for tasks that must not leave the host.
- **Memory namespace** — each agent maintains its own scoped memory pool. When a security agent calls `read_memory`, it searches its own namespace first, then falls back to global. Over time, agents accumulate domain-specific knowledge that does not pollute the shared pool.
- **Tool policy** — an allowlist or denylist of AI tools. A read-only analyst can be given `read_file`, `read_memory`, and `search_repository` and denied `run_terminal_command` and `edit_file` entirely. Tool policy is enforced by the daemon, not stated in the prompt.
- **Persistent briefing state** — after each invocation, the agent writes a brief summary of what it learned and what state it left things in. On its next invocation, that briefing is injected as context. Agents build continuity across separate tasks and restarts.

**What agents are not:**

Agents are not long-running processes. An agent does not sit in memory between invocations waiting for work. An agent is a configuration profile that shapes how a ghost shell is instantiated. When work arrives, a ghost shell is spawned from that profile; when the work is done, the ghost shell exits. The agent's memory and briefing state survive; the process does not.

---

## The Runbook × Agent Model

Runbooks and agents are orthogonal. Their combination is more powerful than either alone.

| | No Agent | Named Agent |
|---|---|---|
| **No Runbook** | Default ghost shell (today) | Unstructured task with specialist identity |
| **Runbook** | Task-guided ghost (today) | Task-guided specialist (the common case) |

A runbook specifies `agent: analyst` in its frontmatter. When a ghost shell spawns for that runbook, it inherits the analyst agent's prompt, model, tool policy, and memory namespace. The runbook focuses on the task; the agent focuses on the executor. Each can be updated independently: a new version of the analyst agent benefits all runbooks that reference it, without touching any of them.

**Composability rules:**

1. When a runbook specifies an agent, the agent's system prompt takes precedence. The runbook's task description is injected into the conversation as the first user turn, not prepended to the system prompt.
2. When a runbook specifies a model and the agent also specifies a model, the runbook-level model wins. Runbook specificity beats agent default.
3. Tool policy is the intersection: if the agent denies `run_terminal_command` and the runbook has no opinion, `run_terminal_command` is denied.
4. Memory queries search: agent namespace → global namespace, in that order.

---

## Agent-to-Agent Delegation

Named agents enable a coordinator pattern. A coordinator agent is given a high-level task and the ability to spawn specialist agents to handle sub-tasks. The coordinator synthesizes results and reports back.

**Example:** A `security-review` agent is triggered by a webhook. It spawns two sub-agents: an `access-auditor` that checks IAM policies and a `log-analyst` that searches for anomalous patterns. Each runs in its own ghost shell window with its own memory namespace and model. When both complete, the coordinator reads their results and writes a consolidated incident record.

**Mechanics:**

The existing `spawn_ghost_shell` AI tool gains an `agent` parameter. When a ghost shell is spawned with an agent, the spawning ghost is the parent; capacity accounting includes the parent's slot. The child ghost writes its result to a structured output file (a "mailbox" at `~/.daemoneye/agents/<name>/mailbox/<job_id>.json`); the parent polls for completion and reads the result. Neither parent nor child needs to share memory mid-flight — communication is via the mailbox and the shared memory store.

**Depth limit:** ghost-shell spawn depth is capped at 2 (coordinator + specialists). A specialist cannot spawn further sub-agents. This prevents unbounded recursion while enabling the most common patterns.

---

## Persistent Briefing State

Stateless ghost shells waste context and time re-discovering what prior invocations already learned. A `security-auditor` agent that found three hosts with misconfigured SSH on its last run should not start from zero on its next daily check.

After each ghost shell exits, the daemon asks the model for a short briefing (≤ 500 tokens): what was found, what was done, what remains, and what the next invocation should pay attention to. This briefing is written to `~/.daemoneye/agents/<name>/briefing.md`. On the next invocation of any ghost shell that uses that agent, the briefing is injected as a `[Previous Session]` block in the system prompt.

**Lifecycle:**
- Briefing is generated only when the ghost shell exits cleanly (turn limit reached without error does not count as clean).
- Each invocation overwrites the prior briefing — this is a rolling summary, not a log.
- Briefings are subject to the same masking filter as all other AI-generated content before write.
- The operator can read, edit, or delete a briefing via `daemoneye agent briefing <name>`.

---

## Memory Namespacing

Every agent has a memory namespace equal to its name (configurable). Memory operations within an agent-scoped ghost shell are namespace-aware:

- `add_memory` — writes to the agent namespace unless `namespace: global` is specified.
- `read_memory` / `list_memories` — searches agent namespace first, then global. Returns the namespace of each result.
- `search_repository` — includes agent namespace in the search scope.
- `delete_memory` — operates on the namespace the memory lives in.

Global memories are readable by all agents. Agent-scoped memories are readable by the owning agent and by any agent explicitly granted access via `read_namespaces` in its config. The main interactive session always reads from global namespace only (agents are for autonomous work; the interactive session is the operator's domain).

---

## Tool Policy

Each agent declares a `tools` policy block:

```toml
[tools]
allow = ["read_file", "read_memory", "list_memories", "search_repository", "get_terminal_context"]
# deny is the inverse; use allow or deny, not both
```

If `allow` is specified, only those tools are available. If `deny` is specified, all tools except those listed are available. If neither is specified, the agent inherits the default ghost shell tool policy (which is governed by `GhostPolicy`).

Tool policy is enforced by the daemon in `execute_tool_call()`, not declared in the system prompt. The model cannot talk its way out of a tool denial — the IPC layer rejects the call and returns a structured error that the model sees as a tool result.

---

## Security Properties

Named agents extend, not weaken, DaemonEye's security model.

**Tool policy is additive restriction, never expansion.** An agent can only restrict the toolset available to a ghost shell, not expand it beyond what `GhostPolicy` allows. If a runbook's `auto_approve_scripts` lists three scripts, an agent cannot add a fourth.

**Memory namespacing is soft isolation, not a security boundary.** Agent memories are readable by the operator and by the daemon. Namespace separation is a signal clarity and search efficiency feature, not access control.

**Depth cap is a hard limit.** The daemon enforces the coordinator + specialists depth limit in `check_ghost_capacity()`. An agent cannot override it from its config or prompt.

**Agent configs cannot be written by AI tools.** The `create_agent` / `delete_agent` tools exist for convenience during setup, but they require the same user approval as `edit_file`. An agent cannot modify its own config or another agent's config.

**Briefings are masked.** Before writing a briefing to disk, the daemon runs the masking filter. A model cannot launder a secret through a briefing file.

---

## Key Workflows

### Workflow A: Specialist Agents for Routine Watchdogs

A `disk-monitor` agent is defined with a conservative read-only tool policy and a compact system prompt focused on filesystem analysis. Twelve runbooks that previously had redundant disk-check instructions are updated to `agent: disk-monitor`. Each inherits the agent's prompt, model (Haiku for cost efficiency), and memory namespace. The agent accumulates disk history over weeks; recurring patterns ("this host always spikes on Mondays") surface automatically via its briefing state.

### Workflow B: The Coordinator Pattern

A high-severity security alert fires. The `security-coordinator` agent spawns three specialists: `access-auditor`, `network-scanner`, and `log-analyst`. Each runs in parallel (up to the concurrency cap) in separate ghost shell windows. When all three complete, the coordinator reads their mailbox outputs, synthesizes findings, writes an incident record, and composes a PagerDuty comment. Total elapsed time is the longest of the three specialists, not their sum.

### Workflow C: Graduated Trust via Agent Profiles

A new team member defines a `trainee-agent` with a conservative tool policy: no `run_terminal_command`, no `edit_file`, no `write_script`. Runbooks are assigned to this agent for the first month. Approvals are reviewed in daily standups. After four weeks, the tool policy is loosened one tool at a time as specific capabilities are validated. The agent profile becomes a trust ledger — its evolution tells the story of capabilities earned.

### Workflow D: Domain-Expert Memory Accumulation

A `postgres-dba` agent runs database health runbooks for six months. Its memory namespace contains: non-standard port mappings for every host, known slow-query patterns and their fixes, the incident record of the 2026-03 replication lag event, and a runbook section added after a near-miss vacuum failure. When a new database incident fires, the agent's briefing injects this six months of operational history before the first AI turn. The model does not start from zero.

---

## Success Metrics

- **Agent reuse rate**: number of runbooks referencing a named agent vs. embedding a redundant system prompt inline.
- **Memory namespace hit rate**: fraction of `read_memory` calls returning agent-namespace results (vs. global fallback only).
- **Model cost reduction**: cost delta for watchdog workloads after pinning them to smaller models via agent config.
- **Briefing continuity utilization**: fraction of ghost shell invocations where a non-empty prior briefing was injected (signals agents are accumulating useful state).
- **Coordinator pattern adoption**: count of multi-agent invocations where a coordinator spawned ≥ 1 specialist.
