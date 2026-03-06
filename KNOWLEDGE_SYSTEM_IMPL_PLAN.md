# Knowledge System — Implementation Plan

*Cross-reference: `KNOWLEDGE_SYSTEM_DESIGN.md` for design rationale and open questions.*

---

## Scope

9 new AI tools across 3 providers, 2 new Rust modules, rewritten runbook module, no new Cargo dependencies.

| Feature | New tools |
|---------|-----------|
| Runbook CRUD | `write_runbook`, `delete_runbook`, `read_runbook`, `list_runbooks` |
| Persistent Memory | `add_memory`, `delete_memory`, `read_memory`, `list_memories` |
| Repository Search | `search_repository` |

---

## Critical Files

| File | What changes |
|------|-------------|
| `src/memory.rs` | **New module** — CRUD ops, session loading, 32 KB context-budget cap |
| `src/search.rs` | **New module** — grep-style search across runbooks/scripts/memory/events |
| `src/runbook.rs` | **Rewrite** — markdown loader, frontmatter parser, CRUD, memory loading in watchdog prompt |
| `src/ipc.rs` | +2 Request variants, +4 Response variants, +2 list item structs |
| `src/ai/types.rs` | +9 PendingCall + 9 AiEvent variants; update `to_tool_call()` + `id()` |
| `src/ai/mod.rs` | +9 arms in `dispatch_tool_event()` |
| `src/ai/tools.rs` | +9 tool definitions × 3 providers (Anthropic / OpenAI / Gemini) |
| `src/daemon/server.rs` | +9 AiEvent arms in streaming loop; inject `[MEMORY]` into first-turn context |
| `src/daemon/executor.rs` | +9 PendingCall arms with approval gates where needed |
| `src/cli/commands.rs` | Handle `RunbookWritePrompt`, `RunbookDeletePrompt`, `RunbookList` responses |
| `src/main.rs` | `mod memory;` + `mod search;` |
| `sre.toml` + `src/config.rs` | Add tool discipline documentation (must stay in sync) |

---

## Step 1 — `src/memory.rs` (new module)

```rust
pub enum MemoryCategory { Session, Knowledge, Incident }

impl MemoryCategory {
    pub fn dir_name(&self) -> &'static str { /* "session" / "knowledge" / "incidents" */ }
    pub fn from_str(s: &str) -> Option<Self> { ... }
}

fn memory_dir(category: &MemoryCategory) -> PathBuf {
    crate::config::config_dir().join("memory").join(category.dir_name())
}

fn validate_memory_key(key: &str) -> Result<()> {
    // Reject: empty, '/', '\0', '.', '..' — same logic as validate_script_name
}

pub fn add_memory(key: &str, value: &str, category: MemoryCategory) -> Result<()> {
    // Incident keys get YYYY-MM-DD- prefix automatically if absent
}

pub fn delete_memory(key: &str, category: MemoryCategory) -> Result<()> { ... }
pub fn read_memory(key: &str, category: MemoryCategory) -> Result<String> { ... }

pub fn list_memories(category: Option<MemoryCategory>) -> Result<Vec<(String, String)>> {
    // Returns Vec<(category_prefix, key)> e.g. ("session", "user_prefs")
}

/// Load ~/.daemoneye/memory/session/*.md into a formatted block.
/// Applies mask_sensitive(). Caps at 32 KB (SESSION_MEMORY_CAP).
/// Returns "" when no session memories exist (no change to prompt format).
pub fn load_session_memory_block() -> String {
    // Format: "--- key ---\n<content>\n\n" per entry
    // If over cap: truncate + "[N session memories omitted — use list_memories to find them]"
    // Wrap: "## Persistent Memory\n```\n{}\n```\n\n" when non-empty
}
```

Add `mod memory;` to `src/main.rs`.

**Verify:** `cargo build`

---

## Step 2 — Rewrite `src/runbook.rs`

**New `Runbook` struct:**
```rust
pub struct Runbook {
    pub name: String,
    pub content: String,       // Markdown body after frontmatter
    pub tags: Vec<String>,     // From "tags: [a, b]" frontmatter
    pub memories: Vec<String>, // From "memories: [x, y]" frontmatter
}
pub struct RunbookInfo { pub name: String, pub tags: Vec<String> }
```

