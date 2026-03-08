# DaemonEye System Architecture

This document outlines the high-level architecture and core components of the DaemonEye daemon.

## 1. High-Level Architecture Diagram

```mermaid
graph TD
    subgraph Client ["User Terminal Environment"]
        TmuxClient[User's tmux Session]
        ActivePane[Active Working Pane]
        AIPane[AI Agent Pane / daemoneye chat]
    end

    subgraph Daemon ["DaemonEye Background Daemon"]
        IPCServer[IPC Server\n/tmp/daemoneye.sock]
        TmuxInterop[tmux Interoperability Layer]
        AgentEngine[AI Agent Engine]
        Filter[Security & Data Masking Filter]
        PromptManager[Prompt Engine Manager]
        APIClient[LLM API Client]
        SessionStore[Multi-Turn Session Store]
        SysContext[System Context Collector]
    end

    subgraph ConfigFiles ["File System (~/.daemoneye/)"]
        Prompts[(prompts/)]
        LogFile[(daemon.log)]
        CmdLog[(events.jsonl)]
        ConfigToml[(config.toml)]
        Scripts[(scripts/)]
        Runbooks[(runbooks/)]
        Memory[(memory/)]
        SchedStore[(schedules.json)]
    end

    Scheduler[(Scheduler Task)]
    WebhookServer[(Webhook HTTP :9393)]

    Cloud(("Anthropic / OpenAI / Gemini API"))

    %% User Interaction & Terminal I/O
    TmuxClient --- ActivePane
    TmuxClient --- AIPane

    %% IPC
    AIPane -->|Unix Socket JSON| IPCServer
    IPCServer -->|Token stream| AIPane

    %% AI Workflow
    IPCServer --> AgentEngine
    TmuxInterop -->|Capture Buffer (first turn + on-demand tool)| AgentEngine
    SysContext -->|Host audit (once per session)| AgentEngine
    AgentEngine -->|Raw Context| Filter
    Filter -->|Sanitized Context| APIClient
    PromptManager -->|System Prompt| APIClient
    SessionStore <-->|Load / persist history| AgentEngine

    APIClient <-->|REST streaming| Cloud

    %% Agent Output & Execution
    APIClient -->|Token stream| IPCServer
    APIClient -.->|Tool call: propose command| IPCServer
    IPCServer -.->|User approval prompt| AIPane
    AIPane -.->|Approved / denied| IPCServer
    IPCServer -.->|Inject command| TmuxInterop
    TmuxInterop -.->|send-keys| ActivePane
    TmuxInterop -.->|capture-pane (result)| AgentEngine

    %% Config
    PromptManager -.->|Load prompts| Prompts
    IPCServer -.->|Write| LogFile
    IPCServer -.->|Append execution record| CmdLog
    ConfigToml -.->|Read| AgentEngine

    %% Scheduler
    Scheduler -->|create de-<id> window| TmuxInterop
    Scheduler -->|watchdog analysis| AgentEngine
    Scheduler <-->|persist jobs| SchedStore
    Scripts -.->|resolve script path| Scheduler
    Runbooks -.->|load runbook| Scheduler

    %% Knowledge System
    Memory -.->|session memories (first turn)| AgentEngine
    Memory -.->|knowledge memories (watchdog)| Scheduler

    %% Webhook
    WebhookServer -->|inject alert messages| SessionStore
    WebhookServer -->|tmux display-message| TmuxInterop
    WebhookServer -->|watchdog analysis| AgentEngine
```

## 2. Core Components

### 2.1 User Terminal Environment

