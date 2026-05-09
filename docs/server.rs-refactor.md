# server.rs Decomposition Plan

## Current State

`server.rs` is 2,698 lines with one monolithic `handle_client` function
(lines 332–2558, ~2,226 lines) that handles:

- IPC parsing
- ~20 request-type dispatches (Ping, Shutdown, Ask, hook notifications,
  named session CRUD, etc.)
- Session upsert, catch-up brief, pipe-pane setup
- Token-pressure compaction (elide + digest)
- Prompt assembly (first-turn and subsequent-turn)
- AI streaming loop with tool call collection
- Tool budget enforcement
- Tool execution delegation to `executor`
- Ghost shell spawn
- Response persistence (memory + disk)
- Auto-naming suggestion

The file also contains utility functions, auto-naming helpers, and ~140
lines of tests.

## Guiding Principles

- **No public API changes** — `mod.rs` has `pub use server::*`; the plan
  preserves `handle_client`'s signature and `build_catchup_brief` /
  `is_valid_pane_id` visibility.
- **Extract by responsibility, not by size** — each new module owns one
  coherent concern.
- **Follow existing patterns** — `SessionCtx`-style parameter structs,
  `#[cfg(test)]` modules co-located with code, `unwrap_or_log()` for
  mutex recovery.
- **Keep tests with their code** — the 14 test functions stay adjacent
  to the functions they test.
- **Each phase is independently `cargo test`-verifiable** — 586 tests
  must pass after every phase.

---

## File Size Targets

| File (after) | ~Lines | Responsibility |
|---|---|---|
| `server.rs` | 600 | IPC parse + dispatch + `handle_ask` orchestrator |
| `hook.rs` | 200 | 9 hook notification handlers |
| `auto_name.rs` | 120 | Session naming + diff summary |
| `prompt.rs` | 250 | Prompt assembly via `PromptCtx` |
| `stream.rs` | 700 | AI event loop + tool exec + persistence |
| **Total** | **~1,870** | Net reduction from dedup and tighter scoping |

---

## Phase 1: Low-Risk Extractions (no `handle_client` surgery)

### Phase 1.1: `src/daemon/hook.rs` — IPC Hook Notification Handlers

**Status:** [x] Done

**What moves here:** The ~10 request arms in the `handle_client` match
that handle hook notifications — lines ~1029–1186:

- `NotifyActivity`
- `NotifyComplete`
- `NotifyFocus`
- `NotifyWindowChanged`
- `NotifySessionClosed`
- `NotifySessionCreated`
- `NotifyClientAttached`
- `NotifyClientDetached`
- `NotifyResize`

**New file shape:**

```
src/daemon/hook.rs
├── async fn handle_notify_activity(tx, pane_id)
├── async fn handle_notify_complete(tx, pane_id, exit_code)
├── async fn handle_notify_focus(cache, tx, pane_id)
├── async fn handle_notify_window_changed(cache, tx)
├── async fn handle_notify_session_closed(sessions, cache, managed_session, bg_session, tx, session_name)
├── async fn handle_notify_session_created(tx, session_name)
├── async fn handle_notify_client_attached(sessions, tx, session_name)
├── async fn handle_notify_client_detached(sessions, tx, session_name)
├── async fn handle_notify_resize(cache, tx, width, height)
└── #[cfg(test)] mod tests
```

**Why:** Simple, self-contained handlers. Each validates a pane ID,
updates cache, broadcasts on a channel, or modifies session state.
Currently buried inside a 2K-line match block. Extracting them makes
the routing section dramatically shorter and makes each handler
independently testable.

**Dependencies:** `SessionCache`, `SessionStore`,
`managed_session: Arc<Option<String>>`, `bg_session: Arc<Mutex<String>>`.
All already available. No new imports needed.

---

### Phase 1.2: `src/daemon/auto_name.rs` — Session Auto-Naming

**Status:** [x] Done

**What moves here:** `AUTONAME_SYSTEM_PROMPT`, `AUTONAME_TIMEOUT_SECS`,
`suggest_session_name()`, `diff_sessions_summary()` — lines 156–330.