**Frontmatter parser** (manual — no new deps):
```rust
fn parse_frontmatter(raw: &str) -> (Vec<String>, Vec<String>, String) {
    // If starts with "---\n": find "\n---\n", parse tags/memories lines, return body
    // Otherwise: (vec![], vec![], raw.to_string())
}
// Parse "tags: [a, b, c]" → vec!["a", "b", "c"] (split on ',', strip whitespace/brackets)
```

**Validation on write:**
```rust
fn validate_runbook_content(content: &str) -> Result<()> {
    if !content.contains("# Runbook:") { bail!("Missing '# Runbook:' heading"); }
    if !content.contains("## Alert Criteria") { bail!("Missing '## Alert Criteria' section"); }
    Ok(())
}
```

**CRUD functions:**
```rust
pub fn load_runbook(name: &str) -> Result<Runbook>  // .md only; returns Err if not found
pub fn write_runbook(name: &str, content: &str) -> Result<()>  // upsert; validates format
pub fn delete_runbook(name: &str) -> Result<()>
pub fn list_runbooks() -> Result<Vec<RunbookInfo>>  // sorted by name
```

**Updated `watchdog_system_prompt()`:**
- After loading the runbook, resolve `runbook.memories` from `~/.daemoneye/memory/knowledge/`
- Append as `## Runbook Memory Context\n### key\n<content>` sections

**Backward compat:** TOML runbooks silently ignored (`.md` only). Existing `.toml` files stay on disk.

Update existing test `watchdog_prompt_contains_runbook_name` to use the new markdown format.

**Verify:** `cargo build && cargo test`

---

## Step 3 — `src/search.rs` (new module)

```rust
pub struct SearchResult {
    pub kind: String,       // "runbook", "script", "memory/session", etc.
    pub name: String,       // File stem (no extension)
    pub line_number: usize,
    pub matched_line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

pub fn search_repository(query: &str, kind: &str, context_lines: usize) -> Vec<SearchResult> {
    // kind: "runbooks" | "scripts" | "memory" | "events" | "all"
    // For "all": searches runbooks + scripts + memory (not events)
    // For "events": reads last 10,000 lines of ~/.daemoneye/events.jsonl
    //   Formats each JSON line as human-readable key=value pairs
    // Case-insensitive literal match; also matches file stem against query
    // Cap at 50 total results
}

pub fn format_results(results: &[SearchResult]) -> String {
    // "No matches found." if empty
    // Groups by file, shows line number + context
}
```

Add `mod search;` to `src/main.rs`.

**Verify:** `cargo build`

---

## Step 4 — `src/ipc.rs`: new variants

**Add to `Request`:**
```rust
RunbookWriteResponse { id: String, approved: bool },
RunbookDeleteResponse { id: String, approved: bool },
```

**Add to `Response`:**
```rust
RunbookWritePrompt  { id: String, runbook_name: String, content: String },
RunbookDeletePrompt { id: String, runbook_name: String, active_jobs: Vec<String> },
RunbookList  { runbooks: Vec<RunbookListItem> },
```

**Add structs:**
```rust
pub struct RunbookListItem { pub name: String, pub tags: Vec<String> }
```

**Note:** Memory operations return plain strings (no dedicated Response variants needed — same pattern as `ReadScript`).

**Verify:** `cargo build`

---

## Step 5 — `src/ai/types.rs` + `src/ai/mod.rs`: new variants

**9 new `PendingCall` variants:**
```rust
WriteRunbook     { id, thought_signature, name: String, content: String },
DeleteRunbook    { id, thought_signature, name: String },
ReadRunbook      { id, thought_signature, name: String },
ListRunbooks     { id, thought_signature },
AddMemory        { id, thought_signature, key: String, value: String, category: String },
DeleteMemory     { id, thought_signature, key: String, category: String },
ReadMemory       { id, thought_signature, key: String, category: String },
ListMemories     { id, thought_signature, category: Option<String> },
SearchRepository { id, thought_signature, query: String, kind: String },
```

**9 matching `AiEvent` variants** (same field names and types).

**`to_tool_call()` arms:** serialize each variant's fields into the `arguments` JSON string.

**`id()` arms:** each returns `id`.

**`dispatch_tool_event()` in `src/ai/mod.rs`:** 9 new arms matching tool name strings to `AiEvent` variants, extracting args from the JSON `Value`.

**Verify:** `cargo build`

---

## Step 6 — `src/ai/tools.rs`: tool definitions (3 providers × 9 tools)

