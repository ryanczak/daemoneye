# DaemonEye Knowledge System — Design Document

*Drafted: March 2026*

---

## 1. Overview

This document describes the design for a persistent knowledge management layer in DaemonEye. It adds three inter-related capabilities:

| Feature | Summary |
|---------|---------|
| **Runbook CRUD** | Full create/read/update/delete for markdown runbooks via AI tools |
| **Persistent Memory** | Key/value knowledge store in `~/.daemoneye/memory/` with selective context loading |
| **Repository Search** | Keyword search across runbooks, scripts, memory, and the event log |

Together, these form a self-improving knowledge loop: the AI accumulates facts from live experience (memory), codifies them into structured procedures (runbooks), and can rediscover both via search. Over time the AI becomes progressively more context-aware about the user's specific environment.

---

## 2. Runbook System

### 2.1 Format

Runbooks are stored as markdown files in `~/.daemoneye/runbooks/<name>.md`. A YAML frontmatter block carries structured metadata. The rest of the file is free-form markdown.

**Standard format:**

```markdown
---
tags: [nginx, infrastructure, production]
memories: [nginx_config, k8s_prod_cluster]
---

# Runbook: <name>

## Purpose
What this runbook is for and when it is used. One paragraph.

## Alert Criteria
Conditions that should trigger an alert when this runbook is used in watchdog mode.
- Condition 1
- Condition 2

## Remediation Steps
Ordered steps the AI should consider when an alert condition is met.
1. Step one
2. Step two

## Notes
Environment-specific knowledge the AI has accumulated about this context.
This section should be updated by the AI as it learns.
```

**Frontmatter fields:**

| Field | Type | Description |
|-------|------|-------------|
| `tags` | list of strings | Keywords for search filtering and discovery |
| `memories` | list of strings | Memory keys to load into context when this runbook is used |

**Required sections:** `# Runbook: <name>`, `## Purpose`, `## Alert Criteria`. All other sections are optional but recommended.

**Naming:** File name is the runbook's canonical key. `nginx-watchdog.md` → key `nginx-watchdog`. Lowercase, hyphens, no path separators or special characters.

### 2.2 AI Tools

| Tool | Description | Approval Gate |
|------|-------------|---------------|
| `create_runbook(name, content)` | Write a new runbook | `RunbookWritePrompt` — shows full content |
| `update_runbook(name, content)` | Overwrite an existing runbook | `RunbookWritePrompt` — shows diff or full content |
| `delete_runbook(name)` | Delete a runbook | `RunbookDeletePrompt` — warns if active scheduled jobs reference it |
| `read_runbook(name)` | Read a runbook's full content | None |
| `list_runbooks()` | List runbook names + tags | None |

### 2.3 Approval Gates

`RunbookWritePrompt` and `RunbookDeletePrompt` follow the same IPC pattern as `ScriptWritePrompt`:
- Daemon sends the prompt response over the socket
- Client displays content and prompts `[Y]es / [N]o`
- Client returns `RunbookWriteResponse { id, approved }` or `RunbookDeleteResponse { id, approved }`

`delete_runbook` should additionally check `ScheduleStore` for jobs with `runbook == name` and include a warning in the prompt if any are found.

### 2.4 Backward Compatibility

Existing `.toml` runbooks are loaded by `runbook.rs` today. On first startup after this change, the daemon will emit a warning for any `.toml` files found in `~/.daemoneye/runbooks/` and skip them. A `daemoneye migrate runbooks` subcommand (or in-chat `/migrate runbooks`) can convert them. In the interim, the watchdog AI tool docs should note that only `.md` runbooks are supported.

### 2.5 Watchdog Integration

When `watchdog_system_prompt()` builds its context for a scheduled watchdog job, it:
1. Loads the referenced runbook's markdown content
2. Parses the frontmatter `memories:` list
3. Fetches each listed key from `~/.daemoneye/memory/knowledge/`
4. Appends the loaded memories as a `[RUNBOOK MEMORY]` context block below the runbook

This makes memory loading automatic and scoped — watchdog jobs only pull in the knowledge that's relevant to their runbook.

### 2.6 Format Validation

On `create_runbook` and `update_runbook`, the daemon validates:
- Presence of `# Runbook:` header
- Presence of `## Alert Criteria` section
- No path traversal in `name` (no `/`, `\0`, `..`)
- File extension `.md` (enforced by the daemon, not the AI)

Validation failures are returned as a `ToolResult` error so the AI can correct and retry.

---

## 3. Persistent Memory System

### 3.1 Memory Taxonomy

Three categories with distinct loading behaviour:

```
~/.daemoneye/memory/
├── session/       # Loaded at the start of every chat session
├── knowledge/     # On-demand: loaded when referenced by a runbook or explicit tool call
└── incidents/     # Historical record: searchable but never auto-loaded
```