- **User's tmux Session**: DaemonEye does not render its own window or emulate a terminal. It operates entirely within the user's pre-existing terminal emulator via `tmux`.
- **Active Working Pane**: The pane where the user is currently working. When DaemonEye is invoked, it reads the content from this pane and injects approved commands here.
- **AI Agent Pane** (`cli/mod.rs`): A tmux pane running `daemoneye chat` or `daemoneye ask`. On startup the pane is resized to 40% of the window width (minimum 80 columns) using `tmux resize-pane`; `query_pane_width` is then used for accurate header rendering, avoiding a `TIOCGWINSZ` race condition. The chat interface features:
  - Width-adaptive header with session ID; all border calculations use `visual_len()` (strips ANSI escapes, counts Unicode code points) to handle multi-byte box-drawing characters correctly.
  - ANSI syntax highlighting in fenced code blocks: per-language keyword sets, comment styles, string literals, and numeric literals.
  - Inline markdown rendering: bold, italic, and `code` spans; AI prose is tinted bright-white.
  - Context-budget indicator embedded into the right-hand side of the bottom border of the user input box after each `SessionInfo` response. Shows `turn N · Xk / Yk tokens · Z% remaining`, where the context window (Yk) is derived from `AiConfig::context_window()` per model. Color-coded by percentage: dim when under 50 % used, yellow at 50–74 %, bold red at ≥ 75 %. Driven by the `UsageUpdate { prompt_tokens }` IPC response sent by the daemon after each completed AI turn; the `prompt_tokens` value is carried across turns as a `&mut u32` parameter so subsequent query boxes display the count from the previous turn.
  - `/clear` in-chat command: generates a new session ID so the next message starts a clean context, prints a dim separator, and updates the header hint.
  - `SystemMsg` responses rendered with an amber ⚙ prefix.
  - `ToolResult` responses rendered as a dimmed bordered panel capped at 10 rows with a truncation indicator.
  - **Session-level command approval**: `ToolCallPrompt` responses display a three-option prompt — `[Y]es` (once), `[A]pprove for session` (session-wide), `[N]o` (deny). Two independent approval classes are tracked in `SessionApproval { regular: bool, sudo: bool }`, stored in `run_chat_inner` and passed into `ask_with_session`. `daemon::utils::command_has_sudo` (regex `(?:^|[;&|])\s*sudo\b`) classifies each command — the CLI and daemon share a single implementation with no duplication. Once a class is session-approved, subsequent commands in that class display `✓ auto-approved (session)` without prompting. Approval state resets on `/clear`, `/prompt`, and `/refresh`.
  - **Interactive line editor**: The chat input runs in raw terminal mode via `set_raw_mode` / `restore_termios` (libc `tcsetattr`). An `AsyncStdin` wrapper (`AsyncFd<StdinRawFd>`) registers fd 0 once with tokio's epoll reactor and serves both the raw editor and cooked-mode tool-approval prompts sequentially. `InputLine` holds the character buffer and cursor; `InputState` wraps it with a navigable `Vec<String>` history. `read_key` parses escape sequences (CSI, SS3) with 30 ms inter-byte timeouts. `render_input_multiline` redraws the word-wrapped input area on every keystroke: the area grows dynamically from 1 to a maximum of 5 rows as the buffer length exceeds the available width, consuming rows from the scroll region upward; `resize_input_area` handles the scroll-region resize (`setup_scroll_region_n` / `draw_input_frame_n`), clearing any rows that transition between scroll-region and input-area roles; `collapse_input_area` restores the 1-row layout before returning so callers always see a clean state. The input frame top border displays `user@host` (via `local_user_host()`, same as the chat history query borders) rather than a static label. SIGWINCH is handled inside the input loop so resizes repaint correctly while the user is typing. Pane-selection prompts (`offer_no_sibling_options`, `pick_sibling_pane`) use `sync_read_line()` which temporarily clears `O_NONBLOCK` on fd 0 before the synchronous read and restores it afterward, preventing the prompt from racing past immediately. Supported: ←/→ cursor, ↑/↓ history, Home/End, Ctrl+A/E/K/U, Backspace, Delete, Ctrl+C (clear line), Ctrl+D (delete-forward or EOF on empty).

### 2.2 IPC Layer

- **Protocol**: Newline-delimited JSON over a Unix Domain Socket at `/tmp/daemoneye.sock`.
- **Request types**:
  - `Ping` — liveness check.
  - `Shutdown` — graceful stop.
  - `Ask { query, tmux_pane, session_id }` — start or continue a conversation turn.
  - `ToolCallResponse { id, approved }` — user's approval decision for a proposed command.
  - `CredentialResponse { id, credential }` — user-supplied sudo password for an approved background command, sent in response to `CredentialPrompt`.
  - `ScriptWriteResponse { id, approved }` — user's approval decision for a proposed script write, sent in response to `ScriptWritePrompt`.
  - `RunbookWriteResponse { id, approved }` — user's approval decision for a proposed runbook write, sent in response to `RunbookWritePrompt`.
  - `RunbookDeleteResponse { id, approved }` — user's approval decision for a proposed runbook deletion, sent in response to `RunbookDeletePrompt`.
