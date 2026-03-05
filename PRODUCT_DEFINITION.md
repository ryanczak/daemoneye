# Product Definition Document: DaemonEye

## 1. Product Overview

**Name**: DaemonEye (aka T.1.K.)  
**Type**: Linux Daemon / Tmux Plugin  
**Inspiration**: tmux, claude code, T-1000 (Terminator franchise)

**Vision Statement**:  
DaemonEye elevates the command-line experience by embedding AI agents like Google Gemini, Anthropic Claude, or OpenAI's ChatGPT directly into your existing terminal workflow via **tmux**. Operating as a lightweight daemon process, DaemonEye manages AI interactions through tmux panes without attempting to replace your terminal emulator. The goal of DaemonEye is to act as an intelligent, context-aware pair-sysadmin, leveraging advanced AI to automate tasks, troubleshoot problems, manage OS settings and security.

---

## 2. Target Audience

- **System Administrators (Sysadmins)**: Managing fleets of internal/external servers, deploying applications, performing configuration management, and troubleshooting live production issues.
- **SREs & Platform Engineers**: Operating and troubleshooting OS, scripts, apps, CI/CD pipelines and cloud infrastructure directly from the terminal, via control plane APIs, and scrappiness as required to get the job done.
- **Developers**: Writing code, managing local environments, reading complex build logs, and seeking rapid, context-aware debugging support.

---

## 3. Core Features & Capabilities

### 3.1 Native tmux Integration

- **tmux Backend Process**: DaemonEye runs as a background daemon and integrates directly with your active `tmux` server.
- **Seamless Attachment**: Attach to an existing tmux session, or start a new one, and invoke the AI agent. The AI agent will appear in a newly spawned tmux pane alongside your work.
- **Session Persistence**: Sessions, panes, and window layouts are fully preserved through native tmux capabilities, meaning users can detach and reattach to remote or local environments without dropping their AI context.

### 3.2 Deep AI Integration

- **Context-Aware Assistance**: DaemonEye's "killer feature" is its ability to feed the terminal's visible output, backscroll history, and deeply audited host configuration (OS state, uptime, running processes, and command history) into AI agents. The AI knows *what* the user is looking at and *what* commands were recently executed within the tmux session. Per-pane context now also includes each pane's current working directory and OSC terminal title (set by running applications such as vim, ssh, and k9s via escape sequences). High-signal tmux session environment variables — including cloud account (AWS_PROFILE), Kubernetes cluster (KUBECONFIG), vault address (VAULT_ADDR), runtime environment tier (NODE_ENV), and active language runtime (VIRTUAL_ENV, etc.) — are captured via `tmux show-environment` with a curated allowlist and included in the AI's context snapshot.
- **Instant Activation**: Summon an AI agent instantly via a tmux keybinding or CLI command. This opens an interactive AI session in a dynamically positioned tmux pane.
- **AI-Powered Capabilities**:
  - **Pair-Programming & Troubleshooting**: The AI doesn't just suggest commands; it uses Tool Calling to propose executing commands directly in your active tmux session. Each proposed command presents a three-option approval prompt — approve once, approve the entire class of commands for the session, or deny. This session-level approval (independent for regular and sudo command classes) eliminates repetitive prompts in trusted automated sequences while keeping privilege-escalation commands under separate control.
  - **Dual Execution Modes**: The AI chooses between two command execution modes. *Background mode* runs the command in a dedicated tmux window (`de-bg-*`) on the daemon host using the user's configured shell — capturing output via `capture-pane` and returning it to the AI; the window is always cleaned up after the result is captured. *Foreground mode* injects the command directly into your active terminal pane, making it visible and interactive. The AI knows your daemon's hostname and whether your pane is SSH'd to a remote machine, and selects the mode accordingly.
  - **Command Scheduler & Watchdog**: The AI can schedule commands to run once at a specific UTC time or repeatedly on an interval. Watchdog jobs run a command on a schedule and pass the output to the AI for analysis using a named runbook — triggering alerts when issues are detected. Each scheduled job runs in its own tmux window (`de-<id>`), left in place on failure for inspection. Alerts can be forwarded to an external notification command via `[notifications] on_alert`.
  - **Passive Pane Monitoring**: AI can execute long-running background commands which the daemon automatically tracks via `tmux pane-died` and `alert-bell` hooks. If you ask the AI to run a build or wait for a server to spin up, it spawns the background window and returns to you immediately. When the background pane finishes or explicitly emits a bell, the daemon cleans up the window through an automatic Garbage Collection lifecycle and interrupts your chat session with an out-of-band `[System]` alert.
  - **Scripts Directory**: The AI can author, update, and list reusable shell scripts in `~/.daemoneye/scripts/`. Script writes require a user approval step that shows the full content before writing. Scripts can be referenced by name in scheduled jobs.
  - **Sudo Integration**: Commands requiring elevated privileges are handled gracefully in both modes. Background sudo prompts appear in the chat interface with echo-disabled password input. Foreground sudo commands notify you to type your password in the terminal pane.
  - **Task Automation & Fleet Management**: Generate scripts or run on-the-fly automation commands to manage single host configurations or automated fleet deployments. The AI agent acts as an expert sysadmin.
  - **Security Auditing**: Have the AI agent analyze system states, running processes, or security scan outputs to recommend and automatically apply remediation solutions.
  - **Command Audit Log**: Every command the AI executes is written to `~/.daemoneye/commands.log` — a tamper-evident, single-line-per-event log with timestamp, session ID, execution mode, approval status, and output excerpt.
  - **Prompt Library**: A library of pre-defined prompts for common tasks. Users can also create and save their own prompts. The prompts are stored in the user's home directory in the `.daemoneye/prompts` directory.

