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
        CmdLog[(commands.log)]
        ConfigToml[(config.toml)]
        Scripts[(scripts/)]
        Runbooks[(runbooks/)]
        SchedStore[(schedules.json)]
    end

    Scheduler[(Scheduler Task)]

    Cloud(("Anthropic / OpenAI / Gemini API"))

    %% User Interaction & Terminal I/O
    TmuxClient --- ActivePane
    TmuxClient --- AIPane

    %% IPC
    AIPane -->|Unix Socket JSON| IPCServer
    IPCServer -->|Token stream| AIPane

    %% AI Workflow
    IPCServer --> AgentEngine
    TmuxInterop -->|Capture Buffer & History| AgentEngine
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
```

## 2. Core Components

### 2.1 User Terminal Environment

- **User's tmux Session**: DaemonEye does not render its own window or emulate a terminal. It operates entirely within the user's pre-existing terminal emulator via `tmux`.
- **Active Working Pane**: The pane where the user is currently working. When DaemonEye is invoked, it reads the content from this pane and injects approved commands here.
- **AI Agent Pane** (`client.rs`): A tmux pane running `daemoneye chat` or `daemoneye ask`. On startup the pane is resized to 40% of the window width (minimum 80 columns) using `tmux resize-pane`; `query_pane_width` is then used for accurate header rendering, avoiding a `TIOCGWINSZ` race condition. The chat interface features:
  - Width-adaptive header with session ID; all border calculations use `visual_len()` (strips ANSI escapes, counts Unicode code points) to handle multi-byte box-drawing characters correctly.
  - ANSI syntax highlighting in fenced code blocks: per-language keyword sets, comment styles, string literals, and numeric literals.
  - Inline markdown rendering: bold, italic, and `code` spans; AI prose is tinted bright-white.
  - Session/turn indicator line (dim, full-width separator) printed after each `SessionInfo` response showing turn number and message count.
  - `/clear` in-chat command: generates a new session ID so the next message starts a clean context, prints a dim separator, and updates the header hint.
  - `SystemMsg` responses rendered with an amber ⚙ prefix.
  - `ToolResult` responses rendered as a dimmed bordered panel capped at 10 rows with a truncation indicator.
  - **Session-level command approval**: `ToolCallPrompt` responses display a three-option prompt — `[Y]es` (once), `[A]pprove for session` (session-wide), `[N]o` (deny). Two independent approval classes are tracked in `SessionApproval { regular: bool, sudo: bool }`, stored in `run_chat_inner` and passed into `ask_with_session`. The `command_is_sudo` helper (mirrors `daemon.rs::command_has_sudo` using the same `(?:^|[;&|])\s*sudo\b` regex) classifies each command. Once a class is session-approved, subsequent commands in that class display `✓ auto-approved (session)` without prompting. Approval state resets on `/clear`, `/prompt`, and `/refresh`.
  - **Interactive line editor**: The chat input runs in raw terminal mode via `set_raw_mode` / `restore_termios` (libc `tcsetattr`). An `AsyncStdin` wrapper (`AsyncFd<StdinRawFd>`) registers fd 0 once with tokio's epoll reactor and serves both the raw editor and cooked-mode tool-approval prompts sequentially. `InputLine` holds the character buffer and cursor; `InputState` wraps it with a navigable `Vec<String>` history. `read_key` parses escape sequences (CSI, SS3) with 30 ms inter-byte timeouts. `render_input_row` redraws the input box on every keystroke with a horizontal viewport that scrolls to keep the cursor visible. SIGWINCH is handled inside the input loop so resizes repaint correctly while the user is typing. Supported: ←/→ cursor, ↑/↓ history, Home/End, Ctrl+A/E/K/U, Backspace, Delete, Ctrl+C (clear line), Ctrl+D (delete-forward or EOF on empty).

### 2.2 IPC Layer

- **Protocol**: Newline-delimited JSON over a Unix Domain Socket at `/tmp/daemoneye.sock`.
- **Request types**:
  - `Ping` — liveness check.
  - `Shutdown` — graceful stop.
  - `Ask { query, tmux_pane, session_id }` — start or continue a conversation turn.
  - `ToolCallResponse { id, approved }` — user's approval decision for a proposed command.
  - `CredentialResponse { id, credential }` — user-supplied sudo password for an approved background command, sent in response to `CredentialPrompt`.
  - `ScriptWriteResponse { id, approved }` — user's approval decision for a proposed script write, sent in response to `ScriptWritePrompt`.
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
  - `ScheduleList { jobs: Vec<ScheduleListItem> }` — list of scheduled jobs with id, name, kind, action, status, last_run, next_run.
  - `ScriptList { scripts: Vec<ScriptListItem> }` — list of scripts in `~/.daemoneye/scripts/` with name and size.
- Each client connection handles exactly one request/response lifecycle. `Ask` connections receive a token stream (interleaved with zero or more tool-call approval round-trips) terminated by `Ok` or `Error`.

### 2.3 DaemonEye Background Daemon

- **AI Agent Engine** (`daemon.rs`): Orchestrates the full request lifecycle — loads session history, builds the prompt (host context + execution context on first turn, terminal snapshot on subsequent turns), streams AI events, handles tool call approval and execution, and persists conversation history.
- **Execution Context Detector** (`daemon.rs`): On the first turn of each session, detects the daemon's local hostname (via `/proc/sys/kernel/hostname`) and whether the user's tmux pane is running `ssh` or `mosh` (via `tmux display-message #{pane_current_command}`). Injects a structured `## Execution Context` block into the prompt so the AI knows which machine each execution mode targets.
- **Dual Command Execution** (`daemon.rs`):
  - *Background mode* (`background=true`): Runs in a dedicated tmux window (`de-bg-<id_short>`) on the daemon host. The command is sent via `tmux send-keys` and appended with `; exit $?`. The window is configured with `remain-on-exit on`, and `poll_until_dead()` polls `pane_dead_status` every 200 ms until the process exits (up to 300 s). The exit code is extracted natively from the dead pane, and the window is always killed after output is captured. This gives the command access to the user's full shell environment (PATH, aliases, functions). If the command contains `sudo`, the daemon first sends a `CredentialPrompt` IPC response; the client reads the password with echo disabled and returns it via `CredentialResponse`; the credential is injected into the window via `send-keys` after detecting the sudo password prompt in `capture-pane` output. The credential is never logged or transmitted to the AI. When `poll_until_dead()` times out (e.g., the process was killed or the window died), the daemon falls back to exit code 124 (POSIX timeout convention).
  - *Foreground mode* (`background=false`): Injects the command (unmodified) into the user's active tmux pane via `tmux send-keys`. Completion is detected by watching `/proc/<shell_pid>/task/<shell_pid>/children` — a Linux kernel file that lists the shell's direct child PIDs. The daemon polls this file every 20 ms; when a child PID appears (`saw_child = true`) and then disappears with `pane_current_command` back to the idle shell name, the command is complete. A 100 ms grace-period fallback handles commands that finish before the first poll tick. For sudo commands, `pane_current_command` is polled at 100 ms intervals (up to 3 s, with early exit if the pane returns to idle for NOPASSWD/cached-credential runs) to detect whether a password prompt appears; if one is found, a `SystemMsg` notification is sent and the working pane is focused. The shell PID is obtained via `tmux display-message -p "#{pane_pid}"` before the command is injected.
