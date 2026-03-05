# Product Requirements Document (PRD) Details for DaemonEye

This document specifies the functional and non-functional requirements for DaemonEye, building upon the Product Definition.

## 1. Functional Requirements

### 1.1 Daemon Process & tmux Integration

- **FR-1.1.1**: The application MUST run as a background daemon process, independent of any specific terminal emulator.
- **FR-1.1.2**: The application MUST use `tmux` as its presentation layer and session interaction mechanism.
- **FR-1.1.3**: The daemon MUST be capable of spawning new tmux panes within the user's active tmux session to present the AI interface.
- **FR-1.1.4**: The application MUST provide a CLI tool or tmux keybinding to trigger the AI agent, which communicates with the background daemon.
- **FR-1.1.5**: The daemon MUST append a structured, single-line audit record to a configurable command log file for every tool call execution event (approved, denied, or timed out). The log path MUST default to `~/.daemoneye/commands.log` and MUST be overridable or disableable via CLI flags (`--command-log-file`, `--no-command-log`).

### 1.2 AI Agent Integration

- **FR-1.2.1**: The AI agent MUST act as an expert sysadmin and security expert, capable of determining the right tools to use.
- **FR-1.2.2**: When activated, the application MUST capture terminal context (visible output, backscroll history, environment state) from the currently active tmux pane and provide it to the AI agent. The captured context MUST include, for each pane: (a) the shell's current working directory (`#{pane_current_path}`); (b) the OSC terminal title (`#{pane_title}`) when set by a running application (e.g., vim, ssh, k9s) and distinct from the command name. Additionally, the application MUST fetch high-signal tmux session environment variables via `tmux show-environment` filtered against an allowlist (AWS_PROFILE, AWS_DEFAULT_REGION, AWS_REGION, KUBECONFIG, KUBE_CONTEXT, KUBECTL_CONTEXT, VAULT_ADDR, DOCKER_HOST, DOCKER_CONTEXT, ENVIRONMENT, APP_ENV, NODE_ENV, RAILS_ENV, RACK_ENV, VIRTUAL_ENV, CONDA_DEFAULT_ENV, GOPATH, GOENV, JAVA_HOME, LANG, LC_ALL) and prepend them as a `[SESSION ENVIRONMENT]` block to every AI context snapshot.
- **FR-1.2.3**: The AI MUST be able to analyze stack traces, crash logs, failing services, and security scan outputs to provide root cause analysis and remediation strategies.
- **FR-1.2.4**: The AI interacts directly with the user's active terminal using tmux session features. This allows the AI agent to "pair program" with the user. By hooking into the user's terminal session via tmux, the AI agent can execute commands, read output, and respond to system prompts with the user's permission.
- **FR-1.2.5**: The application MUST actively audit the system state (OS release, uptime, memory, load average, top CPU processes, shell environment, and shell history) once per session, cache it, and prepend this summary to the AI agent's context alongside the visible terminal buffer.
- **FR-1.2.6**: The daemon MUST detect its own hostname and whether the user's active tmux pane is connected to a remote host (via SSH or mosh). This execution context MUST be injected into the first-turn AI prompt so the AI understands which machine each execution mode (`background` vs `foreground`) will target.
- **FR-1.2.7**: The application MUST support two distinct command execution modes and the AI MUST be instructed to choose between them appropriately:
  - *Background mode*: Command runs in a dedicated tmux window (`de-bg-<session_name>-<YYYYMMDDHHMMSS>-<id_short>`) on the daemon host using the user's configured shell. The window is configured with `remain-on-exit on`. Output is captured via `capture-pane` and the exit code is extracted natively via `pane_dead_status` and returned to the AI. Background panes are tracked via `pane-died` and `alert-bell` hooks. Upon completion, the pane's entire scrollback history is logged to `~/.daemoneye/pane_logs/` and the window is destroyed after an automatic Garbage Collection timeout (5 seconds for success, 60 seconds for failure with user notifications). This gives the AI access to the user's full shell environment (PATH, aliases, functions) and makes all execution visible and auditable in the tmux session. The application MUST share background commands with the user and gain approval before executing them.
  - *Foreground mode*: Command is injected into the user's active tmux pane via `send-keys`. The user can see and interact with the command. If the user's pane is SSH'd to a remote host, the command executes on that remote host.
