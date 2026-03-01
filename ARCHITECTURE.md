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
    end

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
  - `CredentialResponse { id, credential }` — user-supplied credential (password, passphrase, PIN) in response to a `CredentialPrompt` from a background PTY command.
  - `ConfirmationResponse { id, accepted }` — user's yes/no decision in response to a `ConfirmationPrompt` (e.g. SSH host-key acceptance).
- **Response types**:
  - `Ok` — signals successful completion of a turn.
  - `Error(String)` — error from the daemon or AI provider.
  - `SessionInfo { message_count }` — sent once before streaming; carries prior turn count.
  - `Token(String)` — streaming AI response token.
  - `ToolCallPrompt { id, command, background }` — daemon requests user approval for a command.
  - `CredentialPrompt { id, prompt }` — background PTY command is waiting for a credential (sudo password, SSH passphrase, GPG key PIN, etc.). The client prompts the user with echo disabled and returns a `CredentialResponse`.
  - `ConfirmationPrompt { id, message }` — background PTY command is waiting for a yes/no decision (e.g. SSH host-key fingerprint). The client displays the message and returns a `ConfirmationResponse`.
  - `SystemMsg(String)` — daemon notification (e.g., credential prompt detected in foreground pane, pane switch). Rendered in the chat interface with an amber ⚙ prefix.
  - `ToolResult(String)` — captured output of an approved command, sent before the AI continues. Rendered as a dimmed bordered panel (capped at 10 rows).
- Each client connection handles exactly one request/response lifecycle. `Ask` connections receive a token stream (interleaved with zero or more tool-call approval round-trips and zero or more credential/confirmation round-trips) terminated by `Ok` or `Error`.

### 2.3 DaemonEye Background Daemon

- **AI Agent Engine** (`daemon.rs`): Orchestrates the full request lifecycle — loads session history, builds the prompt (host context + execution context on first turn, terminal snapshot on subsequent turns), streams AI events, handles tool call approval and execution, and persists conversation history.
- **Execution Context Detector** (`daemon.rs`): On the first turn of each session, detects the daemon's local hostname (via `/proc/sys/kernel/hostname`) and whether the user's tmux pane is running `ssh` or `mosh` (via `tmux display-message #{pane_current_command}`). Injects a structured `## Execution Context` block into the prompt so the AI knows which machine each execution mode targets.
- **PTY Executor** (`pty_exec/mod.rs`, `pty_exec/prompt.rs`): Provides `run_pty_command`, the shared substrate for background command execution. Allocates a PTY master/slave pair via `libc::openpty`; spawns the child with `tokio::process::Command::pre_exec` which calls `setsid()`, attaches the slave as the controlling terminal (`TIOCSCTTY`), redirects stdin/stdout/stderr to the slave fd, and sets conservative resource limits (`RLIMIT_AS` = 512 MiB, `RLIMIT_NOFILE` = 256). The parent reads the master fd via `tokio::io::unix::AsyncFd` in a non-blocking loop. `strip_ansi()` (also used by the foreground wait path) strips CSI and SS3 escape sequences and bare carriage returns from output before accumulating it. A `PromptDetector` scans the last 5 lines of accumulated output against two pattern sets: *credential* prompts (sudo, su, SSH password, GPG passphrase, PIN) and *confirmation* prompts (SSH host-key, generic yes/no). On detection, the daemon sends a `CredentialPrompt` or `ConfirmationPrompt` IPC response; the client reads the user's answer and returns a `CredentialResponse` or `ConfirmationResponse`; the answer is written directly to the PTY master fd. A `last_prompt` deduplication key prevents re-triggering on the same prompt text across successive reads while still supporting wrong-password re-prompts (the key is cleared only when the prompt disappears from output). After three failed credential attempts `detector.exhausted()` kills the child and returns an error. A configurable timeout (default 30 s, paused while awaiting user input) sends `SIGKILL` and returns a timeout message if the command hangs.
- **Dual Command Execution** (`daemon.rs`):
  - *Background mode* (`background=true`): Delegates to `pty_exec::run_pty_command`. The PTY executor handles all interactive prompts (credentials, confirmations) via IPC round-trips with the client. Exit codes are classified and appended to the output summary returned to the AI. Sensitive output is passed through `mask_sensitive` before being stored in the conversation.
  - *Foreground mode* (`background=false`): Injects the command into the user's active tmux pane via `tmux send-keys`. Completion is detected by polling `pane_current_command` until it returns to the pre-command idle shell value (`idle_cmd`) AND two consecutive identical `capture-pane` snapshots confirm output has stabilised (the two-tick stability check prevents false-positive completion when `idle_cmd` matches a subshell). A unified `PromptDetector` is applied to each `capture-pane` snapshot, covering all credential and confirmation patterns (not just sudo). When a prompt is detected: a `SystemMsg` is sent to the chat pane; focus switches to the working pane via `tmux select-pane`; `prompt_active_cmd` is set to the current `pane_current_command` value. Resolution is detected when `pane_current_command` changes from `prompt_active_cmd` — this correctly handles wrong-password re-prompts (sudo stays running, so the value doesn't change) while immediately detecting success, cancellation, or max-failure (process exits, value changes). On resolution, focus switches back to the chat pane and a `SystemMsg` instructs the AI to inspect the tool result output for success or failure. The active-wait timeout is paused while a prompt is pending; a 120-second wall-clock hard limit prevents infinite hangs regardless of outcome.