- **Response types**:
  - `Ok` — signals successful completion of a turn.
  - `Error(String)` — error from the daemon or AI provider.
  - `SessionInfo { message_count }` — sent once before streaming; carries prior turn count.
  - `Token(String)` — streaming AI response token.
  - `ToolCallPrompt { id, command, background }` — daemon requests user approval for a command.
  - `CredentialPrompt { id, prompt }` — daemon requests the sudo password for an approved background command; `prompt` is a human-readable message (e.g. `"[sudo] password required for: sudo apt upgrade"`).
  - `SystemMsg(String)` — daemon notification (e.g., sudo prompt detected, pane switch). Rendered in the chat interface with an amber ⚙ prefix.
  - `ToolResult(String)` — captured output of an approved command, sent before the AI continues. Rendered as a dimmed bordered panel (capped at 10 rows).
  - `ScriptWritePrompt { id, script_name, content }` — daemon shows full script content to user before writing; requires `ScriptWriteResponse`.
  - `RunbookWritePrompt { id, runbook_name, content }` — daemon shows full runbook content to user before writing; requires `RunbookWriteResponse`.
  - `RunbookDeletePrompt { id, runbook_name, active_jobs }` — daemon warns if active scheduled jobs reference the runbook before deletion; requires `RunbookDeleteResponse`.
  - `ScheduleList { jobs: Vec<ScheduleListItem> }` — list of scheduled jobs with id, name, kind, action, status, last_run, next_run.
  - `ScriptList { scripts: Vec<ScriptListItem> }` — list of scripts in `~/.daemoneye/scripts/` with name and size.
  - `RunbookList { runbooks: Vec<RunbookListItem> }` — list of runbooks in `~/.daemoneye/runbooks/` with name and tags.
  - `MemoryList { entries: Vec<MemoryListItem> }` — list of memory entries with category and key.
  - `UsageUpdate { prompt_tokens: u32 }` — sent once per completed AI turn (both final-answer and tool-call paths), immediately before `Ok`. Carries the `prompt_tokens` count from the last API call so the CLI can display an accurate context-budget indicator in the user query box.
- Each client connection handles exactly one request/response lifecycle. `Ask` connections receive a token stream (interleaved with zero or more tool-call approval round-trips) terminated by `Ok` or `Error`.

### 2.3 DaemonEye Background Daemon

- **AI Agent Engine** (`daemon/server.rs` & `daemon/executor.rs`): Orchestrates the full request lifecycle. `server.rs` handles the IPC connection loop, loads session history, builds the per-turn prompt, and streams AI events. `executor.rs` processes the resulting tool calls, handling user approval gates and dispatching background/foreground execution. Approval gate logic (send prompt → await response → timeout/deny/approve → log) is centralized in `prompt_and_await_approval()`, used by both the foreground and background arms.

  **Prompt construction is split by turn:**
  - *First turn*: Full host context (`## Host Context`, `## Execution Context`, session memory block) + fresh terminal snapshot from `cache.get_labeled_context()` (`## Terminal Session`). `session_summary` is computed only here.
  - *Subsequent turns*: Token-budget note only (`[Token Budget] Context at Xk / Yk tokens (Z% used) …`) + the user's query. No terminal snapshot is included — the AI calls `get_terminal_context` when it needs one.

  After every AI turn `server.rs` sends `Response::UsageUpdate { prompt_tokens }` to the client and persists the value in `SessionEntry.last_prompt_tokens` so budget warnings in the next turn's prompt are accurate.
- **Execution Context Detector** (`daemon/session.rs`): On the first turn of each session, detects the daemon's local hostname (via `/proc/sys/kernel/hostname`) and whether the user's tmux pane is running `ssh` or `mosh` (via `tmux display-message #{pane_current_command}`). Injects a structured `## Execution Context` block into the prompt so the AI knows which machine each execution mode targets.
- **Dual Command Execution** (`daemon/executor.rs` & `daemon/background.rs`):
  - *Background mode* (`background=true`): Handled by `daemon/background.rs`. Runs in a dedicated tmux window (`de-bg-<session_name>-<YYYYMMDDHHMMSS>-<id_short>`) on the daemon host. The shell is detected via `pane_current_command` immediately after window creation to select the correct exit-code variable (`$?` for POSIX shells; `$status` for fish/csh/tcsh). The command is wrapped as `cmd; __de_ec=$?; daemoneye notify complete <pane_id> $__de_ec <session>` (fish: `set __de_ec $status`) — **no** `; exit` — so the shell stays alive after the command finishes, keeping the window available for follow-up commands. `run_background_in_window` returns immediately with the pane ID; a background tokio task monitors completion via two paths: (primary) the command wrapper calls `daemoneye notify complete` via IPC, which broadcasts `(pane_id, exit_code)` on `COMPLETE_TX` — window persists for follow-up commands, `exit_code` updated in `bg_windows`; (fallback) the shell itself crashes or exits, `pane-died` hook fires and broadcasts the pane ID on `BG_DONE_TX`, exit code recovered via `pane_dead_status` — window is GC-killed and entry removed from `bg_windows`. On either path the pane scrollback is archived to `~/.daemoneye/pane_logs/<win_name>.log` and a `[Background Task Completed]` `Message` is injected into the session history with the exit code, masked output, and whether the pane is still open. Up to 5 background windows are tracked in `SessionEntry.bg_windows`; the oldest completed window is evicted (killed) when the cap is reached; all windows are killed by `cleanup_bg_windows()` when the session expires. If the command contains `sudo`, the daemon first sends a `CredentialPrompt` IPC response; the client reads the password with echo disabled and returns it via `CredentialResponse`; the credential is injected synchronously (before `run_background_in_window` returns) after detecting the sudo password prompt in the pane. The credential is never logged or transmitted to the AI.
  - *Foreground mode* (`background=false`): Injects the command into the user's active tmux pane via `tmux send-keys`. To detect completion cleanly, the daemon dynamically attaches a temporary `pane-title-changed` hook to the target pane. This hook calls `daemoneye notify activity` when the shell updates the terminal title. The daemon waits on an event-driven IPC channel (`fg_rx.recv()`) for this notification, falling back to a slow poll if the shell does not support dynamic titles. For sudo commands, `pane_current_command` is polled at 100 ms intervals to detect whether a password prompt appears; if detected, a `SystemMsg` notification is sent and the working pane is focused.
