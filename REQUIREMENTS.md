# Product Requirements Document (PRD) Details for T1000

This document specifies the functional and non-functional requirements for T1000, building upon the Product Definition.

## 1. Functional Requirements

### 1.1 Daemon Process & tmux Integration

- **FR-1.1.1**: The application MUST run as a background daemon process, independent of any specific terminal emulator.
- **FR-1.1.2**: The application MUST use `tmux` as its presentation layer and session interaction mechanism.
- **FR-1.1.3**: The daemon MUST be capable of spawning new tmux panes within the user's active tmux session to present the AI interface.
- **FR-1.1.4**: The application MUST provide a CLI tool or tmux keybinding to trigger the AI agent, which communicates with the background daemon.
- **FR-1.1.5**: The daemon MUST append a structured, single-line audit record to a configurable command log file for every tool call execution event (approved, denied, or timed out). The log path MUST default to `~/.t1000/commands.log` and MUST be overridable or disableable via CLI flags (`--command-log-file`, `--no-command-log`).

### 1.2 AI Agent Integration

- **FR-1.2.1**: The AI agent MUST act as an expert sysadmin and security expert, capable of determining the right tools to use.
- **FR-1.2.2**: When activated, the application MUST capture terminal context (visible output, backscroll history, environment state) from the currently active tmux pane and provide it to the AI agent.
- **FR-1.2.3**: The AI MUST be able to analyze stack traces, crash logs, failing services, and security scan outputs to provide root cause analysis and remediation strategies.
- **FR-1.2.4**: The AI interacts directly with the user's active terminal using tmux session features. This allows the AI agent to "pair program" with the user. By hooking into the user's terminal session via tmux, the AI agent can execute commands, read output, and respond to system prompts with the user's permission.
- **FR-1.2.5**: The application MUST actively audit the system state (OS release, uptime, memory, load average, top CPU processes, shell environment, and shell history) once per session, cache it, and prepend this summary to the AI agent's context alongside the visible terminal buffer.
- **FR-1.2.6**: The daemon MUST detect its own hostname and whether the user's active tmux pane is connected to a remote host (via SSH or mosh). This execution context MUST be injected into the first-turn AI prompt so the AI understands which machine each execution mode (`background` vs `foreground`) will target.
- **FR-1.2.7**: The application MUST support two distinct command execution modes and the AI MUST be instructed to choose between them appropriately:
  - *Background mode*: Command runs as a daemon subprocess on the daemon's local host. Output is captured and returned to the AI silently.
  - *Foreground mode*: Command is injected into the user's active tmux pane via `send-keys`. The user can see and interact with the command. If the user's pane is SSH'd to a remote host, the command executes on that remote host.

### 1.3 Prompt Library

- **FR-1.3.1**: The application MUST include a library of pre-defined prompts for common tasks.
- **FR-1.3.2**: Users MUST be able to create, save, and manage their own custom prompts.
- **FR-1.3.3**: All user-defined and standard prompts MUST be stored in the user's home directory under `~/.t1000/prompts`.

### 1.4 Authentication & Security

- **FR-1.4.1**: Sensitive data (passwords, secret keys, PII) MUST be masked or filtered from the terminal buffer before being transmitted to the AI API.
- **FR-1.4.2**: Users MUST have explicit controls over what terminal context is sent to the LLM.
- **FR-1.4.3**: When the AI proposes a background command that requires `sudo`, the application MUST prompt the user for their password via the chat interface with terminal echo disabled. The password MUST be piped directly to the `sudo` subprocess and MUST NOT be logged, stored on disk, or transmitted to the AI.
- **FR-1.4.4**: When the AI proposes a foreground command that requires `sudo`, the application MUST notify the user in the chat interface to type their password in the terminal pane. The application MUST extend the wait time for the command to complete to accommodate the password entry.

### 1.5 Extensibility

- **FR-1.5.1**: The application MUST include a native plugin architecture for community extensions.
- **FR-1.5.2**: The application MUST allow plugins to hook into AI prompt lifecycles, and third-party APIs (e.g., AWS/GCP/Azure).

---

## 2. Non-Functional Requirements

### 2.1 Performance

- **NFR-2.1.1**: Capturing tmux buffers and transmitting to the AI MUST NOT block the user's terminal interaction.
- **NFR-2.1.2**: The daemon MUST be lightweight and consume minimal background system resources when idle.

### 2.2 Compatibility & Environment

- **NFR-2.2.1**: The application MUST run on standard modern Linux distributions (Ubuntu, Fedora, Arch, etc.).
- **NFR-2.2.2**: The application requires `tmux` to be available in the system PATH.

### 2.3 Usability

- **NFR-2.3.1**: The interaction with the AI agent inside the tmux pane MUST feel natural and responsive.
- **NFR-2.3.2**: The chat interface MUST provide a visual indicator (animated spinner) while waiting for the AI to respond.
- **NFR-2.3.3**: Tool call prompts MUST clearly communicate which execution mode will be used and where the command will run (daemon host or user's terminal pane).
- **NFR-2.3.4**: The chat interface SHOULD use color and Unicode symbols to visually distinguish AI responses, tool calls, approvals, errors, and system messages.
