# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```sh
cargo build                        # debug build
cargo build --release              # release build (binary at target/release/daemoneye)
cargo test                         # run all tests
cargo test <test_name>             # run a single test by name
cargo test -- --nocapture          # run tests with stdout visible
```

The project compiles cleanly with only pre-existing `dead_code` warnings — no errors. 298 tests pass.

## Architecture Overview

DaemonEye is a Rust daemon that embeds an AI assistant into `tmux`. It forks into the background, binds a Unix domain socket (`~/.daemoneye/daemoneye.sock`, resolved via `config::default_socket_path()`), and communicates with CLI clients via newline-delimited JSON.

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
| `src/daemon/server.rs` | IPC server loop; AI prompt assembly; session store (`HashMap<session_id, SessionEntry>`) |
| `src/daemon/executor.rs` | Tool call dispatch; approval gate (`ToolCallOutcome`); background/foreground execution coordination |
| `src/daemon/background.rs` | `run_background_in_window`, `notify_job_completion`, GC lifecycle |
| `src/daemon/session.rs` | Detects daemon hostname and whether the user's pane is local/SSH/mosh |
| `src/daemon/utils.rs` | Event logger (`events.jsonl`), `command_has_sudo`, `is_interactive_command`, `interactive_destination`, `normalize_output` helpers |
| `src/ai/types.rs` | `PendingCall` enum (one variant per AI tool), `AiEvent`, `Message`, `AiUsage` |
| `src/ai/mod.rs` | `AiClient` trait; `dispatch_tool_event()` |
| `src/ai/tools.rs` | Tool definitions for all three providers (Anthropic / OpenAI / Gemini) |
| `src/ai/backends/` | Per-provider SSE streaming implementations |
| `src/ai/filter.rs` | Regex-based sensitive-data masking; `init_masking()` at daemon start |
| `src/tmux/mod.rs` | All `tmux` subprocess calls (one function per operation) |
| `src/tmux/cache.rs` | Background 2 s poll; `SessionCache`, `PaneState`, `get_labeled_context()` |
| `src/tmux/session.rs` | Session-level tmux helpers: `other_sessions_context()`, `format_other_sessions()`, `client_dimensions()`, `session_environment()`, `list_sessions()` |
| `src/util.rs` | `UnpoisonExt` trait — `unwrap_or_log()` extension on `LockResult` that logs ERROR on poison recovery |
| `src/config.rs` | `~/.daemoneye/config.toml` parsing; `SRE_PROMPT_TOML` constant; `AiConfig::resolve_api_key()` |
| `src/scheduler.rs` | `ScheduleStore` (atomic JSON persistence); `run_scheduled_job()` |
| `src/scripts.rs` | Script management in `~/.daemoneye/scripts/` (chmod 700, path-traversal validation) |
| `src/runbook.rs` | TOML runbook loader; `watchdog_system_prompt()` for AI watchdog analysis |
| `src/sys_context.rs` | One-shot host audit (OS, uptime, memory, processes, shell history); `OnceLock` |
| `src/cli/` | Terminal rendering, readline input, session-level approval state, chat/ask/notify commands |

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

`[Catch-up]` — a `Response::SystemMsg` sent before the first AI token on the turn after a tmux client re-attaches following ≥ 30 s of detachment. Generated by `build_catchup_brief()` (`daemon/server.rs`) which scans messages added since `messages_at_detach` for event prefixes (`[Background Task Completed`, `[Webhook Alert]`, `[Watchdog]`, `[Watch Pane`). `SessionEntry.last_detach` / `messages_at_detach` are set by `NotifyClientDetached`; cleared by `NotifyClientAttached` or after brief generation (N15).

### Adding a new AI tool (checklist)

1. `src/ai/types.rs`: add `PendingCall::ToolName { ... }` variant + `to_tool_call()` arm + `id()` arm + `tool_name()` arm.
2. `src/ai/types.rs`: add `AiEvent::ToolName { ... }` variant.
3. `src/ai/tools.rs`: add a `ToolDef` entry to the `TOOLS` slice (Anthropic + OpenAI share it); add dispatch arm in `dispatch_tool_event()`.
4. `src/ai/backends/gemini.rs`: add inline entry to the `function_declarations` array in `chat()`.
5. `src/daemon/server.rs`: add `AiEvent::ToolName` arm in the streaming match.
6. `src/daemon/executor.rs`: add `PendingCall::ToolName` arm in `execute_tool_call()`.
7. `src/config.rs` (`SRE_PROMPT_TOML` / `assets/prompts/sre.toml`): document the new tool.

### Current AI tools

| Tool | Description |
|---|---|
| `run_terminal_command` | Foreground (user pane) or background (daemon host window) |
| `schedule_command` | One-shot or recurring scheduled jobs |
| `list_schedules` / `cancel_schedule` / `delete_schedule` | Schedule management |
| `write_script` / `read_script` / `list_scripts` / `delete_script` | Script CRUD in `~/.daemoneye/scripts/` |
| `watch_pane` | Block until regex `pattern` matches pane output, or command exits, or timeout |
| `read_file` | Paginated daemon-host file read with optional grep filter; masks sensitive data; path `canonicalize()`d to resolve symlinks; **blocked from `~/.daemoneye/`** |
| `edit_file` | Atomic string replacement in daemon-host file; requires user approval; path `canonicalize()`d, tmp at `<canonical>.de_tmp`; **blocked from `~/.daemoneye/`** |
| `write_runbook` / `read_runbook` / `delete_runbook` / `list_runbooks` | Runbook CRUD |
| `add_memory` / `read_memory` / `delete_memory` / `list_memories` | Persistent memory |
| `search_repository` | Grep across runbooks / scripts / memory / events |
| `get_terminal_context` | Fresh tmux snapshot on demand |
| `list_panes` | Enumerate all panes in session (pane ID, window-relative index, window, cmd, cwd, title) |

## Important Invariants

- `main()` is synchronous so `libc::fork()` can be called before the tokio runtime starts. Never move the fork inside an async context.
- All mutex lock sites use `.unwrap_or_log()` (the `UnpoisonExt` trait from `src/util.rs`) to recover from poisoned locks — do not change these to `.unwrap()`. The trait logs an ERROR before returning the inner value so poison events are visible in `daemon.log`.
- tmux window names for daemon-managed windows use predictable prefixes: `de-bg-*` (background execution), `de-sched-*` (scheduled jobs). These are used for GC and listing.