- **Passive Pane Monitoring** (`daemon/server.rs` & `daemon/background.rs`): The daemon installs a global `pane-died` hook and a per-session `alert-bell` hook at startup via `tmux set-hook`, using `#{pane_id}` as a tmux format variable so the hook fires for every pane in the session. Session names are shell-escaped via `shell_escape_arg()` (`daemon/utils.rs`) before embedding in the `run-shell` hook string to handle names containing `\`, `"`, `$`, or `` ` ``. The hook command is `run-shell -b '<binary> notify activity #{pane_id} 0 "<session>"'` using the absolute path of the running binary. The `NotifyActivity` IPC handler (in `server.rs`) broadcasts the pane ID on `BG_DONE_TX`, waking the background completion monitor's fallback path. The `NotifyComplete` IPC handler broadcasts `(pane_id, exit_code)` on `COMPLETE_TX`, waking the primary completion path. For `watch_pane`, a temporary `pane-title-changed[@de_wp_N]` hook is installed per invocation; the spawned monitor task races the hook signal against a 500 ms `pane_current_command` poll — when the foreground process returns to a known shell name, the command is considered done; a `[Watch Pane Complete]` or `[Watch Pane Timeout]` message is injected into the session history and a `tmux display-message` overlay is shown. For background job completion, the monitoring task in `run_background_in_window` selects between `COMPLETE_TX` (primary, carries exit code directly) and `BG_DONE_TX` (fallback, exit code recovered via `pane_dead_status`), then handles capture, archival, `[Background Task Completed]` session history injection, `tmux display-message` status banner, and GC window cleanup (fallback path) or window persistence (primary path). `notify_job_completion()` (in `background.rs`) is used exclusively by scheduled/watchdog jobs.
- **Event Logger** (`daemon/utils.rs`): Appends structured JSON records to `~/.daemoneye/events.jsonl` for every tool call (approved, denied, timed out), lifecycle event, and AI interaction.
- **Multi-Turn Session Store**: An in-memory `HashMap<session_id, SessionEntry>` shared across connections. Entries are pruned after 30 minutes of inactivity. History is bounded to 40 messages (oldest tail + first message retained) to keep context windows manageable. Session files are written append-only via `append_session_message()` on the hot path; `write_session_file()` (full rewrite) is reserved for post-`trim_history` compaction.
- **tmux Interoperability Layer** (`tmux/`): Executes `tmux capture-pane`, `tmux send-keys`, `tmux list-panes`, `tmux resize-pane`, and `tmux display-message` to read and write to the user's terminal. Key query functions: `pane_current_command` (foreground process name), `query_pane_width` / `query_window_width` (column counts for accurate header rendering), `resize_pane_width` (auto-resize chat pane to 40% of window width on startup), `list_panes_detailed()` (single tab-delimited `list-panes -a -F` call that fetches session name, window name, pane ID, command, path, title, dead, dead_status, scroll position, history size, copy-mode flag, and synchronized flag for every pane in one subprocess — replaces the former `list-panes` + N×`pane_current_command` pattern), `session_environment()` (calls `tmux show-environment` and filters the output against a 20-key allowlist — AWS_PROFILE, AWS_DEFAULT_REGION, AWS_REGION, KUBECONFIG, KUBE_CONTEXT, KUBECTL_CONTEXT, VAULT_ADDR, DOCKER_HOST, DOCKER_CONTEXT, ENVIRONMENT, APP_ENV, NODE_ENV, RAILS_ENV, RACK_ENV, VIRTUAL_ENV, CONDA_DEFAULT_ENV, GOPATH, GOENV, JAVA_HOME, LANG/LC_ALL — returning a key→value map), `pane_dead_status()` (queries `#{pane_dead}\t#{pane_dead_status}` via `display-message` for a specific pane; returns 124 when the pane status is also unavailable, following the POSIX timeout-exit-code convention).
- **Session Cache** (`tmux/cache.rs`): A background task that polls every 2 seconds, captures all panes in the monitored session, and maintains a summarized snapshot. Used as a fallback when the client's specific pane is not available. `PaneState` carries `current_path` (shell CWD via `#{pane_current_path}`), `pane_title` (OSC terminal title via `#{pane_title}`), `scroll_position`, `history_size`, `in_copy_mode`, `synchronized`, `window_name` (the tmux window the pane belongs to), `dead` (true when the pane's foreground process has exited), and `dead_status` (the exit code when `dead` is true). `SessionCache` carries an `environment` map (allowlisted key→value pairs from `session_environment()`) that is refreshed each poll cycle, along with window topology data collected via `tmux list-windows` (`window_id`, `window_name`, `pane_count`, `active`, `zoomed`, and `last_active`). `get_labeled_context()` prepends a `[SESSION ENVIRONMENT]` block when environment vars are present, a `[SESSION TOPOLOGY]` block showing window layout (including IDs and last active flags), appends `| cwd: /path` to `[ACTIVE PANE]` labels, includes the path and title in `[BACKGROUND PANE]` summary lines, and appends `[dead: N]` when the pane's foreground process has exited with code N. Called on the first turn of each conversation and by the `get_terminal_context` tool handler in `executor.rs` — not on every subsequent turn. `refresh()` uses a single `list_panes_detailed()` call instead of N separate subprocesses.
- **System Context Collector** (`sys_context.rs`): Runs once per daemon lifetime (via `OnceLock`). Captures OS release, uptime, memory, load average, top CPU processes, shell environment (curated safe variables only), and shell history. Prepended to the AI context on the first turn of each conversation.
- **Security & Data Masking Filter** (`ai/filter.rs`): Applied to all terminal context and user queries before transmission. Built-in patterns cover: AWS access key IDs, PEM private key blocks, GCP service-account JSON `"private_key"` fields, JWT bearer tokens, GitHub PATs (classic and fine-grained), database/broker connection URLs with embedded credentials, password/token/API-key assignments, URL query-param secrets, credit card numbers, and SSNs. Patterns are compiled once at daemon startup via `init_masking()`, which also incorporates any `extra_patterns` the user has added to `config.toml` — built-in patterns cannot be disabled. 16 unit tests cover the full pattern set.
- **Prompt Engine Manager** (`config.rs`): Loads named system prompts from `~/.daemoneye/prompts/<name>.toml`. Falls back to the compiled-in SRE prompt if no file is found. The built-in SRE prompt (compiled via `include_str!` from `assets/prompts/sre.toml`) instructs the AI that the terminal snapshot is not automatically included and to call `get_terminal_context` when needed, in addition to the `## Command Execution Modes` section. `config.toml` also carries a `[masking]` section (`MaskingConfig`) with an optional `extra_patterns` list. `AiConfig::context_window()` maps the configured model name to its context-window token limit (claude: 200k; gemini-1.5-pro: 2M; other gemini: 1M; gpt-4o/turbo: 128k; default: 128k) — used by both the server-side budget warning and the CLI context-budget display.
- **LLM API Client** (`ai/mod.rs`, `ai/tools.rs`, `ai/backends/`): Implements `AiClient` trait for Anthropic, OpenAI, and Gemini with SSE streaming. Uses a process-wide `reqwest::Client` for connection reuse. Emits events over an unbounded channel for all supported tools: `Token`, `ToolCall` (command execution), `ScheduleCommand`, `ListSchedules`, `CancelSchedule`, `WriteScript`, `ListScripts`, `ReadScript`, `WriteRunbook`, `DeleteRunbook`, `ReadRunbook`, `ListRunbooks`, `AddMemory`, `DeleteMemory`, `ReadMemory`, `ListMemories`, `SearchRepository`, `GetTerminalContext`, `Error`, and `Done`. Tool definitions and `dispatch_tool_event()` are in `ai/tools.rs`; Gemini definitions are inline in `ai/backends/gemini.rs`. The Gemini backend extracts `thoughtSignature` from each `functionCall` part during streaming and round-trips it back in `convert_messages` history; this is required by Gemini 2.5 thinking models for multi-turn tool use. Gemini thinking models occasionally emit Python-style function call syntax (e.g. `print(default_api.run_terminal_command(background = false, command = "...", target_pane = None))`) instead of a structured `functionCall` JSON block; the API signals this with `finishReason: MALFORMED_FUNCTION_CALL`. `parse_malformed_gemini_call()` recovers by extracting `command` and `background` via two independent regexes that handle any argument order, both single- and double-quoted values, and optional spaces around `=`.
- **Schedule Store** (`scheduler.rs`): Thread-safe, file-backed store for scheduled jobs. Persistence is atomic: writes go to `.tmp` then rename over `~/.daemoneye/schedules.json`. Provides `add`, `cancel`, `list`, `take_due`, `mark_done` operations.
- **Scripts Module** (`scripts.rs`): Manages `~/.daemoneye/scripts/` — executable scripts (chmod 700). Provides `list_scripts`, `write_script`, `read_script`, `resolve_script` operations.
- **Runbook Module** (`runbook.rs`): Manages markdown runbook files in `~/.daemoneye/runbooks/<name>.md`. Parses YAML-style frontmatter to extract `tags: [...]` and `memories: [...]` fields. Validates content on write (requires `# Runbook:` heading and `## Alert Criteria` section). CRUD operations — `load_runbook`, `write_runbook` (approval-gated), `delete_runbook` (approval-gated, warns if active jobs reference it), `list_runbooks` — are exposed as AI tools. `watchdog_system_prompt()` loads any knowledge memory keys listed in `memories:` from `memory/knowledge/` and injects them as a `## Runbook Memory Context` block in the watchdog AI prompt.
- **Memory Module** (`memory.rs`): Provides persistent key-value storage under `~/.daemoneye/memory/` in three categories: `session/` (loaded into every AI turn as a `## Persistent Memory` block, capped at 32 KB), `knowledge/` (loaded on-demand by runbooks or `read_memory` tool), and `incidents/` (historical records, searchable only). CRUD operations — `add_memory`, `delete_memory`, `read_memory`, `list_memories` — are exposed as AI tools without approval gates. `load_session_memory_block()` applies the sensitive-data masking filter and is called by `server.rs` on the first turn of every conversation.
- **Search Module** (`search.rs`): Keyword search across all knowledge-base directories. `search_repository(query, kind, context_lines)` searches `runbooks/`, `scripts/`, `memory/{session,knowledge,incidents}/`, and the last 10,000 lines of `events.jsonl` depending on `kind` (`"runbooks"` \| `"scripts"` \| `"memory"` \| `"events"` \| `"all"`). Matches are case-insensitive; filenames are matched in addition to content. Results are capped at 50 and formatted with line numbers and surrounding context. Exposed as the `search_repository` AI tool.