- **FR-1.2.10**: The daemon MUST support scheduling commands to run once at a specified UTC datetime or repeatedly at a specified ISO 8601 interval (e.g. `PT5M`, `PT1H`, `P1D`). Scheduled jobs MUST persist across daemon restarts in `~/.daemoneye/schedules.json` using atomic writes. Each job MUST run in a dedicated tmux window pane (`de-sched-<YYYYMMDDHHMMSS>-<id_short>`), which MUST be destroyed on success and left in place on failure for inspection. Output is captured vis 'capture-pane' and the exit code is extracted natively via `pane_dead_status` and returned to the AI. Scheduled job panes are tracked via `pane-died` and `alert-bell` hooks. The AI MUST be able to create, list, cancel, and delete scheduled jobs via tool calls. The daemon MUST support a `ScheduleWritePrompt` approval gate that displays the full schedule content to the user before writing.
- **FR-1.2.11**: The daemon MUST support watchdog jobs that run a command on a schedule and pass the captured output to the AI for analysis using a named runbook (TOML file in `~/.daemoneye/runbooks/`). When the AI detects an alert condition, it MUST send a `SystemMsg` notification to connected clients. If `[notifications] on_alert` is configured in `config.toml`, the daemon MUST invoke that shell command with `$DAEMONEYE_JOB` and `$DAEMONEYE_MSG` environment variables set.
- **FR-1.2.12**: The daemon MUST maintain a `~/.daemoneye/scripts/` directory of executable scripts (mode 0700). The AI MUST be able to write new scripts and update existing ones, subject to a `ScriptWritePrompt` approval gate that displays the full script content to the user before writing. The AI MUST be able to list available scripts and read their contents without approval.
- **FR-1.2.13**: The daemon MUST support an overarching Information Window (`de-info`). The daemon MUST ensure this window exists in the session upon startup, structured as a 3-pane layout containing tails of `daemon.log`, `activity.log`, and `commands.log`.
- **FR-1.2.9**: The chat input MUST operate in raw terminal mode to support readline-style editing. Users MUST be able to navigate the current input line with ←/→ arrow keys (and Home/End, Ctrl+A/E) and submit past entries with ↑/↓ arrow keys. The input MUST support in-line deletion (Backspace, Delete, Ctrl+K, Ctrl+U). The visible input window MUST scroll horizontally to keep the cursor in view for inputs wider than the chat pane. History MUST persist for the lifetime of the chat session and reset when the session ends.

### 1.3 Prompt Library

- **FR-1.3.1**: The application MUST include a library of pre-defined prompts for common tasks.
- **FR-1.3.2**: Users MUST be able to create, save, and manage their own custom prompts. A `daemoneye prompts` subcommand MUST list available prompts with their descriptions. A `/prompt <name>` in-chat command MUST start a new session using the named prompt as the system message. *(Config and file loading are implemented; the subcommand and in-chat command are pending.)*
- **FR-1.3.3**: All user-defined and standard prompts MUST be stored in the user's home directory under `~/.daemoneye/prompts`.

### 1.4 Authentication & Security

- **FR-1.4.1**: Sensitive data (passwords, secret keys, PII) MUST be masked or filtered from the terminal buffer before being transmitted to the AI API. The filter MUST cover at minimum: AWS access key IDs, PEM private key blocks, GCP service-account JSON private key fields, JWT bearer tokens, GitHub personal access tokens, database/broker connection URLs with embedded credentials, password/token/API-key assignments, URL query-param secrets, credit card numbers, and SSNs. Users MUST be able to extend the filter with additional patterns via `[masking] extra_patterns` in `config.toml`; built-in patterns MUST NOT be disableable.
- **FR-1.4.2**: Users MUST have explicit controls over what terminal context is sent to the LLM.
- **FR-1.4.3**: When the AI proposes a background command that requires `sudo`, the daemon MUST detect the sudo password prompt in the tmux window via `capture-pane`. It MUST then send a `CredentialPrompt` IPC response to the client (echo disabled). The password MUST be injected into the background window via `send-keys` and MUST NOT be logged, stored on disk, or transmitted to the AI.
- **FR-1.4.4**: When the AI proposes a foreground command that requires `sudo`, the daemon MUST inject the command into the user's terminal pane and wait briefly for it to start. The daemon MUST then capture the pane output and check whether a sudo password prompt is present. If a password prompt is detected: the application MUST notify the user in the chat interface, make the target terminal pane active so the user can type their password, wait for the password to be entered and the command to complete, then switch focus back to the AI chat pane. If no password prompt is detected (e.g. a NOPASSWD sudoers rule or a still-valid credential timestamp), the application MUST proceed with a standard wait interval without switching panes.

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
