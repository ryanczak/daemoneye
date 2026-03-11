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

The project compiles cleanly with only pre-existing `dead_code` warnings — no errors. Tests currently do not compile due to an unresolved import (`E0432`) in a test configuration; `cargo build` succeeds.

## Architecture Overview

DaemonEye is a Rust daemon that embeds an AI assistant into `tmux`. It forks into the background, binds a Unix domain socket (`/tmp/daemoneye.sock`), and communicates with CLI clients via newline-delimited JSON.

### Request/Response lifecycle

1. User runs `daemoneye chat` or `daemoneye ask` — the CLI client reads `$TMUX_PANE`, connects to the socket, and sends a `Request::Ask`.
2. The daemon captures the user's pane via `tmux capture-pane`, applies the masking filter (`ai/filter.rs`), assembles the system prompt + context snapshot, and streams tokens from the configured LLM.
3. When the AI emits a tool call the daemon sends `Response::ToolCallPrompt` back to the client. The client prompts the user: `[Y]es / [A]pprove session / [N]o / or type a message to redirect`. The client returns `Request::ToolCallResponse`.
   - **Y / A / N**: standard approve/session-approve/deny flow.
   - **Typed message**: `approved: false` with `user_message: Some(text)`. The daemon aborts the entire pending tool chain (omitting it from history), injects the text as a plain user turn, and re-enters the AI loop so the model can course-correct without seeing a synthetic tool error.
4. Approved commands run in one of two modes: **background** (dedicated `de-bg-*` tmux window on the daemon host, monitored via `pane-died` hook) or **foreground** (injected into the user's active pane via `send-keys`, completion detected via a three-way branch: interactive commands like `ssh`/`mosh`/`telnet`/`screen` use prompt-pattern detection and return immediately once connected; remote panes use output-stability polling; local panes poll `pane_current_command`).
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
| `src/daemon/utils.rs` | Event logger (`events.jsonl`), `command_has_sudo`, `is_interactive_command`, `interactive_destination` helpers |
| `src/ai/types.rs` | `PendingCall` enum (one variant per AI tool), `AiEvent`, `Message`, `AiUsage` |
| `src/ai/mod.rs` | `AiClient` trait; `dispatch_tool_event()` |
| `src/ai/tools.rs` | Tool definitions for all three providers (Anthropic / OpenAI / Gemini) |
| `src/ai/backends/` | Per-provider SSE streaming implementations |
| `src/ai/filter.rs` | Regex-based sensitive-data masking; `init_masking()` at daemon start |
| `src/tmux/mod.rs` | All `tmux` subprocess calls (one function per operation) |
| `src/tmux/cache.rs` | Background 2 s poll; `SessionCache`, `PaneState`, `get_labeled_context()` |
| `src/config.rs` | `~/.daemoneye/config.toml` parsing; `SRE_PROMPT_TOML` constant; `AiConfig::resolve_api_key()` |
| `src/scheduler.rs` | `ScheduleStore` (atomic JSON persistence); `run_scheduled_job()` |
| `src/scripts.rs` | Script management in `~/.daemoneye/scripts/` (chmod 700, path-traversal validation) |
| `src/runbook.rs` | TOML runbook loader; `watchdog_system_prompt()` for AI watchdog analysis |
| `src/sys_context.rs` | One-shot host audit (OS, uptime, memory, processes, shell history); `OnceLock` |
| `src/cli/` | Terminal rendering, readline input, session-level approval state, chat/ask/notify commands |

### Global statics in daemon

- `FG_DONE_TX`: `OnceLock<broadcast::Sender<()>>` — shared by foreground completion (P6) and `watch_pane` (P8).
- `FG_HOOK_COUNTER`: `AtomicUsize` — unique `alert-activity[N]` slot per concurrent watcher.

### Session context format

```
[SESSION TOPOLOGY] N windows — name (K panes, active/zoomed), …
[SESSION ENVIRONMENT] KEY=value, …
[ACTIVE PANE %N | cwd: /path | scrolled N lines up | copy mode]
[BACKGROUND PANE %N — cmd — /cwd (title) [synchronized]]: summary
```

### Adding a new AI tool (checklist)

1. `src/ai/types.rs`: add `PendingCall::ToolName { ... }` variant + `to_tool_call()` arm + `id()` arm.
2. `src/ai/tools.rs`: add tool definition for all three providers (Anthropic, OpenAI, Gemini).
3. `src/ai/mod.rs`: add `AiEvent::ToolName` variant and arm in `dispatch_tool_event()`.
4. `src/daemon/server.rs`: add `AiEvent::ToolName` arm in the streaming loop.
5. `src/daemon/executor.rs`: add execution handler in the pending calls loop.

## Important Invariants

- `main()` is synchronous so `libc::fork()` can be called before the tokio runtime starts. Never move the fork inside an async context.
- All mutex lock sites use `.unwrap_or_else(|e| e.into_inner())` to recover from poisoned locks — do not change these to `.unwrap()`.
- tmux window names for daemon-managed windows use predictable prefixes: `de-bg-*` (background execution), `de-sched-*` (scheduled jobs). These are used for GC and listing.