**New file shape:**

```
src/daemon/auto_name.rs
├── const AUTONAME_SYSTEM_PROMPT
├── const AUTONAME_TIMEOUT_SECS
├── async fn suggest_session_name(messages, config) -> Option<(String, String)>
├── async fn diff_sessions_summary(meta1, meta2, config) -> Option<String>
└── #[cfg(test)] mod tests
```

**Why:** Two async functions that make independent LLM calls, not part
of the core request/response flow. Called from two different places in
`handle_client` (auto-name suggestion after turn completion and the
`DiffSessions` request arm). Self-contained and trivially extractable.

---

### Phase 1.3: `src/daemon/prompt.rs` — Prompt Assembly

**Status:** [x] Done

**What moves here:** All prompt construction logic from `handle_client`:

- First-turn prompt assembly (~lines 1562–1595)
- Subsequent-turn prompt with budget line (~lines 1596–1687)
- `prepend_foreground_target()` (lines 37–64)
- `current_time_line` formatting
- `pane_map` injection
- Activity-based snapshot refresh logic
- `[BUDGET]` line construction

**`PromptCtx` struct design:**

```rust
struct PromptCtx<'a> {
    session_summary: &'a str,
    default_target_pane: Option<&'a str>,
    cache: &'a SessionCache,
    sys_ctx: &'a str,
    daemon_host: &'a str,
    environment: &'a str,
    pane_location: &'a str,
    chat_width: Option<u32>,
    memory_block: &'a str,
    manifest_block: &'a str,
    auto_search_block: &'a str,
    pane_map: &'a str,
    current_time_line: &'a str,
    safe_query: &'a str,
    // For subsequent turns:
    last_prompt_tokens: u32,
    context_window: u32,
    history_count: usize,
    history_cap: Option<usize>,
    this_turn_count: usize,
    ghost_turn_limit: Option<usize>,
    inject_snapshot: bool,
}
```

Follows the `SessionCtx<'a>` pattern already used in `executor/mod.rs` —
borrow-only, no clones.

**New file shape:**

```
src/daemon/prompt.rs
├── struct PromptCtx<'a>
├── fn build_first_turn_prompt(ctx: &PromptCtx) -> String
├── fn build_subsequent_turn_prompt(ctx: &PromptCtx) -> String
├── fn build_budget_line(parts: &[String], max_pct: u32) -> String
├── fn prepend_foreground_target(ctx: &str, target_pane: Option<&str>, cache: &SessionCache) -> String
├── fn format_current_time_line() -> String
└── #[cfg(test)] mod tests
```

**Why:** ~200 lines of string formatting currently interleaved with
session state management. Clear inputs (session context, pane info,
token counts) and clear output (the prompt string). Extracting it makes
the `handle_client` flow easier to follow and enables testing prompt
construction with mock data.

---

## Phase 2: Core Loop Extraction

### Phase 2.1: Split `handle_client` into Dispatch Table + `handle_ask`

**Status:** [x] Done

**After decomposition, `server.rs` becomes:**