### 2.4 Knowledge System

The knowledge system provides three inter-related persistence layers exposed to the AI as tools:

**Runbooks** (`runbook.rs`, `~/.daemoneye/runbooks/`): Markdown files with YAML-style frontmatter. Serve as watchdog procedures and environment-specific reference docs. Standard format:
```markdown
---
tags: [disk, storage]
memories: [disk_thresholds]
---
# Runbook: disk-check

## Purpose
…

## Alert Criteria
…

## Remediation Steps
…

## Notes
…
```
The AI uses `write_runbook` (approval-gated), `read_runbook`, `list_runbooks`, and `delete_runbook` (approval-gated). Before writing a new runbook the AI is instructed to call `list_runbooks` to avoid duplicates.

**Memory** (`memory.rs`, `~/.daemoneye/memory/`): Three-tier persistent key-value store. Each entry is a `.md` file in the appropriate category subdirectory:
- `session/` — User preferences and recurring environment notes. Automatically loaded into every AI turn (via `load_session_memory_block()`), capped at 32 KB with masking applied.
- `knowledge/` — Named service configs, host quirks, port tables. Loaded on-demand by watchdog runbooks (`memories:` frontmatter) or the `read_memory` tool.
- `incidents/` — Historical incident records. Never auto-loaded; discovered via `search_repository`.

