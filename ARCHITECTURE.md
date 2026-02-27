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
    ConfigToml -.->|Read| AgentEngine
```

## 2. Core Components

### 2.1 User Terminal Environment

- **User's tmux Session**: T1000 does not render its own window or emulate a terminal. It operates entirely within the user's pre-existing terminal emulator via `tmux`.
- **Active Working Pane**: The pane where the user is currently working. When T1000 is invoked, it reads the content from this pane and injects approved commands here.
- **AI Agent Pane**: A tmux pane running `t1000 chat` or `t1000 ask`. Streams AI responses and presents tool call approval prompts.

### 2.2 IPC Layer

- **Protocol**: Newline-delimited JSON over a Unix Domain Socket at `/tmp/t1000.sock`.
- **Request types**: `Ping`, `Shutdown`, `Ask { query, tmux_pane, session_id }`, `Chat { tmux_pane }`, `ToolCallResponse { id, approved }`.
- **Response types**: `Ok`, `Error(String)`, `Token(String)`, `ToolCallPrompt { id, command, background }`.
- Each client connection handles exactly one request/response cycle, except for streaming `Ask`/`Chat` connections which receive a token stream terminated by `Ok` or `Error`.

### 2.3 T1000 Background Daemon

- **AI Agent Engine** (`daemon.rs`): Orchestrates the full request lifecycle — loads session history, builds the prompt (host context on first turn, terminal snapshot on subsequent turns), streams AI events, handles tool call approval and execution, and persists conversation history.
- **Multi-Turn Session Store**: An in-memory `HashMap<session_id, SessionEntry>` shared across connections. Entries are pruned after 30 minutes of inactivity. History is bounded to 40 messages (oldest tail + first message retained) to keep context windows manageable.
- **tmux Interoperability Layer** (`tmux/mod.rs`): Executes `tmux capture-pane`, `tmux send-keys`, `tmux list-panes`, and `tmux display-message` to read and write to the user's terminal.
- **Session Cache** (`tmux/cache.rs`): A background task that polls every 2 seconds, captures all panes in the monitored session, and maintains a summarized snapshot. Used as a fallback when the client's specific pane is not available.
- **System Context Collector** (`sys_context.rs`): Runs once per daemon lifetime (via `OnceLock`). Captures OS release, uptime, memory, load average, top CPU processes, shell environment (curated safe variables only), and shell history. Prepended to the AI context on the first turn of each conversation.
- **Security & Data Masking Filter** (`ai/filter.rs`): Applied to all terminal context and user queries before transmission. Masks AWS keys, passwords, tokens, secrets, PEM blocks, credit card numbers, and SSNs.
- **Prompt Engine Manager** (`config.rs`): Loads named system prompts from `~/.t1000/prompts/<name>.toml`. Falls back to the compiled-in SRE prompt if no file is found.
- **LLM API Client** (`ai/client.rs`): Implements `AiClient` trait for Anthropic, OpenAI, and Gemini with SSE streaming. Uses a process-wide `reqwest::Client` for connection reuse. Emits `Token`, `ToolCall`, `Error`, and `Done` events over an unbounded channel.

### 2.4 Daemon Lifecycle

- **Startup**: Validates API key, detects or creates the monitored tmux session, starts the cache poller and session cleanup tasks, checks for an already-running daemon (ping probe), then binds the Unix socket.
- **Logging**: All output (`println!`/`eprintln!`) is redirected to `~/.t1000/daemon.log` (or `--log-file FILE`) via `dup2` at startup. View live with `t1000 logs`.
- **Shutdown**: `t1000 stop` sends a `Shutdown` IPC request — the daemon responds `Ok`, removes the socket file, and exits. SIGTERM and SIGINT are also handled gracefully via `tokio::signal::unix`, removing the socket file before exit.

## 3. Data Flow Example: Troubleshooting an Error

1. The user encounters a daemon failure in their active tmux pane.
2. The user runs `t1000 ask "why did nginx crash?"` (or presses the tmux keybinding to open `t1000 chat`).
3. The CLI client reads `$TMUX_PANE` from the environment and sends an `Ask` request over the Unix socket.
4. The daemon captures the last 200 lines from the client's pane, runs the sensitive-data filter, and fetches the cached system context (or collects it fresh on the first-ever request).
5. The **Prompt Manager** supplies the SRE system prompt; the combined host context + terminal snapshot + user query is sent to the configured LLM API.
6. The API client streams tokens back; the daemon forwards each as a `Token` response to the client, which prints them as they arrive.
7. If the LLM invokes a tool call (e.g., `journalctl -u nginx.service`), the daemon sends a `ToolCallPrompt` response. The client displays the proposed command and waits up to 60 seconds for the user to type `y` or `n`.
8. On approval, the daemon injects the command into the user's tmux pane via `tmux send-keys`, waits for output, captures 200 lines, and returns the result to the LLM as a tool result message.
9. The LLM receives the tool result and continues its response. This loop repeats until the LLM produces a final answer with no further tool calls.
10. The daemon sends `Ok` to signal completion. The conversation history is stored under the session ID for the next turn.