```rust
// server.rs (~600 lines)

// ── Utility helpers ──
fn is_valid_pane_id(id: &str) -> bool
pub fn build_catchup_brief(new_msgs, away_secs) -> Option<String>

// ── Main entry point ──
pub async fn handle_client(stream, cache, sessions, schedule_store,
                           bg_session, managed_session) -> Result<()>
    // Parse IPC message
    // Match request type → dispatch to handlers
    // For Ask: extract fields, call handle_ask()

// ── Ask handler (the heavy path) ──
async fn handle_ask(
    initial_query, client_pane, session_id, chat_pane,
    prompt_override, chat_width, client_tmux_session, client_target_pane,
    mut tx, mut rx, cache, sessions, schedule_store, bg_session, config
) -> Result<()>
    // Session upsert + catch-up + pipe-pane
    // Compaction logic
    // Prompt assembly (delegates to prompt.rs)
    // Send SessionInfo + SystemMsg
    // Call run_conversation_loop()

// ── Quick-return request handlers ──
async fn handle_ping(tx) -> Result<()>
async fn handle_shutdown(tx) -> Result<()>
async fn handle_refresh(tx) -> Result<()>
async fn handle_set_model(tx, sessions, config, session_id, model) -> Result<()>
async fn handle_list_models(tx, sessions, config, session_id) -> Result<()>
async fn handle_set_pane(tx, sessions, cache, session_id, pane_id) -> Result<()>
async fn handle_list_panes(tx, sessions, cache, session_id) -> Result<()>
async fn handle_status(tx, sessions, schedule_store, config) -> Result<()>
async fn handle_query_limits(tx, sessions, config, session_id) -> Result<()>
async fn handle_reset_tool_count(tx, sessions, session_id) -> Result<()>

// Named session CRUD handlers
async fn handle_save_session(tx, sessions, session_id, name, description, force) -> Result<()>
async fn handle_load_session(tx, sessions, config, session_id, name, force) -> Result<()>
async fn handle_list_saved_sessions(tx) -> Result<()>
async fn handle_delete_saved_session(tx, name) -> Result<()>
async fn handle_rename_saved_session(tx, sessions, old_name, new_name) -> Result<()>
async fn handle_diff_sessions(tx, sessions, config, name1, name2) -> Result<()>

// ── Tests ──
#[cfg(test)] mod tests
```

**Key design decisions:**

1. **`handle_client` stays thin** (~200 lines): parse IPC, match request
   type, dispatch. The match arms become 1-liners calling the appropriate
   handler function.

2. **`handle_ask`** (~600 lines): owns the full Ask lifecycle — session
   upsert, compaction, prompt assembly, sending SessionInfo, and calling
   `run_conversation_loop()`. Substantial, but each subsection has a
   clear role.

3. **Quick-return handlers** are extracted as standalone `async fn`s.
   Each handles one request type, sends its response, and returns. This
   makes the routing section in `handle_client` a clean dispatch table
   rather than inline logic.

---

### Phase 2.2: `src/daemon/stream.rs` — AI Event Streaming Loop

**Status:** [x] Done

**What moves here:** The inner `loop { ... }` from lines 1753–2556 —
the AI event streaming, token forwarding, tool call collection, budget
enforcement, tool execution, ghost spawn, response persistence, and
auto-name suggestion.

**New file shape:**

```
src/daemon/stream.rs
├── async fn run_conversation_loop(
│       mut tx, sessions, session_id, session_name, chat_pane,
│       messages, sys_prompt, session_active_model, is_ghost_session,
│       this_turn_count, pre_trim_len, post_trim_len, needs_compaction,
│       config, cache, schedule_store
│   ) -> Result<()>
│   // Outer loop: AI turn iteration
│   // Inner loop: event streaming + tool call collection
│   // Tool execution delegation
│   // Response persistence
│   // Auto-name suggestion
│
├── async fn collect_events_and_tool_calls(
│       mut ai_rx, mut tx, config, cache, schedule_store,
│       session_ctx
│   ) -> (String, Vec<PendingCall>)
│   // Inner loop: stream events, collect PendingCalls
│
├── fn enforce_tool_budget(
│       call: &PendingCall,
│       tool_call_counts: &mut HashMap<&str, u32>,
│       total_turn_call_count: &mut u32,
│       config: &Config,
│       session_id: Option<&str>,
│       sessions: &SessionStore
│   ) -> Option<ToolResult>
│   // Per-tool batch cap, per-turn total cap, per-session cap
│
├── fn truncate_tool_results(
│       tool_results: Vec<ToolResult>,
│       char_cap: Option<usize>
│   ) -> Vec<ToolResult>
│   // Truncate for history storage
│
├── async fn persist_turn_response(
│       tx: &mut WriteHalf,
│       sessions: &SessionStore,
│       session_id: Option<&str>,
│       messages: &[Message],
│       usage: &AiUsage,
│       this_turn_count: usize,
│       pre_trim_len: usize,
│       post_trim_len: usize,
│       needs_compaction: bool,
│       config: &Config,
│       chat_pane: Option<&str>
│   ) -> Result<()>
│   // Write to in-memory store and on-disk file
│   // Send UsageUpdate + Ok
│
├── async fn suggest_auto_name(
│       tx: &mut WriteHalf,
│       sessions: &SessionStore,
│       session_id: Option<&str>,
│       messages: &[Message],
│       config: &Config
│   ) -> Result<()>
│   // Delegated to auto_name.rs
│
├── const APPROVAL_GATED: &[&str]
│   // Approval-gated tool names (duplicated from config.rs for
│   // local use; kept in sync with LimitsConfig::APPROVAL_GATED)
│
└── #[cfg(test)] mod tests
```