**Search** (`search.rs`): Cross-corpus keyword search. `search_repository(query, kind)` covers runbooks, scripts, all three memory tiers, and the event log. Results are grouped by file, annotated with line numbers and context, and capped at 50 matches.

**Session memory injection**: On the first turn of every conversation `server.rs` calls `load_session_memory_block()` and injects its output between the `## Execution Context` and `## Terminal Session` blocks. When no session memories exist the block is empty and the prompt format is unchanged.

### 2.5 Daemon Lifecycle

- **Startup**: `main()` is a plain synchronous function. For `daemoneye daemon` (without `--console`), `libc::fork()` is called *before* the tokio runtime starts — the parent prints the child PID and exits; the child calls `libc::setsid()` and redirects stdin from `/dev/null`, then builds the tokio runtime. Inside the runtime: validates the API key, calls `init_masking()` with any user-defined `extra_patterns` to compile the masking pattern set, detects or creates the monitored tmux session, starts the cache poller and session cleanup tasks, checks for an already-running daemon (ping probe), then binds the Unix socket. If `[webhook] enabled = true` in `config.toml`, the webhook HTTP server is spawned as an additional tokio task. All mutex lock sites use `.unwrap_or_else(|e| e.into_inner())` to recover from a poisoned lock rather than panicking. If the daemon created a new tmux session, it automatically opens the AI chat pane using `tmux split-window` with the current binary's absolute path.
- **Logging**: All output (`println!`/`eprintln!`) is redirected to `~/.daemoneye/daemon.log` (or `--log-file FILE`) via `dup2` at startup. Use `--console` to keep output on the terminal. View live with `daemoneye logs`.
- **Event log**: Written to `~/.daemoneye/events.jsonl` by default.
- **Shutdown**: `daemoneye stop` sends a `Shutdown` IPC request — the daemon responds `Ok`, removes the socket file, and exits. SIGTERM and SIGINT are also handled gracefully via `tokio::signal::unix`, removing the socket file before exit.