Follows the exact pattern of `write_script` / `read_script` / `list_scripts` for each provider (Anthropic `get_tool_definition`, OpenAI `get_openai_tool_definition`, Gemini `function_declarations`).

**Tool descriptions:**

| Tool | Parameters | Description |
|------|-----------|-------------|
| `write_runbook` | `name: string, content: string` | Create or update runbook. Must use standard format. User approval required. |
| `delete_runbook` | `name: string` | Delete runbook. User approval required. Warns if active jobs reference it. |
| `read_runbook` | `name: string` | Read full runbook content. |
| `list_runbooks` | *(none)* | List runbook names and tags. |
| `add_memory` | `key: string, value: string, category: string` | Store/overwrite a memory entry. category: session/knowledge/incident. |
| `delete_memory` | `key: string, category: string` | Remove a memory entry. |
| `read_memory` | `key: string, category: string` | Read a specific memory entry. |
| `list_memories` | `category?: string` | List memory keys, optionally filtered by category. |
| `search_repository` | `query: string, kind: string` | Search runbooks/scripts/memory/events. kind: runbooks\|scripts\|memory\|events\|all. |

**Verify:** `cargo build`

---

## Step 7 — `src/daemon/server.rs`: streaming loop + memory injection

**9 new `AiEvent` arms** in the streaming match, following the `WriteScript` push pattern:
```rust
AiEvent::WriteRunbook { id, name, content, thought_signature } => {
    pending_calls.push(PendingCall::WriteRunbook { id, thought_signature, name, content });
}
// ... etc for all 9
```

**Memory injection in first-turn context:**
```rust
// In the is_first_turn branch, before assembling the format! string:
let memory_block = crate::memory::load_session_memory_block();

// Add to format! string between Execution Context and Terminal Session:
// {memory_block}## Terminal Session\n```\n{session_summary}\n```\n\n
// load_session_memory_block() returns "" when no memories exist — no format change for new users
```

**Verify:** `cargo build`

---

## Step 8 — `src/daemon/executor.rs`: PendingCall dispatch arms

All 9 arms added to the main `match call { ... }` block.

**Approval-gated arms** (same pattern as `WriteScript` arm):

`WriteRunbook` — send `RunbookWritePrompt`, timeout, read `RunbookWriteResponse`, if approved call `runbook::write_runbook`.

`DeleteRunbook` — scan `schedule_store.list()` for jobs where `job.runbook == Some(name)`, collect names into `active_jobs`, send `RunbookDeletePrompt`, timeout, read `RunbookDeleteResponse`, if approved call `runbook::delete_runbook`.

**Read-only arms** (same pattern as `ReadScript` — no approval gate):

`ReadRunbook` → `runbook::load_runbook(name).map(|rb| rb.content)`

`ListRunbooks` → `runbook::list_runbooks()`, send `Response::RunbookList`, return count string.

**Memory arms** (no approval gate — direct file ops):

`AddMemory` → `memory::add_memory(key, value, MemoryCategory::from_str(category)...)`

`DeleteMemory` → `memory::delete_memory(key, category)`

`ReadMemory` → `memory::read_memory(key, category)`

`ListMemories` → `memory::list_memories(category.as_deref().map(...))`, return formatted table string.

**Search arm** (no approval gate):

`SearchRepository` → `search::search_repository(query, kind, 2)` → `search::format_results(&results)`

**Verify:** `cargo build`

---

## Step 9 — `src/cli/commands.rs`: CLI approval gate handling

**`Response::RunbookWritePrompt`** — display `runbook_name` + full `content` (same rendering as `ScriptWritePrompt`), prompt `[Y]es / [N]o`, send `Request::RunbookWriteResponse { id, approved }`.

**`Response::RunbookDeletePrompt`** — display `runbook_name`; if `active_jobs` is non-empty, warn "⚠ The following scheduled jobs reference this runbook: <list>". Prompt `[Y]es / [N]o`. Send `Request::RunbookDeleteResponse { id, approved }`.

**`Response::RunbookList`** — render as table (name + tags columns), same as `ScriptList` rendering.

**Verify:** `cargo build`

---

## Step 10 — `sre.toml` + `src/config.rs` (`SRE_PROMPT_TOML`)

Add a `## Knowledge Tools` section to the system prompt (after existing tool heuristics):