- **Passive Pane Monitoring** (`daemon.rs`): Driven by the `watch_pane` tool request. Sets `monitor-activity on` for the target background pane and installs a tmux window hook via `alert-activity` that executes `run-shell 'daemoneye notify activity #{pane_id}'`. The daemon handles the `NotifyActivity` IPC request by appending a `[System]` alert directly to the session's in-memory message history and flashing an OS-level overlay onto the user's `chat_pane` utilizing `tmux display-message`. Watch polling loops run asynchronously and non-blocking.
- **Command Audit Logger** (`daemon.rs`): Appends a single-line structured record to `~/.daemoneye/commands.log` (or a user-specified path) for every tool call — approved, denied, or timed out. Each line includes timestamp, session ID, mode, pane, status, command, and a 200-character output excerpt. Logging can be disabled with `--no-command-log`.
- **Multi-Turn Session Store**: An in-memory `HashMap<session_id, SessionEntry>` shared across connections. Entries are pruned after 30 minutes of inactivity. History is bounded to 40 messages (oldest tail + first message retained) to keep context windows manageable.
- **tmux Interoperability Layer** (`tmux/mod.rs`): Executes `tmux capture-pane`, `tmux send-keys`, `tmux list-panes`, `tmux resize-pane`, and `tmux display-message` to read and write to the user's terminal. Key query functions: `pane_current_command` (foreground process name), `query_pane_width` / `query_window_width` (column counts for accurate header rendering), `resize_pane_width` (auto-resize chat pane to 40% of window width on startup), `list_panes_detailed()` (single tab-delimited `list-panes -s -F` call that fetches command, path, title, dead, and dead_status for every pane in one subprocess — replaces the former `list-panes` + N×`pane_current_command` pattern), `session_environment()` (calls `tmux show-environment` and filters the output against a 20-key allowlist — AWS_PROFILE, AWS_DEFAULT_REGION, AWS_REGION, KUBECONFIG, KUBE_CONTEXT, KUBECTL_CONTEXT, VAULT_ADDR, DOCKER_HOST, DOCKER_CONTEXT, ENVIRONMENT, APP_ENV, NODE_ENV, RAILS_ENV, RACK_ENV, VIRTUAL_ENV, CONDA_DEFAULT_ENV, GOPATH, GOENV, JAVA_HOME, LANG/LC_ALL — returning a key→value map), `pane_dead_status()` (queries `#{pane_dead}\t#{pane_dead_status}` via `display-message` for a specific pane; returns 124 when the pane status is also unavailable, following the POSIX timeout-exit-code convention).
- **Session Cache** (`tmux/cache.rs`): A background task that polls every 2 seconds, captures all panes in the monitored session, and maintains a summarized snapshot. Used as a fallback when the client's specific pane is not available. `PaneState` now carries `current_path` (the shell's CWD via `#{pane_current_path}`) and `pane_title` (the OSC terminal title set by running applications via `#{pane_title}`). `SessionCache` carries an `environment` map (allowlisted key→value pairs from `session_environment()`) that is refreshed each poll cycle, along with window topology data collected via `tmux list-windows` (`window_id`, `window_name`, `pane_count`, `active`, `zoomed`, and `last_active`). `get_labeled_context()` prepends a `[SESSION ENVIRONMENT]` block when environment vars are present, a `[SESSION TOPOLOGY]` block showing window layout (including IDs and last active flags), appends `| cwd: /path` to `[ACTIVE PANE]` labels, and includes the path and title in `[BACKGROUND PANE]` summary lines. `refresh()` uses a single `list_panes_detailed()` call instead of N separate subprocesses.
- **System Context Collector** (`sys_context.rs`): Runs once per daemon lifetime (via `OnceLock`). Captures OS release, uptime, memory, load average, top CPU processes, shell environment (curated safe variables only), and shell history. Prepended to the AI context on the first turn of each conversation.
- **Security & Data Masking Filter** (`ai/filter.rs`): Applied to all terminal context and user queries before transmission. Built-in patterns cover: AWS access key IDs, PEM private key blocks, GCP service-account JSON `"private_key"` fields, JWT bearer tokens, GitHub PATs (classic and fine-grained), database/broker connection URLs with embedded credentials, password/token/API-key assignments, URL query-param secrets, credit card numbers, and SSNs. Patterns are compiled once at daemon startup via `init_masking()`, which also incorporates any `extra_patterns` the user has added to `config.toml` — built-in patterns cannot be disabled. 16 unit tests cover the full pattern set.
- **Prompt Engine Manager** (`config.rs`): Loads named system prompts from `~/.daemoneye/prompts/<name>.toml`. Falls back to the compiled-in SRE prompt if no file is found. The built-in SRE prompt includes a `## Command Execution Modes` section that instructs the AI when to use background vs foreground execution. `config.toml` also carries a `[masking]` section (`MaskingConfig`) with an optional `extra_patterns` list of user-defined regex patterns appended to the built-in masking set at daemon startup.
- **LLM API Client** (`ai/client.rs`): Implements `AiClient` trait for Anthropic, OpenAI, and Gemini with SSE streaming. Uses a process-wide `reqwest::Client` for connection reuse. Emits `Token`, `ToolCall`, `ScheduleCommand`, `ListSchedules`, `CancelSchedule`, `WriteScript`, `ListScripts`, `ReadScript`, `Error`, and `Done` events over an unbounded channel. Tool descriptions for all three providers document the daemon-host vs user-pane execution semantics plus all six scheduler/script tools.
- **Schedule Store** (`scheduler.rs`): Thread-safe, file-backed store for scheduled jobs. Persistence is atomic: writes go to `.tmp` then rename over `~/.daemoneye/schedules.json`. Provides `add`, `cancel`, `list`, `take_due`, `mark_done` operations.
- **Scripts Module** (`scripts.rs`): Manages `~/.daemoneye/scripts/` — executable scripts (chmod 700). Provides `list_scripts`, `write_script`, `read_script`, `resolve_script` operations.
- **Runbook Loader** (`runbook.rs`): Loads TOML runbook files from `~/.daemoneye/runbooks/<name>.toml`. Builds the AI system prompt for watchdog analysis via `watchdog_system_prompt()`.

