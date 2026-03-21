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

- **Context-Aware Assistance**: DaemonEye's "killer feature" is its ability to feed the terminal's visible output, backscroll history, and deeply audited host configuration (OS state, uptime, running processes, and command history) into AI agents. On the first turn of each session the full terminal snapshot is provided automatically. On subsequent turns the AI requests a fresh snapshot on demand via the `get_terminal_context` tool — keeping context lean for conversational exchanges and ensuring a current view when the AI actually needs to inspect the screen. The AI knows *what* the user is looking at and *what* commands were recently executed within the tmux session. Per-pane context includes each pane's current working directory, OSC terminal title (set by running applications such as vim, ssh, and k9s via escape sequences), synchronized-input flag, and dead-pane status with exit code (so the AI knows when a background job has finished and with what result). Non-active panes are also annotated with a temporal activity indicator — `[active Xs ago]`, `[idle Nm]`, or `[idle NhNm]` — derived from tmux's `pane_activity` timestamp, giving the AI a sense of which panes are actively in use. ANSI SGR colour codes in the active pane content are converted to semantic markers — `[ERROR: text]` (red), `[WARN: text]` (yellow), `[OK: text]` (green) — so the AI can identify failures and successes at a glance without needing to interpret raw terminal escape sequences. When two or more tmux sessions exist, an `[OTHER SESSIONS]` block is appended listing each non-current session's name, window count, last-activity age, and attachment state — so the AI can reason across parallel workstreams (e.g. a staging session running alongside a production session) without requiring the user to switch contexts. High-signal tmux session environment variables — including cloud account (AWS_PROFILE), Kubernetes cluster (KUBECONFIG), vault address (VAULT_ADDR), runtime environment tier (NODE_ENV), and active language runtime (VIRTUAL_ENV, etc.) — are captured via `tmux show-environment` with a curated allowlist and included in the AI's context snapshot.
- **Instant Activation**: Summon an AI agent instantly via a tmux keybinding or CLI command. This opens an interactive AI session in a dynamically positioned tmux pane.
- **AI-Powered Capabilities**:
  - **Pair-Programming & Troubleshooting**: The AI doesn't just suggest commands; it uses Tool Calling to propose executing commands directly in your active tmux session. Each proposed command presents a three-option approval prompt — approve once, approve the entire class of commands for the session, or deny. This session-level approval (independent for regular and sudo command classes) eliminates repetitive prompts in trusted automated sequences while keeping privilege-escalation commands under separate control.
  - **Dual Execution Modes**: The AI chooses between two command execution modes. *Background mode* runs the command in a dedicated tmux window (`de-bg-*`) on the daemon host using the user's configured shell. The call returns immediately with the assigned pane ID; when the command finishes, a `[Background Task Completed]` context message containing the exit code and captured output is injected into the AI session. The shell is kept alive after each command so the AI can chain follow-up commands in the same environment using the returned pane ID. Up to 5 background windows persist per session; the oldest completed window is evicted when the cap is reached. The full scrollback is always archived to `~/.daemoneye/pane_logs/`. The AI can explicitly close a background window when it is no longer needed via `close_background_window(pane_id)` (no approval required); background windows that remain open 15 minutes after completion are automatically garbage-collected by the daemon as a safety net. *Foreground mode* injects the command into your active terminal pane via `send-keys` and detects completion cleanly by attaching a temporary `pane-title-changed` tmux hook, eliminating terminal clutter while remaining completely event-driven; the command is visible and interactive in your pane. If the AI does not explicitly specify a target pane, the client auto-selects the sole sibling pane, presents a numbered picker when multiple siblings exist, or offers to split the window side-by-side when the chat pane is alone; the chosen pane is persisted to `~/.daemoneye/pane_prefs.json` so the user is never prompted again after the first run. The AI knows your daemon's hostname and whether your pane is SSH'd to a remote machine, and selects the mode accordingly. Interactive session commands (`ssh`, `mosh`, `telnet`, `screen`) are treated as a distinct sub-case of foreground mode: the daemon returns as soon as the remote connection is established and instructs the AI to use `target_pane` to direct follow-up commands into the open session — no re-connection needed. Exit codes for foreground commands are captured via a shell hook (`PROMPT_COMMAND` / `precmd`) that writes the exit status to the tmux session environment after each command; `daemoneye setup` prints the one-line snippet to add to `~/.bashrc` or `~/.zshrc`.
  - **Command Scheduler & Watchdog**: The AI can schedule commands to run once at a specific UTC time or repeatedly on an interval. Watchdog jobs run a command on a schedule and pass the output to the AI for analysis using a named runbook (markdown with YAML frontmatter) — triggering alerts when issues are detected. Runbooks reference knowledge memory keys whose content is automatically injected into the watchdog prompt. Each scheduled job runs in its own tmux window (`de-<id>`), left in place on failure for inspection. Alerts can be forwarded to an external notification command via `[notifications] on_alert`.
  - **Knowledge System**: Three-tier AI-writable persistence. *Runbooks* (`~/.daemoneye/runbooks/`, markdown) encode watchdog procedures, alert criteria, and remediation steps; they can reference named knowledge memories. *Memory* (`~/.daemoneye/memory/`) stores durable facts in three categories: `session` entries are auto-injected into every AI turn (32 KB cap); `knowledge` entries are loaded on-demand by runbooks or the `read_memory` tool; `incident` records are historical and searchable. *Search* lets the AI locate anything across runbooks, scripts, memory, and the full event log in a single tool call. Runbook and memory writes are immediately available to the next turn without any daemon restart.
  - **Passive Pane Monitoring**: The daemon registers hooks at startup using the absolute path of the running binary. Four global hooks: `pane-died` (notifies the daemon when a background pane exits), `after-new-session` (automatically installs all per-session hooks for any tmux session created after the daemon starts — no manual reconfiguration needed), `client-detached` (records detach time and history watermark so the next AI turn can issue a catch-up brief), and `client-attached` (clears the detach record so the brief fires only once per detach cycle). Three per-session hooks: `alert-bell` (existing), `pane-focus-in` (instantly updates the active-pane state when the user switches panes — eliminates up to 2 s staleness from the background poll), and `session-window-changed` (instantly refreshes window topology when the user switches windows). When the user re-attaches after ≥ 30 seconds away and new events have accumulated (background task completions, webhook alerts, watchdog results, or watch-pane outcomes), the daemon delivers a `[Catch-up] N events while you were away (Xm): …` system message at the top of the next AI turn — keeping the user informed without requiring manual review of session history. When a background pane exits, the daemon issues a `tmux display-message` overlay, injects a `[Background Task Completed]` context message into the AI's session history, and GC-kills the window. The AI can also passively monitor arbitrary panes via `watch_pane`.
  - **Scripts Directory**: The AI can author, update, list, and delete reusable shell scripts in `~/.daemoneye/scripts/`. Script writes and deletes require a user approval step before any change is made. Scripts can be referenced by name in scheduled jobs.
  - **Sudo Integration**: Commands requiring elevated privileges are handled gracefully in both modes. Background sudo prompts appear in the chat interface with echo-disabled password input. Foreground sudo commands notify you to type your password in the terminal pane.
  - **Webhook Alert Receiver**: An optional HTTP endpoint (default port 9393, disabled by default) accepts alert payloads from Prometheus Alertmanager, Grafana, or any JSON source. Alerts are deduplicated by fingerprint, masked for sensitive data, injected into the AI's active session histories, and displayed via `tmux display-message` in all chat panes — turning DaemonEye into a true on-call responder without user intervention. A matching runbook triggers automatic AI analysis; if the AI detects an alert condition it notifies all sessions and fires the configured `on_alert` notification hook. Protected by an optional Bearer token.
  - **Task Automation & Fleet Management**: Generate scripts or run on-the-fly automation commands to manage single host configurations or automated fleet deployments. The AI agent acts as an expert sysadmin.
  - **Security Auditing**: Have the AI agent analyze system states, running processes, or security scan outputs to recommend and automatically apply remediation solutions.
  - **Structured Event Log**: Every command the AI executes, AI usage metrics, and system events are written to `~/.daemoneye/events.jsonl` as structured JSON objects.
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