```
## Knowledge Tools

### Runbooks
- Always call `list_runbooks` or `search_repository(kind:"runbooks")` before creating a new runbook to avoid duplicates.
- `write_runbook` requires the standard format:
  ```
  ---
  tags: [tag1, tag2]
  memories: [knowledge_key1, knowledge_key2]
  ---
  # Runbook: <name>
  ## Purpose
  ## Alert Criteria
  ## Remediation Steps
  ## Notes
  ```
- After resolving an alert via a runbook, update its `## Notes` section with what you learned.
- Populate `memories:` frontmatter with relevant knowledge memory keys so they are automatically loaded when the runbook is used.
- `delete_runbook` requires user approval and warns if active scheduled jobs reference it.

### Memory
Three categories with different loading behaviour:
- **session** — Loaded at the start of every chat session. Keep entries brief. Use for user preferences and recurring environment notes.
- **knowledge** — Loaded on-demand when referenced by a runbook's `memories:` field or via `read_memory`. Use for specific technical facts about named services, hosts, or configurations.
- **incident** — Never auto-loaded. Searchable via `search_repository`. Use to record root cause, symptoms, and fix after closing a significant issue.

Before writing a new memory, call `list_memories` to check if an entry should be updated instead of created.

Write a **session** memory when you learn a durable user preference.
Write a **knowledge** memory when you discover something specific about a named service or host configuration.
Write an **incident** memory when closing a significant issue.

### Search
- Use `search_repository` before listing files when looking for something specific.
- Use `kind:"all"` for open-ended discovery across runbooks, scripts, and memory simultaneously.
- Use `kind:"events"` to find historical command executions and past alert history.
```

Both `sre.toml` and `SRE_PROMPT_TOML` in `src/config.rs` must be updated identically.

**Verify:** `cargo build && cargo test` — `builtin_sre_prompt_parses` must pass.

---

## Step 11 — Tests

**`src/memory.rs`:**
- `add_and_read_memory` — write + read roundtrip
- `add_memory_upsert` — overwrite existing key returns updated value
- `delete_memory_removes_file`
- `list_memories_returns_all_categories`
- `session_memory_block_respects_cap` — >32 KB of session memories → block is truncated + note appended

**`src/runbook.rs`:**
- `load_runbook_parses_frontmatter` — write markdown with frontmatter, assert tags/memories populated
- `write_runbook_validates_missing_heading` — assert `Err` on missing `# Runbook:` heading
- `write_runbook_validates_missing_alert_criteria` — assert `Err` on missing section
- `list_runbooks_returns_sorted` — write 3 runbooks, assert alphabetical order

**`src/search.rs`:**
- `search_finds_match_in_runbooks` — write runbook containing keyword, assert match returned
- `search_returns_empty_for_no_match`
- `search_kind_filter_excludes_other_dirs`

**Verify:** `cargo test` — all new tests pass, all 165+ existing tests pass.

---

## Commit Sequence

| # | Commit message | Files |
|---|----------------|-------|
| 1 | Add `memory` module with session/knowledge/incident storage | `src/memory.rs`, `src/main.rs` |
| 2 | Rewrite runbook module for markdown format with CRUD | `src/runbook.rs` |
| 3 | Add `search` module for repository keyword search | `src/search.rs`, `src/main.rs` |
| 4 | IPC: RunbookWritePrompt/DeletePrompt/RunbookList variants | `src/ipc.rs` |
| 5 | AI types and tools: 9 new tool definitions (all providers) | `src/ai/types.rs`, `src/ai/mod.rs`, `src/ai/tools.rs` |
| 6 | Daemon: wire 9 new tools in server + executor; inject session memory | `src/daemon/server.rs`, `src/daemon/executor.rs` |
| 7 | CLI: RunbookWritePrompt, RunbookDeletePrompt, RunbookList rendering | `src/cli/commands.rs` |
| 8 | System prompt: document knowledge tools in sre.toml + config.rs | `sre.toml`, `src/config.rs` |
| 9 | Tests: memory, runbook, search modules | all new test functions |

---

## File System Layout (final state)

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
    ├── session/       # loaded at every session start
    │   ├── user_prefs.md
    │   └── environment.md
    ├── knowledge/     # on-demand (referenced by runbooks or read_memory)
    │   ├── nginx_config.md
    │   └── k8s_prod.md
    └── incidents/     # historical, searchable only
        └── 2026-02-15-nginx-oom.md
```