### 2.6 Scheduler and Watchdog

The scheduler runs as a background tokio task that polls `ScheduleStore::take_due()` every second. When a job fires, `run_scheduled_job()` is spawned:

1. **Action resolution**: `ActionOn::Script(name)` resolves to a full path via `scripts::resolve_script()`; `ActionOn::Command(cmd)` is used directly; `ActionOn::Alert` emits a `SystemMsg` and fires the notification hook without running any command.
2. **Execution**: A dedicated tmux window (`de-sched-<YYYYMMDDHHMMSS>-<id_short>`) is created with `tmux::create_job_window()`. The command is sent via `tmux send-keys` with `; exit $?` appended. `remain-on-exit on` is set so the pane stays alive after the process exits. The daemon subscribes to `BG_DONE_TX` via `bg_done_subscribe()` and waits for the `notify activity` IPC hook to fire (up to 300 s).
3. **Completion notifications**: `notify_job_completion()` is called after the job exits. It archives the pane scrollback to `pane_logs/` and sends a `SystemMsg` notification. Per FR-1.2.10: the window is killed immediately on success; on failure it is left open indefinitely for user inspection via `daemoneye schedule windows`.
4. **Watchdog AI analysis**: If the job has a `runbook` set, the captured output is passed to the configured LLM using `watchdog_system_prompt()` built from the runbook's context. If the AI response contains "ALERT", a `SystemMsg` is broadcast to connected clients and `fire_notification()` is called.
5. **Rescheduling**: `Every` jobs have their `next_run` advanced by `interval_secs` and transition back to `Pending`; `Once` jobs remain `Succeeded`/`Failed`.
6. **Notification hook**: `fire_notification()` runs the user-configured `[notifications] on_alert` shell command (from `config.toml`) with `$DAEMONEYE_JOB` and `$DAEMONEYE_MSG` environment variables.

### 2.7 Scripts Directory

`~/.daemoneye/scripts/` holds executable scripts managed by the daemon. Key properties:

- All scripts are written with `chmod 700` (owner-only, no group/other access).
- The AI can **write** scripts via the `write_script` tool, which triggers a `ScriptWritePrompt` IPC round-trip: the client displays the full script content and prompts the user to approve or deny. The daemon only writes the file on approval.
- The AI can **list** and **read** scripts without approval (read-only operations).
- Scripts can be referenced by scheduled jobs (`ActionOn::Script(name)`) and are resolved to their full path at job execution time.
- Script names are validated to reject path traversal (no `/`, `\0`, `.`, or `..`).

### 2.8 Webhook Ingestion

The webhook server (`webhook.rs`) is an optional axum HTTP server spawned as a daemon-side tokio task when `[webhook] enabled = true` in `config.toml`. It is disabled by default.

**HTTP server**: Listens on `0.0.0.0:<port>` (default 9393). Two routes:
- `POST /webhook` — alert ingestion
- `GET /health` — liveness probe (returns `"ok"`)

**Authentication**: When `[webhook] secret` is non-empty, every `POST /webhook` request must include `Authorization: Bearer <secret>`. Requests without a valid token are rejected with `401 Unauthorized`.

**Payload format detection and parsers** (`parse_payload()`):
1. **Alertmanager / Grafana unified** — detected by a top-level `"alerts"` array. Extracts per-alert `labels` (including `alertname`, `severity`), `annotations.summary`, `annotations.description`, `fingerprint` field (or SHA of sorted labels), and `status`.
2. **Grafana legacy** — detected by a top-level `"state"` string without `"alerts"`. Maps `ruleName`/`title` → alert name, `message` → description, `"ok"` → Resolved.
3. **Generic fallback** — tries `alertname`/`name`/`title` for the name, `severity`/`level`/`priority` for severity, `summary`/`message` for summary. If nothing matches, the full JSON body is used as the description.