- **Command Audit Logger** (`daemon.rs`): Appends a single-line structured record to `~/.daemoneye/commands.log` (or a user-specified path) for every tool call — approved, denied, or timed out. Each line includes timestamp, session ID, mode, pane, status, command, and a 200-character output excerpt. Logging can be disabled with `--no-command-log`.
- **Multi-Turn Session Store**: An in-memory `HashMap<session_id, SessionEntry>` shared across connections. Entries are pruned after 30 minutes of inactivity. History is bounded to 40 messages (oldest tail + first message retained) to keep context windows manageable.
- **tmux Interoperability Layer** (`tmux/mod.rs`): Executes `tmux capture-pane`, `tmux send-keys`, `tmux list-panes`, `tmux resize-pane`, and `tmux display-message` to read and write to the user's terminal. Key query functions: `pane_current_command` (foreground process name), `query_pane_width` / `query_window_width` (column counts for accurate header rendering), `resize_pane_width` (auto-resize chat pane to 40% of window width on startup).
- **Session Cache** (`tmux/cache.rs`): A background task that polls every 2 seconds, captures all panes in the monitored session, and maintains a summarized snapshot. Used as a fallback when the client's specific pane is not available.
- **System Context Collector** (`sys_context.rs`): Runs once per daemon lifetime (via `OnceLock`). Captures OS release, uptime, memory, load average, top CPU processes, shell environment (curated safe variables only), and shell history. Prepended to the AI context on the first turn of each conversation.
- **Security & Data Masking Filter** (`ai/filter.rs`): Applied to all terminal context and user queries before transmission. Built-in patterns cover: AWS access key IDs, PEM private key blocks, GCP service-account JSON `"private_key"` fields, JWT bearer tokens, GitHub PATs (classic and fine-grained), database/broker connection URLs with embedded credentials, password/token/API-key assignments, URL query-param secrets, credit card numbers, and SSNs. Patterns are compiled once at daemon startup via `init_masking()`, which also incorporates any `extra_patterns` the user has added to `config.toml` — built-in patterns cannot be disabled. 16 unit tests cover the full pattern set.
- **Prompt Engine Manager** (`config.rs`): Loads named system prompts from `~/.daemoneye/prompts/<name>.toml`. Falls back to the compiled-in SRE prompt if no file is found. The built-in SRE prompt includes a `## Command Execution Modes` section that instructs the AI when to use background vs foreground execution. `config.toml` also carries a `[masking]` section (`MaskingConfig`) with an optional `extra_patterns` list of user-defined regex patterns appended to the built-in masking set at daemon startup.
- **LLM API Client** (`ai/client.rs`): Implements `AiClient` trait for Anthropic, OpenAI, and Gemini with SSE streaming. Uses a process-wide `reqwest::Client` for connection reuse. Emits `Token`, `ToolCall`, `Error`, and `Done` events over an unbounded channel. Tool descriptions for all three providers document the daemon-host vs user-pane execution semantics.