**`session/`** — User-facing context that should always be in scope:
- User preferences ("prefers terse explanations", "always use systemd not init.d")
- Recurring environment notes ("this host is always behind a proxy")
- Interaction patterns learned over time

**`knowledge/`** — Technical facts about specific systems, services, or configurations:
- "nginx on prod uses a non-standard config at /opt/nginx/nginx.conf"
- "the k8s cluster uses a custom CNI; kubectl exec into pods requires --tty"
- Tables of service names, ports, hostnames

**`incidents/`** — Historical incident records, auto-named with a timestamp prefix:
- "2026-02-15-nginx-oom: root cause was worker_connections set too low; fix was..."
- Never auto-loaded; searchable via `search_repository`; can be referenced in runbooks for historical context

### 3.2 Memory Format

Each memory entry is a single markdown file. The file name is the key. The content is free-form markdown — not just a single string — so the AI can store structured knowledge (tables, code blocks, multi-step reasoning).

```
~/.daemoneye/memory/session/user_prefs.md
~/.daemoneye/memory/knowledge/nginx_config.md
~/.daemoneye/memory/incidents/2026-02-15-nginx-oom.md
```

### 3.3 AI Tools

| Tool | Description | Approval Gate |
|------|-------------|---------------|
| `add_memory(key, value, category)` | Create or overwrite a memory entry (upsert) | None — AI's own knowledge store |
| `delete_memory(key, category)` | Remove a memory entry | None |
| `read_memory(key, category)` | Fetch a specific memory entry by key | None |
| `list_memories(category?)` | List all memory keys, optionally filtered by category | None |

`add_memory` is an upsert — calling it with an existing key overwrites the entry. This prevents stale knowledge from accumulating under duplicate keys.

No approval gate on memory writes. Memory is the AI's own internal knowledge store (analogous to a human taking notes). Unlike scripts and runbooks, memory entries are never directly executed.

**Category values:** `"session"`, `"knowledge"`, `"incident"`.

For incident memories, the daemon enforces the timestamp prefix convention (`YYYY-MM-DD-`) in the key, appending it automatically if absent.

### 3.4 Context Loading

**Session start:** When `server.rs` processes the first `Ask` request of a new session, it reads all `.md` files from `~/.daemoneye/memory/session/` and prepends them as a `[MEMORY]` context block:

```
[MEMORY]
--- user_prefs ---
<content>

--- environment ---
<content>
```

**Context budget:** Session memories are capped at 8,000 tokens (~32 KB total file size). If the total exceeds the cap, the daemon loads the most recently modified entries first and appends a note: `[N session memories omitted — use list_memories to find them]`. This prevents an ever-growing memory set from silently degrading performance.

**On-demand (runbook-triggered):** When a watchdog job loads a runbook (see §2.5), the `memories:` frontmatter list is resolved and those files are loaded from `~/.daemoneye/memory/knowledge/` and appended to the watchdog prompt.

**On-demand (explicit):** The AI can call `read_memory(key, category)` at any point during a conversation to pull in a specific entry. Useful when the AI identifies that a knowledge memory is relevant mid-conversation.

### 3.5 Masking Filter

Memory content passes through the standard masking filter (`ai/filter.rs`) before being included in the AI's context block. This handles the case where the AI wrote a memory containing text captured from terminal output that happened to include a credential.

---

## 4. Repository Search

### 4.1 Tool Definition

A single unified tool:

```
search_repository(query, kind, context_lines?)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `query` | string | Keyword or regex pattern to search for |
| `kind` | enum | `"runbooks"`, `"scripts"`, `"memory"`, `"events"`, or `"all"` |
| `context_lines` | int (optional) | Lines of context around each match (default: 2) |

Returns: a structured list of matches — file name, line number, matched line, and surrounding context. Equivalent to `grep -rn -C <context_lines>`.

`kind: "all"` searches runbooks, scripts, and memory simultaneously (not events — that's a heavier operation, see §4.3).

### 4.2 Search Behaviour

- **Case-insensitive by default.** The pattern is treated as a case-insensitive literal string unless it contains regex metacharacters, in which case it is used as a regex directly.
- **File name search included.** A query matching a file name (without extension) is also returned, even if the content doesn't match.
- **Results capped at 50 matches** to keep the tool result manageable. If results are truncated, the tool result notes how many were omitted.
- **Memory categories searched together** when `kind: "memory"` — results are prefixed with their category (`session/`, `knowledge/`, `incidents/`).

### 4.3 Event Log Search (`kind: "events"`)

Searches `~/.daemoneye/events.jsonl` for historical data. Because the event log is append-only and can grow large, this search is:

- **Bounded to the last 10,000 lines** by default (configurable via `context_lines`-overload in future).
- **JSON-field aware**: the query matches against the full JSON line as a string, but the result is returned as formatted key→value pairs for readability rather than raw JSON.
- **Priority: non-critical feature.** Implement after the runbook/memory/search core is stable.

**Example use cases for event log search:**
- "Have I run `nginx -t` on this host before?" → find past executions
- "When did the last watchdog alert fire for the nginx job?" → find alert events
- "What commands did the AI execute last Tuesday?" → incident retrospective

---

## 5. System Prompt Updates

`sre.toml` (and its compiled-in copy in `config.rs`) must document the new tools so the AI uses them consistently. Key additions:

### Memory discipline
- Write a `session` memory when you learn a durable user preference or recurring environment fact.
- Write a `knowledge` memory when you discover something specific about a named service, host, or configuration that will be useful in the future.
- Write an `incident` memory when resolving a significant issue, documenting root cause, symptoms, and fix.
- Before writing a new memory, call `list_memories` to check whether an existing entry should be updated instead.
- Keep `session` memories brief — they are loaded on every turn.

### Runbook discipline
- After resolving an alert using a runbook, update the `## Notes` section with what you learned.
- When creating a runbook for a new watchdog job, populate the `memories:` frontmatter with any relevant knowledge memory keys.
- Use `list_runbooks` and `search_repository` before creating a new runbook to avoid duplicates.