**Processing pipeline** (runs asynchronously; handler returns `200 OK` immediately):
1. **Deduplication** — alert fingerprint checked against `WebhookState.dedup` (a `Mutex<HashMap<fingerprint, last_seen_secs>>`). Suppressed if seen within `dedup_window_secs`. Fingerprint comes from the Alertmanager payload or is computed from sorted `key=value` label pairs.
2. **Masking** — `mask_sensitive()` applied to `summary` and `description` before any further use.
3. **Formatting** — human-readable single or multi-line message assembled from status, name, summary, description, and source.
4. **Event log** — `log_event("webhook_alert", {...})` appends a record to `events.jsonl`.
5. **Session injection** — `inject_into_sessions()` appends a `[Webhook Alert]` `Message` to every active session's JSONL file via `append_session_message()`.
6. **Chat pane notification** — `notify_chat_panes()` calls `tmux display-message -d 8000 -t <chat_pane> "<first line>"` for every active chat pane.
7. **Severity gate** — if `severity_rank(alert.severity) >= severity_rank(config.severity_threshold)`, `fire_notification()` is called (runs `[notifications] on_alert`). If `auto_analyze = true`, runbook analysis is also triggered.

**Runbook auto-analysis** (rate-limited per alert name within `dedup_window_secs`):
- `find_runbook_for_alert()` tries kebab-case, lower-case, and exact-case variants of the alert name (e.g. `"HighDiskUsage"` → `"high-disk-usage"`, `"highdiskusage"`, `"HighDiskUsage"`).
- If a runbook is found, `watchdog_system_prompt()` builds the watchdog prompt and the configured LLM analyses the formatted alert message.
- If the AI response contains `"ALERT"`, the analysis is injected into all sessions, displayed in chat panes, and logged to `events.jsonl` as `"webhook_analysis"`.

## 3. Data Flow Example: Troubleshooting an Error

1. The user encounters a daemon failure in their active tmux pane.
2. The user runs `daemoneye ask "why did nginx crash?"` (or presses the tmux keybinding to open `daemoneye chat`).
3. The CLI client reads `$TMUX_PANE` from the environment and sends an `Ask` request over the Unix socket.
4. On the **first turn**, the daemon captures the last 200 lines from the client's pane via `get_labeled_context()`, runs the sensitive-data filter, and fetches the cached system context (or collects it fresh on the first-ever request). Background pane summaries include each pane's current working directory, OSC terminal title, synchronized-input flag, and a `[dead: N]` annotation. High-signal tmux session environment variables are prepended as a `[SESSION ENVIRONMENT]` section. On **subsequent turns** the terminal snapshot is omitted from the user message — the AI calls `get_terminal_context` when it needs a fresh view of the screen.
5. The **Prompt Manager** supplies the SRE system prompt; the combined host context + terminal snapshot + user query is sent to the configured LLM API.
6. The API client streams tokens back; the daemon forwards each as a `Token` response to the client, which prints them as they arrive.
7. If the LLM invokes a tool call (e.g., `journalctl -u nginx.service`), the daemon sends a `ToolCallPrompt` response. The client displays the command with its execution mode (`daemon · runs silently` or `terminal · visible to you`) and waits up to 60 seconds for the user to approve (`y`) or deny.
8. On approval:
   - *Background*: The daemon creates a dedicated tmux window (`de-bg-<session>-...`), detects the user's shell via `pane_current_command`, and sends the command wrapped with a `DAEMONEYE_EXIT_<id>:$?` marker (keeping the shell alive). `run_background_in_window` returns immediately with the pane ID; a background tokio task races two completion paths: (A) `pane-died` hook → capture output, archive to `pane_logs/`, inject `[Background Task Completed]` into session history, GC-kill the window; (B) exit marker found in scrollback → capture output, archive, inject `[Background Task Completed]`, leave window open for follow-up commands. A `tmux display-message` status banner is sent to the chat pane on either path. If the command requires `sudo`, a `CredentialPrompt` IPC round-trip collects the password first; it is injected synchronously before the function returns.
   - *Foreground*: The daemon dynamically attaches a temporary `pane-title-changed` hook to the target pane and injects the command via `tmux send-keys`. It then waits on an event-driven IPC channel for the hook to fire when the shell title changes, using a slow polling fallback for incompatible shells. If `sudo` is detected during a brief 3 s polling window, a `SystemMsg` switches focus to the working pane for password entry. After completion, the temporary hook is removed and 200 lines are captured from the pane.
   - The execution record is appended to `~/.daemoneye/events.jsonl`.
9. The LLM receives the tool result and continues its response. This loop repeats until the LLM produces a final answer with no further tool calls.
10. The daemon sends `Ok` to signal completion. The conversation history is stored under the session ID for the next turn.