1. A user asks the AI: *"Set up a watchdog that checks disk usage every 10 minutes. Store the alert threshold in memory so we can tune it later."*
2. The AI calls `add_memory("disk_thresholds", "Alert when usage > 85%", category="knowledge")`, then `write_runbook("disk-check", ...)` with `memories: [disk_thresholds]` in the frontmatter and the standard `## Alert Criteria` section.
3. The user approves the runbook write in the chat interface. The AI then creates a scheduled watchdog job referencing the `disk-check` runbook.
4. When the job fires, the daemon captures `df -h` output, loads the `disk_thresholds` knowledge memory, and builds the watchdog prompt with both the runbook content and the memory context.
5. If disk usage exceeds the threshold, the AI emits a `SystemMsg` notification in the chat pane. If `[notifications] on_alert` is configured (e.g. `notify-send`), the alert is also sent there.
6. On success the window is cleaned up; on failure it is left in place for inspection.

### Workflow 4: Knowledge Accumulation Across Sessions

1. The AI learns that a particular host runs a non-standard Postgres port. It calls `add_memory("db-host-quirks", "pg on :5433 not :5432", category="knowledge")`.
2. In a later session the user asks about a connection failure. The AI calls `search_repository("pg", kind="memory")` and finds the quirk immediately.
3. After resolving a major incident, the AI writes an `incident` memory with root cause, symptoms, and fix — making it available for future `search_repository` queries even though it is never auto-loaded.

### Workflow 5: Security Remediation

1. The user runs a vulnerability scanner (`lynis` or `chkrootkit`) on a server.
2. The output is massive. The user hits the AI agent keybinding: *"Summarize the critical vulnerabilities found and generate the commands to patch them."*
3. The AI agent outputs a curated markdown list of issues alongside copy-pasteable (or one-click executable) remediation scripts.

### Workflow 6: On-Call Alert Response

1. A disk-usage Prometheus alert fires and Alertmanager POSTs the payload to `http://localhost:9393/webhook`.
2. DaemonEye checks the fingerprint — this is a new alert, not a duplicate.
3. The alert is masked for sensitive data, logged to `events.jsonl`, and formatted as a `[Webhook Alert]` message.
4. The message is injected into all active AI session JSONL files. A `tmux display-message` overlay appears in every chat pane: `[FIRING] HighDiskUsage — Disk /dev/sda1 at 93% (alertmanager)`.
5. Severity `"critical"` meets the `severity_threshold = "warning"` gate, so `fire_notification()` runs the user's `on_alert` command.
6. `auto_analyze = true` triggers runbook lookup: `"HighDiskUsage"` → `high-disk-usage.md` is found.
7. The AI analyses the alert against the runbook and responds with `ALERT: /dev/sda1 needs immediate attention — remediation steps: …`.
8. The analysis is injected into sessions and displayed in chat panes.
9. On the next AI turn the user sees both the raw alert and the runbook analysis in context, ready to act on the AI's proposed remediation.

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