### 3.3 Extensibility & Community Ecosystem

- **Robust Plugin Architecture**: A native plugin system allowing the community to extend DaemonEye.
- **Third-Party Integrations**: Easily bolt-on additional features like custom AI prompts, specialized cloud provider API integrations (AWS/GCP/Azure CLI enhancements), or specific tooling workflows (Docker, k8s).

---

## 4. Key User Workflows

### Workflow 1: The "What went wrong?" Troubleshooting

1. A user attempts to start a local database service in a tmux pane, but it fails with a cryptic 50-line error trace.
2. The user hits the **AI agent keybinding**.
3. DaemonEye captures the last 200 lines of history from the active pane, notes the daemon hostname and that the pane is local, then passes everything to the AI agent.
4. The AI agent's tmux pane opens, explaining the error in plain English: *"It looks like port 5432 is already bound by another zombie process."* It proposes `sudo kill -9 <PID>`. The user approves; the chat interface prompts for the sudo password with echo disabled, runs the command, and reports the result — all without leaving the AI pane.

### Workflow 2: Rapid Fleet Configuration

1. A sysadmin is SSH'd into a jump server via a tmux session.
2. They open the AI agent pane and ask: *"exexcute an ssh-keyscan loop to update my known_hosts for the 15 web servers listed in `fleet.txt`, then write a command to update Nginx on all of them."*
3. The AI agent provides the exact bash loops and the sysadmin executes them. The sysadmin can also have the AI agent execute the commands for them.

### Workflow 3: Watchdog Monitoring

1. A user asks the AI: *"Set up a watchdog that checks disk usage every 10 minutes using the disk-usage runbook."*
2. The AI creates a scheduled job (`de-<id>` window) that runs `df -h` on a 10-minute interval, referencing the `disk-usage` runbook.
3. When the job fires, the daemon captures the output, passes it to the AI along with the runbook's alert criteria, and the AI analyzes it.
4. If disk usage exceeds the threshold defined in the runbook, the AI emits a `SystemMsg` notification in the chat pane. If `[notifications] on_alert` is configured (e.g. `notify-send`), the alert is also sent there.
5. On success the window is cleaned up; on failure it is left in place for inspection.

### Workflow 4: Security Remediation

1. The user runs a vulnerability scanner (`lynis` or `chkrootkit`) on a server.
2. The output is massive. The user hits the AI agent keybinding: *"Summarize the critical vulnerabilities found and generate the commands to patch them."*
3. The AI agent outputs a curated markdown list of issues alongside copy-pasteable (or one-click executable) remediation scripts.

---

## 5. Technical Requirements

- **Platform**: Linux Environment.
- **Core Dependencies**: `tmux` (must be installed on the host machine). DaemonEye runs as a headless daemon.
- **API Access**: Requires a valid API Key for an AI agent (e.g., Google Gemini, Anthropic Claude, or OpenAI's ChatGPT) configured in the daemon.
- **Privacy & Security Framework**:
  - Explicit user controls over what terminal context is sent to the LLM.
  - Sensitive data masking: a multi-pattern regex filter runs on all terminal context before transmission. Built-in patterns cover AWS keys, PEM/GCP private keys, JWTs, GitHub PATs, database connection URLs, password/token assignments, URL query-param secrets, credit cards, and SSNs. Users extend the filter with org-specific patterns via `[masking] extra_patterns` in `config.toml`; built-in patterns cannot be disabled.

---

## 6. Success Metrics

- **Adoption**: Number of active daily users / GitHub stars.
- **AI Engagement**: Percentage of terminal sessions where the AI agent is invoked.
- **Community Growth**: Number of community-developed plugins created and published to the DaemonEye ecosystem within the first 6 months.