### 2.4 Daemon Lifecycle

- **Startup**: `main()` is a plain synchronous function. For `daemoneye daemon` (without `--console`), `libc::fork()` is called *before* the tokio runtime starts — the parent prints the child PID and exits; the child calls `libc::setsid()` and redirects stdin from `/dev/null`, then builds the tokio runtime. Inside the runtime: validates the API key, calls `init_masking()` with any user-defined `extra_patterns` to compile the masking pattern set, detects or creates the monitored tmux session, starts the cache poller and session cleanup tasks, checks for an already-running daemon (ping probe), then binds the Unix socket. All mutex lock sites use `.unwrap_or_else(|e| e.into_inner())` to recover from a poisoned lock rather than panicking. If the daemon created a new tmux session, it automatically opens the AI chat pane using `tmux split-window` with the current binary's absolute path.
- **Logging**: All output (`println!`/`eprintln!`) is redirected to `~/.daemoneye/daemon.log` (or `--log-file FILE`) via `dup2` at startup. Use `--console` to keep output on the terminal. View live with `daemoneye logs`.
- **Command audit log**: Written to `~/.daemoneye/commands.log` by default. Override with `--command-log-file FILE` or disable entirely with `--no-command-log`.
- **Shutdown**: `daemoneye stop` sends a `Shutdown` IPC request — the daemon responds `Ok`, removes the socket file, and exits. SIGTERM and SIGINT are also handled gracefully via `tokio::signal::unix`, removing the socket file before exit.