### 2.4 Daemon Lifecycle

- **Startup**: `main()` is a plain synchronous function. For `daemoneye daemon` (without `--console`), `libc::fork()` is called *before* the tokio runtime starts — the parent prints the child PID and exits; the child calls `libc::setsid()` and redirects stdin from `/dev/null`, then builds the tokio runtime. Inside the runtime: validates the API key, calls `init_masking()` with any user-defined `extra_patterns` to compile the masking pattern set, detects or creates the monitored tmux session, starts the cache poller and session cleanup tasks, checks for an already-running daemon (ping probe), then binds the Unix socket. All mutex lock sites use `.unwrap_or_else(|e| e.into_inner())` to recover from a poisoned lock rather than panicking. If the daemon created a new tmux session, it automatically opens the AI chat pane using `tmux split-window` with the current binary's absolute path.
- **Logging**: All output (`println!`/`eprintln!`) is redirected to `~/.daemoneye/daemon.log` (or `--log-file FILE`) via `dup2` at startup. Use `--console` to keep output on the terminal. View live with `daemoneye logs`.
- **Command audit log**: Written to `~/.daemoneye/commands.log` by default. Override with `--command-log-file FILE` or disable entirely with `--no-command-log`.
- **Shutdown**: `daemoneye stop` sends a `Shutdown` IPC request — the daemon responds `Ok`, removes the socket file, and exits. SIGTERM and SIGINT are also handled gracefully via `tokio::signal::unix`, removing the socket file before exit.

## 3. Data Flow Example: Troubleshooting an Error

1. The user encounters a daemon failure in their active tmux pane.
2. The user runs `daemoneye ask "why did nginx crash?"` (or presses the tmux keybinding to open `daemoneye chat`).
3. The CLI client reads `$TMUX_PANE` from the environment and sends an `Ask` request over the Unix socket.
4. The daemon captures the last 200 lines from the client's pane, runs the sensitive-data filter, and fetches the cached system context (or collects it fresh on the first-ever request).
5. The **Prompt Manager** supplies the SRE system prompt; the combined host context + terminal snapshot + user query is sent to the configured LLM API.
6. The API client streams tokens back; the daemon forwards each as a `Token` response to the client, which prints them as they arrive.
7. If the LLM invokes a tool call (e.g., `journalctl -u nginx.service`), the daemon sends a `ToolCallPrompt` response. The client displays the command with its execution mode (`daemon · runs silently` or `terminal · visible to you`) and waits up to 60 seconds for the user to approve (`y`) or deny.
8. On approval:
   - *Background*: The daemon runs the command inside a PTY via `pty_exec::run_pty_command`. If the command emits a credential prompt (sudo, SSH, GPG passphrase, etc.), the daemon sends a `CredentialPrompt` response; the client reads the user's input with echo disabled and returns a `CredentialResponse`; the daemon writes the credential directly into the PTY master fd. If the command emits a confirmation prompt (SSH host-key, etc.), the same round-trip happens via `ConfirmationPrompt` / `ConfirmationResponse`. Multiple wrong-password attempts are handled transparently; after three failures the child is killed. On completion, the daemon sends a `ToolResult` with the captured (ANSI-stripped, sensitive-data-masked) output.
   - *Foreground*: The daemon injects the command into the user's tmux pane via `tmux send-keys`. A `PromptDetector` scans each `capture-pane` snapshot during the wait loop. If a credential or confirmation prompt is detected, the daemon sends a `SystemMsg`, switches focus to the user's working pane, and records `prompt_active_cmd`. When `pane_current_command` changes from that recorded value (indicating the prompting process has exited — successfully authenticated, cancelled, or exceeded retries), the daemon switches focus back to the chat pane and sends a `SystemMsg` instructing the AI to inspect the tool result.
   - The execution record is appended to `~/.daemoneye/commands.log`.
9. The LLM receives the tool result and continues its response. This loop repeats until the LLM produces a final answer with no further tool calls.
10. The daemon sends `Ok` to signal completion. The conversation history is stored under the session ID for the next turn.
