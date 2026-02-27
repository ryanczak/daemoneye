# T1000 System Architecture

This document outlines the high-level architecture and core components of the T1000 daemon.

## 1. High-Level Architecture Diagram

```mermaid
graph TD
    subgraph Client ["User Terminal Environment"]
        TmuxClient[User's tmux Session]
        ActivePane[Active Working Pane]
        AIPane[AI Agent Pane / t1000 chat]
    end

    subgraph Daemon ["T1000 Background Daemon"]
        IPCServer[IPC Server\n/tmp/t1000.sock]
        TmuxInterop[tmux Interoperability Layer]
        AgentEngine[AI Agent Engine]
        Filter[Security & Data Masking Filter]
        PromptManager[Prompt Engine Manager]
        APIClient[LLM API Client]
        SessionStore[Multi-Turn Session Store]
        SysContext[System Context Collector]
    end

    subgraph ConfigFiles ["File System (~/.t1000/)"]
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

- **User's tmux Session**: T1000 does not render its own window or emulate a terminal. It operates entirely within the user's pre-existing terminal emulator via `tmux`.
- **Active Working Pane**: The pane where the user is currently working. When T1000 is invoked, it reads the content from this pane and injects approved commands here.
- **AI Agent Pane**: A tmux pane running `t1000 chat` or `t1000 ask`. Streams AI responses and presents tool call approval prompts.

### 2.2 IPC Layer

- **Protocol**: Newline-delimited JSON over a Unix Domain Socket at `/tmp/t1000.sock`.
- **Request types**:
  - `Ping` — liveness check.
  - `Shutdown` — graceful stop.
  - `Ask { query, tmux_pane, session_id }` — start or continue a conversation turn.
  - `ToolCallResponse { id, approved }` — user's approval decision for a proposed command.
  - `SudoPassword { id, password }` — user-supplied sudo password for an approved background command.
- **Response types**:
  - `Ok` — signals successful completion of a turn.
  - `Error(String)` — error from the daemon or AI provider.
  - `SessionInfo { message_count }` — sent once before streaming; carries prior turn count.
  - `Token(String)` — streaming AI response token.
  - `ToolCallPrompt { id, command, background }` — daemon requests user approval for a command.
  - `SudoPrompt { id, command }` — daemon requests the sudo password for an approved background command.
- Each client connection handles exactly one request/response lifecycle. `Ask` connections receive a token stream (interleaved with zero or more tool-call approval round-trips) terminated by `Ok` or `Error`.

### 2.3 T1000 Background Daemon

- **AI Agent Engine** (`daemon.rs`): Orchestrates the full request lifecycle — loads session history, builds the prompt (host context + execution context on first turn, terminal snapshot on subsequent turns), streams AI events, handles tool call approval and execution, and persists conversation history.
- **Execution Context Detector** (`daemon.rs`): On the first turn of each session, detects the daemon's local hostname (via `/proc/sys/kernel/hostname`) and whether the user's tmux pane is running `ssh` or `mosh` (via `tmux display-message #{pane_current_command}`). Injects a structured `## Execution Context` block into the prompt so the AI knows which machine each execution mode targets.
- **Dual Command Execution** (`daemon.rs`):
  - *Background mode* (`background=true`): Runs as a daemon subprocess via `tokio::process::Command`. Output is captured and returned to the AI. Sudo is handled via a `SudoPrompt` / `SudoPassword` IPC round-trip; the password is piped to `sudo -S -p ""` and never stored.
  - *Foreground mode* (`background=false`): Injects the command into the user's active tmux pane via `tmux send-keys`. Sudo commands trigger a `Token` notification so the user knows to type their password in the pane; wait time is extended to 30 seconds.