### 2.5 Scheduler and Watchdog

The scheduler runs as a background tokio task that polls `ScheduleStore::take_due()` every second. When a job fires, `run_scheduled_job()` is spawned:

1. **Action resolution**: `ActionOn::Script(name)` resolves to a full path via `scripts::resolve_script()`; `ActionOn::Command(cmd)` is used directly; `ActionOn::Alert` emits a `SystemMsg` and fires the notification hook without running any command.
2. **Execution**: A dedicated tmux window (`de-<id_short>`) is created with `tmux::create_job_window()`. The command is sent via `tmux send-keys` with an `; exit $?` appended. `poll_until_dead()` polls `pane_dead_status` until the pane dies (up to 300 s).
3. **Window lifecycle**: On success the window is killed; on failure the window is **left open** (`de-<id>`) for the user to inspect.
4. **Watchdog AI analysis**: If the job has a `runbook` set, the captured output is passed to the configured LLM using `watchdog_system_prompt()` built from the runbook's context. If the AI response contains "ALERT", a `SystemMsg` is broadcast to connected clients and `fire_notification()` is called.
5. **Rescheduling**: `Every` jobs have their `next_run` advanced by `interval_secs` and transition back to `Pending`; `Once` jobs remain `Succeeded`/`Failed`.
6. **Notification hook**: `fire_notification()` runs the user-configured `[notifications] on_alert` shell command (from `config.toml`) with `$DAEMONEYE_JOB` and `$DAEMONEYE_MSG` environment variables.