### Search discipline
- Use `search_repository` before listing or reading files when looking for something specific.
- Prefer `kind: "all"` for open-ended discovery; use a specific kind when the target is known.

---

## 6. Implementation Order

| # | What | Risk | Notes |
|---|------|------|-------|
| 1 | IPC: `RunbookWritePrompt` / `RunbookDeletePrompt` / response types | Low | Same pattern as `ScriptWritePrompt` |
| 2 | `runbook.rs`: markdown loader, frontmatter parser, CRUD operations | Low | Replace TOML loader; add validation |
| 3 | AI tools: `create_runbook`, `update_runbook`, `delete_runbook`, `read_runbook`, `list_runbooks` | Low | 3 providers × 5 tools |
| 4 | `server.rs` / `executor.rs`: approval gate wiring for runbook tools | Low | Same pattern as script tools |
| 5 | `memory.rs`: new module — `add_memory`, `delete_memory`, `read_memory`, `list_memories` | Low | Simple file I/O |
| 6 | AI tools: 4 memory tools (3 providers) | Low | |
| 7 | `server.rs`: session memory loading at turn 1 (`[MEMORY]` context block) | Medium | Context budget cap logic |
| 8 | `runbook.rs`: on-demand memory loading from frontmatter in `watchdog_system_prompt()` | Medium | Requires #2 and #5 |
| 9 | `search.rs`: new module — `search_repository()` (runbooks + scripts + memory) | Low | `grep`-style file walk |
| 10 | AI tool: `search_repository` (3 providers) | Low | |
| 11 | `server.rs` / `executor.rs`: search tool wiring | Low | |
| 12 | `sre.toml` + `config.rs`: document all new tools and behavioural discipline | Low | Must stay in sync |
| 13 | Event log search (`kind: "events"`) | Low | Non-critical; add after core is stable |

---

## 7. File System Layout (Complete Picture)

```
~/.daemoneye/
├── config.toml
├── daemon.log
├── events.jsonl
├── schedules.json
├── prompts/
│   └── sre.toml
├── runbooks/          # markdown only (.md)
│   ├── nginx-watchdog.md
│   └── disk-usage.md
├── scripts/           # executable (chmod 700)
│   └── check-disk.sh
└── memory/
    ├── session/       # always loaded at session start
    │   ├── user_prefs.md
    │   └── environment.md
    ├── knowledge/     # on-demand (referenced by runbooks or explicit read)
    │   ├── nginx_config.md
    │   └── k8s_prod.md
    └── incidents/     # historical record (searchable only)
        └── 2026-02-15-nginx-oom.md
```

---

## 8. Open Questions

| # | Question | Recommendation |
|---|----------|----------------|
| Q1 | Should `add_memory` have an approval gate? | No — memory is the AI's own notes. No approval needed. |
| Q2 | Should session memories be truncated or summarised when over the cap? | Truncate (drop oldest by mtime), append a note. Summarisation adds complexity. |
| Q3 | Should incident memories have an auto-summary in the `[MEMORY]` block? | No for now. Load on-demand only. |
| Q4 | Should the event log search be bounded by date range rather than line count? | Date range is more intuitive but harder to implement; line-count cap is sufficient for v1. |
| Q5 | Should `delete_runbook` be hard or soft delete? | Hard delete is simplest. A future `daemoneye runbooks restore <name>` could read from a `.trash/` subfolder. |
| Q6 | How should the AI discover that incident memories exist if they're never auto-loaded? | Via `search_repository(kind: "memory")` or `list_memories(category: "incident")`. The system prompt should remind the AI to search incidents when starting a troubleshooting session. |