- **Command Audit Logger** (`daemon.rs`): Appends a single-line structured record to `~/.t1000/commands.log` (or a user-specified path) for every tool call — approved, denied, or timed out. Each line includes timestamp, session ID, mode, pane, status, command, and a 200-character output excerpt. Logging can be disabled with `--no-command-log`.
- **Multi-Turn Session Store**: An in-memory `HashMap<session_id, SessionEntry>` shared across connections. Entries are pruned after 30 minutes of inactivity. History is bounded to 40 messages (oldest tail + first message retained) to keep context windows manageable.
- **tmux Interoperability Layer** (`tmux/mod.rs`): Executes `tmux capture-pane`, `tmux send-keys`, `tmux list-panes`, and `tmux display-message` to read and write to the user's terminal.
- **Session Cache** (`tmux/cache.rs`): A background task that polls every 2 seconds, captures all panes in the monitored session, and maintains a summarized snapshot. Used as a fallback when the client's specific pane is not available.
- **System Context Collector** (`sys_context.rs`): Runs once per daemon lifetime (via `OnceLock`). Captures OS release, uptime, memory, load average, top CPU processes, shell environment (curated safe variables only), and shell history. Prepended to the AI context on the first turn of each conversation.
- **Security & Data Masking Filter** (`ai/filter.rs`): Applied to all terminal context and user queries before transmission. Masks AWS keys, passwords, tokens, secrets, PEM blocks, credit card numbers, and SSNs.
- **Prompt Engine Manager** (`config.rs`): Loads named system prompts from `~/.t1000/prompts/<name>.toml`. Falls back to the compiled-in SRE prompt if no file is found. The built-in SRE prompt includes a `## Command Execution Modes` section that instructs the AI when to use background vs foreground execution.
- **LLM API Client** (`ai/client.rs`): Implements `AiClient` trait for Anthropic, OpenAI, and Gemini with SSE streaming. Uses a process-wide `reqwest::Client` for connection reuse. Emits `Token`, `ToolCall`, `Error`, and `Done` events over an unbounded channel. Tool descriptions for all three providers document the daemon-host vs user-pane execution semantics.

### 2.4 Daemon Lifecycle

- **Startup**: Validates API key, detects or creates the monitored tmux session, starts the cache poller and session cleanup tasks, checks for an already-running daemon (ping probe), then binds the Unix socket. If the daemon created a new tmux session, it automatically opens the AI chat pane using `tmux split-window` with the current binary's absolute path.
- **Logging**: All output (`println!`/`eprintln!`) is redirected to `~/.t1000/daemon.log` (or `--log-file FILE`) via `dup2` at startup. Use `--console` to keep output on the terminal. View live with `t1000 logs`.
- **Command audit log**: Written to `~/.t1000/commands.log` by default. Override with `--command-log-file FILE` or disable entirely with `--no-command-log`.
- **Shutdown**: `t1000 stop` sends a `Shutdown` IPC request — the daemon responds `Ok`, removes the socket file, and exits. SIGTERM and SIGINT are also handled gracefully via `tokio::signal::unix`, removing the socket file before exit.

## 3. Data Flow Example: Troubleshooting an Error

1. The user encounters a daemon failure in their active tmux pane.
2. The user runs `t1000 ask "why did nginx crash?"` (or presses the tmux keybinding to open `t1000 chat`).
3. The CLI client reads `$TMUX_PANE` from the environment and sends an `Ask` request over the Unix socket.
4. The daemon captures the last 200 lines from the client's pane, runs the sensitive-data filter, and fetches the cached system context (or collects it fresh on the first-ever request).
5. The **Prompt Manager** supplies the SRE system prompt; the combined host context + terminal snapshot + user query is sent to the configured LLM API.
6. The API client streams tokens back; the daemon forwards each as a `Token` response to the client, which prints them as they arrive.
7. If the LLM invokes a tool call (e.g., `journalctl -u nginx.service`), the daemon sends a `ToolCallPrompt` response. The client displays the command with its execution mode (`daemon · runs silently` or `terminal · visible to you`) and waits up to 60 seconds for the user to approve (`y`) or deny.
8. On approval:
   - *Background*: The daemon runs the command as a subprocess and captures stdout/stderr. If the command requires `sudo`, the daemon first sends a `SudoPrompt`; the client reads the password with echo disabled and returns it via `SudoPassword`; the daemon runs `sudo -S -p ""` with the password piped to stdin.
   - *Foreground*: The daemon injects the command into the user's tmux pane via `tmux send-keys`. If `sudo` is detected the client is notified to type the password in the pane; the daemon waits up to 30 seconds instead of 3. After waiting, it captures 200 lines from the pane.
   - The execution record is appended to `~/.t1000/commands.log`.
9. The LLM receives the tool result and continues its response. This loop repeats until the LLM produces a final answer with no further tool calls.
10. The daemon sends `Ok` to signal completion. The conversation history is stored under the session ID for the next turn.