### 2.6 Scripts Directory

`~/.daemoneye/scripts/` holds executable scripts managed by the daemon. Key properties:

- All scripts are written with `chmod 700` (owner-only, no group/other access).
- The AI can **write** scripts via the `write_script` tool, which triggers a `ScriptWritePrompt` IPC round-trip: the client displays the full script content and prompts the user to approve or deny. The daemon only writes the file on approval.
- The AI can **list** and **read** scripts without approval (read-only operations).
- Scripts can be referenced by scheduled jobs (`ActionOn::Script(name)`) and are resolved to their full path at job execution time.
- Script names are validated to reject path traversal (no `/`, `\0`, `.`, or `..`).

## 3. Data Flow Example: Troubleshooting an Error

1. The user encounters a daemon failure in their active tmux pane.
2. The user runs `daemoneye ask "why did nginx crash?"` (or presses the tmux keybinding to open `daemoneye chat`).
3. The CLI client reads `$TMUX_PANE` from the environment and sends an `Ask` request over the Unix socket.
4. The daemon captures the last 200 lines from the client's pane, runs the sensitive-data filter, and fetches the cached system context (or collects it fresh on the first-ever request). Background pane summaries now include each pane's current working directory and OSC terminal title. High-signal tmux session environment variables (cloud account, k8s cluster, runtime tier, language runtime, etc.) are prepended to the full context block as a `[SESSION ENVIRONMENT]` section.
5. The **Prompt Manager** supplies the SRE system prompt; the combined host context + terminal snapshot + user query is sent to the configured LLM API.
6. The API client streams tokens back; the daemon forwards each as a `Token` response to the client, which prints them as they arrive.
7. If the LLM invokes a tool call (e.g., `journalctl -u nginx.service`), the daemon sends a `ToolCallPrompt` response. The client displays the command with its execution mode (`daemon · runs silently` or `terminal · visible to you`) and waits up to 60 seconds for the user to approve (`y`) or deny.
8. On approval:
   - *Background*: The daemon creates a dedicated tmux window (`de-bg-<id>`) and sends the command appended with `; exit $?`. It configures the window with `remain-on-exit on` and polls `pane_dead_status` until the process exits, then the window is killed. If the command requires `sudo`, a `CredentialPrompt` IPC round-trip collects the password first; it is injected into the window via `send-keys` after detecting the password prompt in `capture-pane` output.
   - *Foreground*: The daemon injects the command unmodified and polls `/proc/<shell_pid>/task/<shell_pid>/children` every 20 ms to detect when the child process exits (30-second overall timeout). If `sudo` is detected during a brief 3 s polling window, a `SystemMsg` switches focus to the working pane for password entry. After completion, 200 lines are captured from the pane.
   - The execution record is appended to `~/.daemoneye/commands.log`.
9. The LLM receives the tool result and continues its response. This loop repeats until the LLM produces a final answer with no further tool calls.
10. The daemon sends `Ok` to signal completion. The conversation history is stored under the session ID for the next turn.
