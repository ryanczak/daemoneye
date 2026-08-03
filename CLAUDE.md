# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```sh
cargo build                        # debug build
cargo build --release              # release build (binary at target/release/daemoneye)
cargo clippy --all-targets -- -D warnings  # deny all warnings (CI gate)
cargo test                         # run all tests
cargo test <test_name>             # run a single test by name
cargo test -- --nocapture          # run tests with stdout visible
```

The project compiles cleanly — `cargo clippy --all-targets -- -D warnings` exits zero. Tests pass (unit + integration in `tests/integration.rs`) + 1 ignored.

## Integration Tests

`tests/integration.rs` covers the persistence and IPC protocol layers without a running daemon or tmux session: IPC round-trips (Ask request, ToolCallResponse, SessionInfo), schedule store persistence, session JSONL write/read, session index persistence, event log format validation, event log append/read, and config parsing (minimal config, ghost config). All tests run in CI alongside the unit test suite.

## Architecture Overview

DaemonEye is a Rust daemon that embeds an AI assistant into `tmux`. It forks into the background, binds a Unix domain socket (`~/.daemoneye/var/run/daemoneye.sock`, resolved via `config::default_socket_path()`), and communicates with CLI clients via newline-delimited JSON.

### Request/Response lifecycle

1. User runs `daemoneye chat` or `daemoneye ask` — the CLI client reads `$TMUX_PANE`, connects to the socket, and sends a `Request::Ask`. If the tmux client was previously detached for ≥ 30 s and new event messages arrived (background completions, webhook alerts, watchdog results, watch-pane outcomes), the daemon sends a `Response::SystemMsg` catch-up brief immediately after `Response::SessionInfo` and before the first AI token (N15).
2. The daemon captures the user's pane via `tmux capture-pane`, applies the masking filter (`ai/filter.rs`), assembles the system prompt + context snapshot, and streams tokens from the configured LLM.
3. When the AI emits a tool call the daemon sends `Response::ToolCallPrompt` back to the client. For foreground commands the prompt includes a `target_pane` hint (computed synchronously from the cache before the approval wait) so the client can show the window-relative pane index and apply a visual highlight (`tmux select-pane -P bg=colour17`) to the target pane during the approval window; focus is immediately returned to the chat pane via a second `select-pane` call so the user is not displaced. The client prompts the user: `[Y]es / [A]pprove session / [N]o / or type a message to redirect`. The client returns `Request::ToolCallResponse`.
   - **Y / A / N**: standard approve/session-approve/deny flow.
   - **Typed message**: `approved: false` with `user_message: Some(text)`. The daemon aborts the entire pending tool chain (omitting it from history), injects the text as a plain user turn, and re-enters the AI loop so the model can course-correct without seeing a synthetic tool error.
4. Approved commands run in one of two modes: **background** (dedicated `de-bg-*` tmux window on the daemon host, monitored via `pane-died` hook) or **foreground** (injected into the user's active pane via `send-keys`, completion detected via a three-way branch: interactive commands like `ssh`/`mosh`/`telnet`/`screen` use prompt-pattern detection and return immediately once connected; remote panes use output-stability polling; local panes poll `pane_current_command`). During foreground execution the target pane is visually highlighted (`select-pane -P bg=colour17`) from `send_keys` until `capture_pane`; focus is immediately returned to the chat pane after each style change so the user is not displaced. The highlight is removed on denial or after capture.
5. The daemon sends `Response::ToolResult` with captured output, the LLM continues, and the loop repeats until the LLM produces a final answer.

### Key files

| Path | Role |
|---|---|
| `src/main.rs` | CLI entry point; forks daemon, routes subcommands |
| `src/ipc.rs` | `Request` / `Response` enums — the full wire protocol |
| `src/daemon/instance.rs` | `InstanceLock` — flock-based single-instance enforcement + PID payload |
| `src/daemon/ready.rs` | Fork readiness handshake — child reports `READY` / `ERR <msg>` to the parent over a pipe |
| `src/daemon/server/` | IPC server, split by concern: `mod.rs` client dispatch (`handle_client`), `ask.rs` the `handle_ask` orchestrator, `handlers.rs` the simple request handlers (`handle_ping`, `handle_shutdown`, `handle_refresh`, `handle_set_model`), `catchup.rs` the catch-up brief (`build_catchup_brief`, `is_valid_pane_id`) |
| `src/daemon/hook.rs` | 9 IPC hook notification handlers (`NotifyActivity`, `NotifyComplete`, `NotifyFocus`, etc.) |
| `src/daemon/auto_name.rs` | Session auto-naming (`suggest_session_name`, `diff_sessions_summary`) |
| `src/daemon/prompt.rs` | Prompt assembly via `PromptCtx` (`build_first_turn_prompt`, `build_subsequent_turn_prompt`) |
| `src/daemon/stream.rs` | AI event streaming loop (`run_conversation_loop`); tool execution; response persistence |
| `src/daemon/executor/` | Tool call dispatch; approval gate (`ToolCallOutcome`); background/foreground execution coordination; `ArtifactCtx` for session-origin stamping |
| `src/daemon/background/` | Background execution: `run.rs` `run_background_in_window()`, `respawn.rs` `respawn_background_in_pane()`, `gc.rs` `notify_job_completion()` + `gc_bg_windows()` + `OwnedJobInfo`, `helpers.rs` output capture/archive and session notification |
| `src/daemon/session.rs` | Detects daemon hostname and whether the user's pane is local/SSH/mosh |
| `src/daemon/digest.rs` | Session digest: structured compaction of conversation history at 30 messages; scans events.jsonl + filesystem for artifacts |
| `src/daemon/ghost.rs` | `GhostManager::start_session()` — allocates `de-gs-bg-*` / `de-gs-sj-*` / `de-gs-ir-*` windows for autonomous remediation; briefing generation on clean exit |
| `src/daemon/briefing.rs` | G4 briefing generation: `generate_and_save_briefing()`, `read_briefing()`, `clear_briefing()`; AI summarization + masking + file I/O |
| `src/daemon/policy.rs` | `GhostPolicy` — runtime enforcement of `auto_approve_scripts` / `auto_approve_read_only` for ghost shells |
| `src/daemon/utils/` | Daemon helpers, one file per concern: `event_log.rs` `log_event()` + dated-segment readers + `sweep_event_segments()`, `log_rotation.rs` `rotate_log_file()` + `reattach_log_fds()`, `warnings.rs` `retention_warnings()`, `shell.rs` escaping + `is_interactive_command()`/`interactive_destination()`, `sudo.rs` `command_has_sudo()` + fingerprint-prompt detection, `output.rs` `normalize_output()`, `response.rs` IPC response senders + `fire_notification()`, `host.rs` `daemon_hostname()`. `mod.rs` also holds `sweep_session_archives()`, `sweep_pane_logs()`, `sweep_agent_mailboxes()` |
| `src/ai/types/` | `pending.rs` the `PendingCall` enum (one variant per AI tool), `events.rs` `AiEvent`, `wire.rs` the provider wire types (`Message`, `ToolCall`, `ToolResult`, `TokenBreakdown`) |
| `src/ai/mod.rs` | `AiClient` trait; `dispatch_tool_event()` |
| `src/ai/tools/` | Tool definitions for all three providers (Anthropic / OpenAI / Gemini): `defs.rs` the flat `TOOLS` table, `schema.rs` `ToolDef`/`ParamDef` + `render_gemini()`, `args.rs` serde defaults for tool arguments, `dispatch.rs` `dispatch_tool_event()` |
| `src/ai/backends/` | Per-provider SSE streaming implementations |
| `src/ai/filter.rs` | Regex-based sensitive-data masking; `init_masking()` at daemon start |
| `src/tmux/mod.rs` | All `tmux` subprocess calls (one function per operation) |
| `src/tmux/cache.rs` | Background 2 s poll; `SessionCache`, `PaneState`, `get_labeled_context()` |
| `src/tmux/session.rs` | Session-level tmux helpers: `other_sessions_context()`, `format_other_sessions()`, `client_dimensions()`, `session_environment()`, `list_sessions()`, `session_exists()` |
| `src/util.rs` | `UnpoisonExt` trait — `unwrap_or_log()` extension on `LockResult` that logs ERROR on poison recovery |
| `src/config/` | Config, FHS paths and first-run seeding; the public surface is re-exported from `mod.rs`. `types.rs` `Config` + the per-section structs + `resolve_api_key()`, `load.rs` the path constructors (`config_dir()`, `var_log_dir()`, …), `seeds.rs` `SRE_PROMPT_TOML` + asset seeding, `path_audit.rs` the M6 path-audit gate, `lifecycle.rs` the M6 artifact-lifecycle policy table |
| `src/scheduler.rs` | `ScheduleStore` (atomic JSON persistence); `ActionOn` enum (`Alert`/`Script`/`Ghost`); `ScheduleKind` (`Once`/`Every`/`Cron`); `parse_cron()` helper |
| `src/scripts.rs` | Script management in `~/.daemoneye/scripts/` (chmod 700, path-traversal validation); `install_sudoers()` |
| `src/runbook.rs` | TOML runbook loader; `watchdog_system_prompt()` for AI watchdog analysis |
| `src/session_store.rs` | Named session persistence: `save/load/list/delete/rename_session()`; `ArtifactRef`; `backfill_session_origin()`; `build_resumed_banner()` |
| `src/memory.rs` | Memory module: `MemoryCategory` (note `Incident.dir_name()` is `incidents`, plural, while `canonical_name()` is `incident`), `MemoryInfo`, and CRUD — `add_memory` / `update_memory` / `delete_memory` / `read_memory` / `list_memories` / `list_memories_with_tags`. `memory_dir_for_namespace()` resolves the two-location layout: `memory/<category>/` for the `global` namespace, `agents/<ns>/memory/<category>/` otherwise. Masking (`mask_sensitive`) and the `SESSION_MEMORY_CAP` size cap apply in `load_session_memory_block()`, **not** in the mutators — the mutators do no locking, capping or masking. Frontmatter fields are `tags`, `summary`, `relates_to`, `created`, `updated`, `expires` |
| `src/memory/index.rs` | SQLite FTS5 memory index at `var/index/memory.db`. `open_index()` / `ensure_schema()` create it — a `SCHEMA_VERSION` mismatch drops and recreates, since the index is derived and a rebuild is always safe. `index_memory_file()` / `remove_from_index()` are called **best-effort** from the `src/memory.rs` mutators: an index failure logs a warning and never fails the caller. `reconcile_index()` rebuilds from the files on disk and runs automatically when the index is empty, which is what indexes the memories a fresh install seeds; to force a rebuild when the index is populated but stale, run `daemoneye reindex` (single-transaction, safe while the daemon is running, idempotent). `fts5_search()` returns BM25-ranked `(namespace, key, score)`, best first; `build_match_expr()` quotes each user term and joins with `OR`, because the caller passes a whole user turn and a phrase match would return nothing. The grep scan in `src/search.rs` backs the `search_repository` tool, not memory recall. |
| `src/memory/review.rs` | Memory review scoring: `effective_confidence()` |
| `src/memory/tags.rs` | G5 SessionTags derivation: tag inference from cwd, command, hostname, recent keywords |
| `src/daemon/memory_prompt.rs` | G5 tiered memory prompt: stable ambient block + dynamic turn-relevant block |
| `src/header.rs` | Inline header parser/renderer for all artifact types; `inject_yaml_session_origin()` / `inject_comment_session_origin()` |
| `src/sys_context.rs` | One-shot host audit (OS, uptime, memory, processes, shell history); `OnceLock` |
| `src/cli/` | Terminal rendering, readline input, session-level approval state, chat/ask/notify commands |
| `src/agents/mod.rs` | Named agents: `AgentConfig`, CRUD, validation, `apply_agent_to_ghost_config()` |
| `src/agents/policy.rs` | `ToolPolicy` — agent-level allow/deny tool lists, `permits()`, `format_tool_restriction_block()` |
| `src/agents/mailbox.rs` | G5 mailbox: `MailboxResult`, `write_mailbox()`, `read_mailbox()` for agent-to-agent delegation |

### Global statics in daemon

- `BG_DONE_TX`: `OnceLock<broadcast::Sender<String>>` — sends pane_id on activity; shared by foreground completion and `watch_pane`.
- `FG_HOOK_COUNTER`: `AtomicUsize` — unique `alert-activity[N]` hook slot per concurrent watcher.
- `DAEMON_START`: `OnceLock<Instant>` — recorded at daemon startup; used by `daemon_uptime_secs()` for `daemoneye status`.
- `BUFFER_COUNTER`: `AtomicUsize` — unique tmux buffer names (`de-rb-N`) for N12 local-pane file reads via `load-buffer`/`save-buffer`.

### Session context format

```
[SESSION TOPOLOGY] N windows — name (ID: @K, J panes, active/zoomed), …
[SESSION ENVIRONMENT] KEY=value, …
[CLIENT VIEWPORT] WxH
[ACTIVE PANE %N | idx:K in 'window' | cwd: /path | scrolled N lines up | copy mode]
[BACKGROUND PANE %N (idx:K in 'window') — cmd — /cwd (title) [synchronized] [dead: N] [active Xs ago]]: summary
[VISIBLE PANE %N (idx:K in 'window') — cmd — /cwd (title)]: summary
[SESSION PANE %N (idx:K in 'window') — cmd — /cwd (title)]: summary
[OTHER SESSIONS] name (N windows, active Xm ago, attached/detached), …
```

`idx:K` is the 0-based window-relative pane index — the number the user sees with `ctrl+a q`. Used by the AI to communicate pane targets in human-readable terms and displayed in the tool-call approval prompt so users can visually confirm the target before approving.

`[OTHER SESSIONS]` — appended by `other_sessions_context()` (`tmux/session.rs`) when two or more tmux sessions exist. Omitted in single-session setups and when there is no terminal context. Generated from `tmux list-sessions`; pure formatting extracted into `format_other_sessions()` for testability (N16).

`[Catch-up]` — a `Response::SystemMsg` sent before the first AI token on the turn after a tmux client re-attaches following ≥ 30 s of detachment. Generated by `build_catchup_brief()` (`daemon/server.rs`) which scans messages added since `messages_at_detach` for event prefixes (`[Background Task Completed`, `[Webhook Alert]`, `[Watchdog]`, `[Watch Pane`, `[Ghost Shell Started]`, `[Ghost Shell Completed]`, `[Ghost Shell Failed]`). `SessionEntry.last_detach` / `messages_at_detach` are set by `NotifyClientDetached`; cleared by `NotifyClientAttached` or after brief generation (N15).

### Adding a new AI tool (checklist)

1. `src/ai/types/pending.rs`: add `PendingCall::ToolName { ... }` variant + `to_tool_call()` arm + `id()` arm + `tool_name()` arm.
2. `src/ai/types/pending.rs`: add a `summary()` arm for the new variant (used for `ToolStarted` display). Add a `should_emit_tool_feedback()` arm: return `true` for silent (non-approval-gated) tools so the executor emits `ToolStarted`/`ToolFinished`; return `false` (via the catch-all `_ => false`) for approval-gated tools that already have richer UI.
3. `src/ai/types/events.rs`: add `AiEvent::ToolName { ... }` variant.
4. `src/ai/tools/defs.rs`: add a `ToolDef` entry to the `TOOLS` slice (all three backends share it via `render_gemini(TOOLS)`). **`deferred_group` is required**: `None` makes the tool core — sent with every request and prose-documented in `sre.toml`; `Some("group")` makes it deferred — omitted from the default render and pulled in by `load_tools`. Default to `None` unless the tool is rarely used and its schema is large.
5. `src/ai/tools/dispatch.rs`: add the dispatch arm in `dispatch_tool_event()`.
6. `src/ai/backends/gemini.rs`: no separate entry needed — Gemini tool definitions are auto-generated from `TOOLS` via `render_gemini(TOOLS)`.
7. `src/daemon/stream.rs`: add `AiEvent::ToolName` arm in the streaming match.
8. `src/daemon/executor/mod.rs`: add `PendingCall::ToolName` arm in `execute_tool_call()`. Agent tools (create/read/list/delete agent) dispatch to `executor/knowledge.rs` alongside runbook/memory tools.
9. `src/config/seeds.rs` (`SRE_PROMPT_TOML` / `assets/prompts/sre.toml`): document the new tool.
10. `CLAUDE.md`: add a row to the Current AI tools table below with the right `Loaded` value, **and bump the `N tools: C core + D deferred` line above it**. Both are enforced — `tests/doc_truth.rs` cross-references the table against `TOOLS` and will name the missing tool and the expected counts.

### Current AI tools

**33 tools: 24 core + 9 deferred.** `Loaded` mirrors `ToolDef.deferred_group` in
`src/ai/tools/defs.rs` — `core` means `None` (rendered on every request); a group
name means the tool is omitted until `load_tools` pulls the group in. **Note the
asymmetry**: for scripts, runbooks and memory the *write* side is core while the
*read* side is deferred, so a row like "script CRUD" would be wrong.

| Tool | Loaded | Description |
|---|---|---|
| `run_terminal_command` | core | Foreground (user pane) or background (daemon host window) |
| `edit_file` | core | File operations on daemon host (or remote via `target_pane`): `operation="edit"` (atomic string replacement, requires `old_string`/`new_string`), `operation="create"` (new file from `content`), `operation="delete"` (remove file), `operation="copy"` (duplicate `path` to `dest_path`). All require user approval with colored unified diff. Atomic writes via `.de_tmp` → rename. **Blocked from `~/.daemoneye/`**. IPC: `EditFilePrompt` / `EditFileResponse`. |
| `read_file` | core | Paginated daemon-host file read with optional grep filter; masks sensitive data; path `canonicalize()`d to resolve symlinks; **blocked only from `etc/config.toml` and `etc/prompts/sre.toml`** (API credential files) |
| `search_repository` | core | Grep across runbooks / scripts / memory / events |
| `get_terminal_context` | core | Fresh tmux snapshot on demand |
| `list_panes` | core | Enumerate all panes in session (pane ID, window-relative index, window, cmd, cwd, title) |
| `watch_pane` | core | Block until regex `pattern` matches pane output, or command exits, or timeout |
| `close_background_window` | core | Close a finished background window, freeing its slot rather than waiting for cap eviction (up to 5 exist per session) |
| `recall_context` | core | Retrieve archived turns compacted out of live context — by substring query, by turn range, or both. The answer to an `[elided: …]` placeholder or a too-coarse epoch summary. |
| `load_tools` | core | Pull one or more deferred groups into the active tool set for the rest of the session. `groups` is an array of group names. |
| `write_script` | core | Create/update a script in `~/.daemoneye/scripts/` (chmod 700); approval-gated with a diff |
| `delete_script` | core | Delete a script; approval-gated |
| `read_script` | **scripts** | Read a script's content |
| `list_scripts` | **scripts** | List scripts with sizes |
| `write_runbook` | core | Create/update a runbook in `~/.daemoneye/runbooks/`; approval-gated with a diff |
| `delete_runbook` | core | Delete a runbook; approval-gated |
| `read_runbook` | **runbooks** | Read a runbook's full content |
| `list_runbooks` | **runbooks** | List runbooks with their tags |
| `add_memory` | core | Store a persistent memory entry under `<category>/<key>.md` |
| `read_memory` | core | Read one entry by key + category |
| `update_memory` | core | Patch individual frontmatter/body fields in place; omitted fields are preserved, `updated` is stamped automatically. Preferred over read+delete+add. |
| `list_memories` | core | List keys, optionally filtered by category |
| `delete_memory` | **memory** | Remove an entry |
| `schedule_command` | core | One-shot, interval, or cron job — command, script, or ghost shell (`ghost_runbook`) |
| `list_schedules` | core | List jobs with status and next fire time |
| `cancel_schedule` | core | Cancel a job by UUID (kept in the store) |
| `delete_schedule` | core | Permanently delete a job by UUID |
| `spawn_ghost_shell` | core | Delegate a task to an autonomous background Ghost Shell that follows a named runbook |
| `await_agent_result` | core | Block on a spawned agent's mailbox for the `job_id` returned by `spawn_ghost_shell`, until the result lands or the timeout expires |
| `create_agent` | **agents** | Create/update `~/.daemoneye/agents/<name>/config.toml`; approval-gated |
| `read_agent` | **agents** | Read a named agent's full config |
| `list_agents` | **agents** | List agents with descriptions and models |
| `delete_agent` | **agents** | Delete an agent; approval-gated, warns if runbooks reference it |

## Important Invariants

- Every `events.jsonl` record carries `ts`, `event`, and `pid`; `log_event` stamps
  `pid` itself, so call sites must not pass one. Key **order** in the serialized
  line is `serde_json`'s (alphabetical — this crate does not enable
  `preserve_order`), so never rely on field position when parsing a record.
- `main()` is synchronous so `libc::fork()` can be called before the tokio runtime starts. Never move the fork inside an async context.
- Exactly one daemon may run per `$HOME`, enforced by an exclusive `flock` on
  `~/.daemoneye/var/run/daemoneye.pid` acquired in `run_daemon` before any
  startup side effect (`src/daemon/instance.rs`). The kernel releases it on
  process death, so there is no stale-lock recovery path. The PID written into
  the file is diagnostic payload only — never branch on it. Holding the lock is
  what authorizes unlinking a socket at `default_socket_path()`.
- All mutex lock sites use `.unwrap_or_log()` (the `UnpoisonExt` trait from `src/util.rs`) to recover from poisoned locks — do not change these to `.unwrap()`. The trait logs an ERROR before returning the inner value so poison events are visible in `daemon.log`.
- tmux window names for daemon-managed windows use the format `{prefix}{pane_num}-{unix_ts}-{cmd_slug}`, e.g. `de-bg-42-1712937600-cargo-build`. Prefixes: `de-bg-*` (interactive background execution), `de-sj-*` (regular scheduled jobs), `de-gs-bg-*` (ghost shell background commands, webhook/interactive), `de-gs-sj-*` (ghost shell background commands, scheduler-triggered), `de-gs-ir-*` (ghost shell incident-response main windows). Prefixes are used for GC filtering and listing. The pane number (`%42` → `42`) uniquely identifies the tmux pane; the unix timestamp replaces the old `YYYYMMDDHHMMSS` format; the command slug is the sanitized basename of the first meaningful command word (max 30 chars).
- Liveness probes (`daemon_liveness()`) are reports, never authorizations. No
  code may unlink a socket or remove a file based on a `DaemonLiveness` variant
  — instance ownership is decided only by the `InstanceLock` flock.
- The webhook listener binds eagerly in `run_daemon` and a bind failure is fatal.
  It is a duplicate-instance signal, not a transient condition to retry.
- `daemoneye daemon` (without `--console`) does not report success until the
  forked child has bound its socket. The parent relays the child's outcome over
  the readiness pipe (`src/daemon/ready.rs`) and exits non-zero if the child
  failed or died. The parent must drop its copy of the write end before reading,
  or a child that dies silently hangs it.

### Ghost Shell conventions

- **Detection signal**: `watchdog_system_prompt()` (`runbook.rs`) asks the watchdog model to emit `GHOST_TRIGGER: YES` or `GHOST_TRIGGER: NO` as the final line of its response. `parse_ghost_trigger()` in `webhook.rs` parses this (case-insensitive, last matching line wins) with fallback to legacy `ALERT` keyword check. `evaluate_watchdog_response()` (also in `webhook.rs`) is the shared helper used by both webhooks and scheduled jobs.
- **Turn loop**: `trigger_ghost_turn()` in `ghost.rs` runs the ghost AI loop. Each iteration creates a **fresh** `(ai_tx, ai_rx)` channel — the sender is moved (not cloned) into the spawned task so the channel closes when the task exits and `recv()` unblocks. A `timeout_at` guard prevents hung turns.
- **Turn budget**: `GhostConfig.max_ghost_turns` (runbook frontmatter `max_ghost_turns: N`; 0 = use daemon default of 20). Enforced in `trigger_ghost_turn`; a warning is logged if the limit is hit.
- **Policy enforcement**: `GhostPolicy` (from `GhostConfig`) is passed through `execute_tool_call()`. Ghost shells without a policy return an error rather than proceeding. `ToolPolicy` (agent-level) is also enforced — both must pass independently.
- **Concurrency cap**: `check_ghost_capacity()` (`daemon/ghost.rs`) returns false when `stats::get_ghosts_active() >= config.ghost.max_concurrent_ghosts`. Default cap is 3; set `max_concurrent_ghosts = 0` in `[ghost]` config to disable. Checked before spawning from both webhooks and scheduled jobs.
- **Lifecycle events**: `inject_ghost_event()` in `webhook.rs` injects `[Ghost Shell Started]` / `[Ghost Shell Completed]` / `[Ghost Shell Failed]` / `[Ghost Shell Skipped]` into all active sessions so they appear in catch-up briefs.
- **Session validation**: `session_exists()` (`tmux/session.rs`, wraps `tmux has-session -t`) is called before the AI loop to guard against stale session names.
- **Scheduled ghost jobs**: `ActionOn::Ghost { runbook }` in `scheduler.rs` routes scheduled jobs through `GhostManager::start_session()` + `trigger_ghost_turn()`. The `schedule_command` AI tool accepts a `ghost_runbook` param to schedule these jobs. Old `ActionOn::Command` entries in `schedules.json` are still loaded (backwards-compat) but deprecated.
- **Sudoers installation**: `daemoneye install-sudoers <script-name>` writes a NOPASSWD rule to `/etc/sudoers.d/daemoneye-<name>` allowing the current user to run the script without a password. Required for ghost shells and scheduled jobs that need sudo access to pre-vetted scripts.
- **GhostConfig fields**: `agent: Option<String>` (agent name), `tool_policy: Option<ToolPolicy>` (agent-level tool allow/deny), `spawn_depth: u8` (delegation depth, default 0), `parent_job_id: Option<String>` (parent ghost's job ID), `memory_namespace: Option<String>` (agent memory namespace).
- **Delegation depth**: `spawn_depth` starts at 0 for top-level ghosts. Each `spawn_ghost_shell` increments by 1. Depth 2+ is rejected with an error. Coordinator (depth 0) → specialist (depth 1) is the only allowed pattern.
- **Mailbox**: When a ghost shell exits, its final response is written to `~/.daemoneye/agents/<agent>/mailbox/<job_id>.json` via `write_mailbox_on_exit()`. The coordinator reads results via `await_agent_result(job_id, agent_name)`.

### Named session conventions

- **Storage**: `~/.daemoneye/var/sessions/<name>/meta.toml` + `messages.jsonl`; index at `var/sessions/index.json`. Distinct from `var/log/sessions/<id>.jsonl` (ephemeral per-session JSONL logs).
- **`SessionEntry` fields**: `saved_name: Option<String>` (None = unnamed), `dirty: bool` (true after any message write-back; false after save/load), `artifacts_created: Vec<ArtifactRef>`, `auto_name_suggested: bool`.
- **Auto-naming**: After `[sessions] auto_name_turn_threshold` turns (default 10) in an unnamed session, the daemon fires a one-shot `use_tools=false` LLM call and emits `Response::SystemMsg` suggesting a name. Guarded by `auto_name_suggested` flag so it fires exactly once. Disabled if `auto_name_enabled = false` or `auto_name_turn_threshold = 0`.
- **`session_origin` stamping**: When an artifact (memory, runbook, script) is created inside a named session (`saved_name.is_some()`), `ArtifactCtx` in `executor/knowledge.rs` injects `session_origin: "<name>"` into the file's frontmatter/comment-header via `header::inject_yaml_session_origin()` / `header::inject_comment_session_origin()` before the write. The artifact name is also pushed to `SessionEntry.artifacts_created`.
- **Retroactive backfill**: When an unnamed session is saved for the first time, `session_store::backfill_session_origin(&artifacts, name)` walks `artifacts_created` and stamps `session_origin` on each. Called from the `SaveSession` handler in `server.rs` when `current_saved_name` was `None`.
- **Ghost sessions are excluded**: `ArtifactCtx.is_ghost` guards `track_artifact()` — ghost sessions never write to `artifacts_created` or stamp `session_origin`.
- **Import**: `daemoneye session import <id> --name <name>` reads an orphaned `var/log/sessions/<id>.jsonl` and saves it to the named session store without a running daemon.

@REXYMCP.md