**`run_conversation_loop` signature rationale:** It receives the
pre-assembled `messages` vec and configuration. It does not reach into
the session store for prompt assembly — that's `handle_ask`'s job. It
does need session store access for persistence and for the
`execute_tool_call` call (which needs `SessionCtx`).

---

## Phase 3: Module Registration & Cleanup

**Status:** [x] Done

### 3.1 Update `src/daemon/mod.rs`

```rust
pub mod auto_name;     // NEW
pub mod background;
pub mod digest;
pub mod executor;
pub mod ghost;
pub mod hook;          // NEW
pub mod policy;
pub mod prompt;        // NEW
pub mod scheduled;
pub mod server;
pub mod session;
pub mod stats;
pub mod stream;        // NEW
pub mod utils;
```

### 3.2 Update `src/daemon/server.rs` imports

```rust
use crate::daemon::auto_name;
use crate::daemon::hook;
use crate::daemon::prompt;
use crate::daemon::stream;
// ... existing imports ...
```

### 3.3 Verify

- `cargo build` — clean compilation ✓
- `cargo test` — 586 tests pass ✓
- `cargo build --release` — clean release build ✓

---

## Migration Order

1. **Phase 1.1** (`hook.rs`) — lowest risk. Self-contained handlers, no
   cross-dependencies. Extract as `pub(crate) async fn`, replace match
   arms with calls.

2. **Phase 1.2** (`auto_name.rs`) — trivial. Two async functions, two
   call sites, no shared mutable state.

3. **Phase 1.3** (`prompt.rs`) — moderate. Prompt assembly references
   many local variables. Requires introducing `PromptCtx` and carefully
   threading values.

4. **Phase 2.1** (split `handle_client`) — moderate. Request handlers
   each need the right subset of `cache`, `sessions`, `config`, etc.
   Some will need new parameters.

5. **Phase 2.2** (`stream.rs`) — highest risk. The conversation loop
   has the most complex control flow with nested loops, early returns,
   and shared mutable state via `messages` vec.

6. **Phase 3** — module registration, import updates, final verification.

## Results

All 6 phases complete. The decomposition extracted 4 new modules from `server.rs`,
reducing it from 2,698 to 1,634 lines (601 lines net removed).

| File | Lines | Responsibility |
|---|---|---|
| `server.rs` | 1,634 | IPC dispatch + `handle_ask` orchestrator |
| `hook.rs` | (new) | 9 IPC hook notification handlers |
| `auto_name.rs` | (new) | Session auto-naming + diff summary |
| `prompt.rs` | (new) | Prompt assembly via `PromptCtx` |
| `stream.rs` | 692 | AI event streaming loop + tool execution |

Verification: `cargo build` clean, `cargo test` 586 pass, `cargo build --release` clean.

## Notes

- `is_valid_pane_id()` and `build_catchup_brief()` remain in `server.rs`
  (small utilities, tested there).
- Tests for catch-up brief and pane ID validation stay in `server.rs`.
- `handle_client` signature unchanged — `mod.rs`'s `pub use server::*`
  continues to work.
- `SessionEntry` struct stays in `session.rs` (unchanged).
- `APPROVAL_GATED` constant moves to `stream.rs` alongside
  `enforce_tool_budget()`. A comment notes it is kept in sync with
  `LimitsConfig::APPROVAL_GATED` in `config.rs`.
