# DaemonEye: AI Powered Operator 

DaemonEye is a lightweight background daemon that integrates an AI-powered systems, software, and security expert directly into your tmux session. Instead of a passive chatbot, DaemonEye acts as a context-aware peer that understands your full terminal state—including scrollback buffer, environment variables, running programs and command history. By leveraging Ghost Shell sub-agents, Webhook alert ingestion, and access to scripts and runbooks, it can autonomously troubleshoot failures and remediate incidents in the background. With its secure sudo integration, DaemonEye handles the repetitive lifting of infrastructure management and emergency response, allowing you to focus on your work without distraction.

I wrote DaemonEye after discovering OpenClaw and being completely blown away by its agency and power. So much so that I eventually turned it off because I was afraid of it running amok. I loved working with OpenClaw on the command line to manage my hosts and services but I wanted something that I did not have to worry about when I was not around. DaemonEye is the result. It limits command execution to what is allowed, while providing mechanisms to make complex tasks, even those that require root access, autonomous. 

DaemonEye runbooks can be prescriptive, limitng the agent to a fixed script, or they can be open-ended, allowing the agent autonomy. Either way, runbooks can limit the number of turns an agent has to perform an action, preventing run-away agents and wasted tokens. Runbooks can specify which AI is used so you can use powerful AIs when you need them and cheap, local AIs, when the task is simple or sensitive. Speaking of sensitive, DaemonEye redacts sensitive information like API keys so your secrets are never shared with the AI. Webhooks make it easy to integrate DaemonEye with Prometheus, Grafana, and other services. Webhooks use runbooks to enable autonomous end-to-end incident response and other scripted action. Webhooks enable DaemonEye instances to interoperate in cool ways too.

DaemonEye is not perfect, it will eat your important stuff if you are not careful. It is not as powerful as Claude Code or OpenClaw, but it does a pretty good job helping me manage my hosts and services so maybe it will be useful to you as well. 

---

## ✨ Key Features

### 🛠️ Collaborative Execution & Safety

The AI doesn't just suggest — it acts. Every proposed action goes through an explicit approval prompt before anything runs.

**Terminal commands** show a three-option prompt:

- **`[y]es`** — Approve a single execution.
- **`[A]pprove for session`** — Trust the AI for this command class for the rest of the session.
- **`[N]o`** — Reject, or type a message to redirect the AI mid-stream.

**Script and runbook writes** show an ANSI diff before asking for approval:

- New files display all lines in green with `+` prefixes so you can read exactly what will be written.
- Modifications display a standard unified diff — red `-` for removed lines, green `+` for added lines, with `@@` hunk headers — so you can see precisely what changed.
- **`[A]pprove for session`** is available here too: once approved, future writes to that specific script or runbook auto-proceed (the diff is still shown, the gate is skipped).

**Visual Anchors:** During the command approval window the target pane is highlighted with a dark-blue background (`colour17`) so you always know exactly where a command will land before you commit.

**`/auto-approval`** — type this at the chat prompt to inspect which approvals are currently active. Use `/auto-approval off` to instantly revoke all session approvals and return to explicit confirmation for everything.

### 📡 Webhook Alert Ingestion

Expose an optional HTTP endpoint (default port 9393) to receive alerts from Prometheus Alertmanager, Grafana, or any tool that can POST JSON.

- **Deduplication** — Alerts are fingerprinted; duplicates within a configurable window are suppressed automatically.
- **Sensitive-data masking** — Alert payloads pass through the same redaction filter as terminal context before entering the AI conversation.
- **Watchdog Analysis** — Each incoming alert is automatically analyzed against its matching runbook to determine whether autonomous remediation is warranted.

### 📖 Runbooks & Knowledge

- **Procedure Runbooks** — Store troubleshooting steps in `~/.daemoneye/runbooks/` as Markdown with YAML frontmatter. When an alert fires, DaemonEye finds the matching runbook and uses it to guide the investigation.
- **Durable Memory** — Three-tier persistence for session context, knowledge facts, and incident records. Session memories are injected into every AI turn automatically; knowledge and incident memories are available on demand.
- **Built-in Guides** — Six knowledge memory files are seeded on first run covering webhooks, runbook format, ghost shell usage, scheduling, scripts, and sudoers setup — the AI can reference them without any manual setup.

### 🐕 Command Scheduler & Watchdog

- **Flexible Scheduling** — Run commands or Ghost Shell tasks once at a specific time, on a repeating interval, or on a full cron expression.
- **Watchdog Monitors** — Active monitors use AI-powered analysis to evaluate system state on a schedule and trigger remediation when something looks wrong.
- **Failure Isolation** — Each job runs in its own dedicated tmux window (`de-sj-*`), left open on failure for manual inspection and cleaned up automatically on success.

### 👻 Autonomous Ghost Shells

When a critical alert matches a runbook with `enabled: true`, DaemonEye spawns a Ghost Shell that works the problem without a human present.

- **Unattended Remediation** — Runs inside a dedicated `de-incident-*` tmux window, executing pre-approved remediation steps and reporting back via a catch-up brief when you next attach.
- **Policy Gating** — Non-sudo commands run freely within your OS permissions; `sudo` is gated to scripts explicitly listed in `auto_approve_scripts` that also have a NOPASSWD sudoers rule installed via `daemoneye install-sudoers`.
- **Turn Budget** — A configurable hard ceiling on AI turns (default 20) ensures the agent cannot loop indefinitely. Individual runbooks may set a lower limit, but never a higher one.

### 🔒 Security & Privacy

Context is filtered before it ever leaves your machine.

- **Sensitive Data Redaction** — A built-in regex filter scrubs AWS access keys, PEM private key blocks, GCP service-account JSON, JWT bearer tokens, GitHub personal access tokens (classic and fine-grained), database and broker connection URLs with embedded credentials, password and API key assignments, URL query-param secrets, credit card numbers, and US Social Security Numbers — each replaced with a labelled placeholder (`<REDACTED>`, `<JWT>`, `<DB_URL>`, `<GITHUB_TOKEN>`, etc.) before context reaches any AI provider.
- **User-Defined Patterns** — Add org-specific regexes to `extra_patterns` in `config.toml` to extend the built-in set without replacing it. Per-category hit counts are shown in `daemoneye status` for a continuous audit view.
- **Sudo Password Handling** — When a command requires `sudo`, the daemon first checks whether credentials are already cached (`sudo -n true`). If not, the chat interface prompts for your password with terminal echo disabled — for both foreground and background commands the password is always typed in the chat pane, eliminating the risk of keystrokes landing in the wrong terminal window. Up to 3 attempts are allowed; a wrong password is detected and re-prompted automatically. If all attempts fail, the AI receives a structured error with an `install-sudoers` suggestion. The password is never written to disk, stored in a log, or transmitted to the AI.
- **`sudoers.d` Integration** — `daemoneye install-sudoers <script>` writes a NOPASSWD drop-in to `/etc/sudoers.d/daemoneye-<name>` that pins the exact absolute path of the approved script — no wildcards, no `ALL`. Privilege escalation requires both an `auto_approve_scripts` entry in the runbook and a matching sudoers rule; either alone is insufficient.

---

## Platform Support

**Linux only.** DaemonEye uses `fork(2)`, Unix domain sockets, and Linux-specific tmux hooks — it will not build or run on macOS or Windows.

---

## Requirements

| Dependency | Notes |
|---|---|
| Rust 1.79+ | Required by Rust edition 2024 |
| tmux 2.6+ | Required for hook support (`pane-focus-in`, `client-attached`, `after-new-session`) |

On Debian/Ubuntu:

```sh
sudo apt install tmux 
```

On Fedora:

```sh
sudo dnf install tmux
```

---

## Build

```sh
git clone <repo>
cd daemoneye
cargo build --release
```

The compiled binary is at `target/release/daemoneye`.

To install it into your `~/.cargo/bin` path:

```sh
cargo install --path .
```

---

## Usage

DaemonEye requires the daemon to be running in the background.

### 1. Start the daemon

```sh
daemoneye daemon
```

To stream the daemon logs:

```sh
daemoneye logs
```

To log directly to the console (useful when troubleshooting):

```sh
daemoneye daemon --console
```

To write daemon logs to a custom path:

```sh
daemoneye daemon --log-file /var/log/daemoneye.log
```

Event records (command history, AI turn counts, lifecycle info) are written to `~/.daemoneye/var/log/events.jsonl` by default.

You can also manage the daemon with systemd — run `daemoneye setup` to write the service file and get the enable commands (see [Install DaemonEye](#2-install-daemoneye) below).

### 2. Install DaemonEye

Run `daemoneye setup` once after building. It initialises the full `~/.daemoneye/` directory tree, copies the binary to a stable location, writes a systemd user service file, and prints the tmux keybinding to add to `~/.tmux.conf`.

After an upgrade, use the overwrite flags to refresh installed files:

| Flag | Effect |
|---|---|
| `--overwrite-bin` | Copy the current binary to `~/.daemoneye/bin/daemoneye`, replacing the previously installed version. |
| `--overwrite-memory` | Overwrite the six built-in knowledge memory files in `~/.daemoneye/memory/knowledge/` with the versions bundled in the new binary. Any user-created memory files are not affected. |
| `--overwrite-all` | Combines `--overwrite-bin` and `--overwrite-memory`, and also refreshes `~/.daemoneye/etc/prompts/sre.toml`. Your `config.toml` is never overwritten. |

On first run all seeded files (binary, memories, prompt) are written automatically regardless of flags.

```sh
daemoneye setup
```

#### Directory layout

`daemoneye setup` creates the following tree. Directories and files that already exist are never overwritten, so re-running `setup` after an upgrade is safe. `~/.daemoneye/` is the shared root for both the daemon process and the AI agent. Everything — configuration, scripts, runbooks, memory, logs — lives in a single place:

```
~/.daemoneye/
  bin/
    daemoneye             ← copy of the running binary; the service file and bind-key point here
  etc/
    config.toml           ← main configuration (created once; your edits are preserved)
    prompts/
      sre.toml            ← built-in SRE system prompt (recreated only if missing)
  lib/                    ← place shared SDK modules or Python helpers here
  memory/
    knowledge/
      ghost-shell-guide.md       ← guide to ghost shell usage (seeded once)
      runbook-format.md          ← runbook markdown format reference (seeded once)
      runbook-ghost-template.md  ← ghost-enabled runbook template (seeded once)
      scheduling-guide.md        ← scheduler usage guide (seeded once)
      scripts-and-sudoers.md     ← scripts and sudoers setup guide (seeded once)
      webhook-setup.md           ← webhook integration guide (seeded once)
  runbooks/               ← your procedure runbooks (Markdown + frontmatter)
  scripts/                ← your automation scripts (set chmod 700 on write)
  var/
    log/
      daemon.log          ← daemon process log (tailed by `daemoneye logs`)
      events.jsonl        ← structured event log (command history, AI turns, lifecycle)
      panes/              ← archived background-command output (one .log per job window)
      pipes/              ← pipe-pane capture logs (raw terminal output, runtime only)
    run/
      daemoneye.sock      ← Unix domain socket (created when the daemon starts)
      pane_prefs.json     ← per-session target-pane preferences
      schedules.json      ← scheduled job store
```

#### systemd user service

`daemoneye setup` writes `~/.config/systemd/user/daemoneye.service` — a user-scoped service that runs `~/.daemoneye/bin/daemoneye daemon --console` automatically on login. The `--console` flag is required for `Type=simple` systemd services: without it the daemon forks, the parent exits, and systemd loses track of the process.

When running as a systemd service the daemon starts outside tmux and automatically creates (and owns) a tmux session named `"daemoneye"` — ghost shells, scheduled jobs, and webhook-triggered automation are available immediately with no interactive client connection required.

To use a different session name, add a `[daemon]` section to `~/.daemoneye/etc/config.toml`:

```toml
[daemon]
tmux_session = "myserver"   # override the default "daemoneye" session name
```

```sh
# Enable and start the daemon on login
systemctl --user daemon-reload
systemctl --user enable --now daemoneye

# Check status
systemctl --user status daemoneye

# Stop the daemon
systemctl --user stop daemoneye

# Restart after a config change
systemctl --user restart daemoneye

# Disable autostart
systemctl --user disable daemoneye
```

View daemon logs directly:

```sh
daemoneye logs          # tails ~/.daemoneye/var/log/daemon.log
```

Or through journald:

```sh
journalctl --user -u daemoneye -f
```

#### tmux keybinding

Add the printed `bind-key` line to `~/.tmux.conf`:

```sh
# ~/.tmux.conf
bind-key T split-window -v '~/.daemoneye/bin/daemoneye chat'
```

Reload your tmux config:

```sh
tmux source-file ~/.tmux.conf
```

The bind-key uses the full path to `~/.daemoneye/bin/daemoneye` so it works even when `~/.cargo/bin` is not in the `PATH` that tmux inherits.

#### Shell hook (optional)

Add the appropriate snippet to your shell config to enable accurate exit-code tracking for foreground commands in `daemoneye status`:

```sh
# bash (~/.bashrc)
_de_exit_trap() { tmux set-environment "DE_EXIT_${TMUX_PANE#%}" "$?" 2>/dev/null; }
PROMPT_COMMAND="_de_exit_trap${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
```

```sh
# zsh (~/.zshrc)
_de_precmd() { tmux set-environment "DE_EXIT_${TMUX_PANE#%}" "$?" 2>/dev/null; }
precmd_functions+=(_de_precmd)
```

Without this hook foreground commands still appear in `daemoneye status` but are always recorded as succeeded regardless of their actual exit code.

### 3. Interact with the AI

Press your configured hotkey (e.g., `Ctrl+b T`) inside a tmux session to open a new split pane connected to DaemonEye. Ask it questions about errors in your other panes, or request it to execute commands.

You can also interact directly from the command line:

```sh
# Single question (non-interactive)
daemoneye ask "why is nginx returning 502?"

# Interactive multi-turn chat
daemoneye chat
```

### All subcommands

| Command | Description |
|---|---|
| `daemoneye daemon` | Start the background daemon |
| `daemoneye daemon --console` | Start daemon with output on the console (troubleshooting) |
| `daemoneye daemon --log-file FILE` | Write daemon log to `FILE` instead of `~/.daemoneye/var/log/daemon.log` |
| `daemoneye daemon --session NAME` | Override the managed tmux session name from config (useful for testing or running multiple instances) |
| `daemoneye stop` | Stop the daemon gracefully |
| `daemoneye logs` | Tails the `daemon.log` file |
| `daemoneye chat` | Start an interactive multi-turn chat session (auto-attaches to managed tmux session when outside tmux) |
| `daemoneye chat --session NAME` | Open a chat window in a specific tmux session and attach to it |
| `daemoneye ask <query>` | Send a single question to the AI |
| `daemoneye setup` | Initialise `~/.daemoneye/`, install binary, write systemd service, print tmux config |
| `daemoneye setup --overwrite-bin` | Re-copy the current binary to `~/.daemoneye/bin/daemoneye` |
| `daemoneye setup --overwrite-memory` | Refresh built-in knowledge memory files from the current binary |
| `daemoneye setup --overwrite-all` | Refresh binary, memories, and built-in SRE prompt (user `config.toml` is never touched) |
| `daemoneye scripts` | List scripts in `~/.daemoneye/scripts/` |
| `daemoneye schedule list` | List scheduled jobs and their status |
| `daemoneye schedule cancel <id>` | Cancel a scheduled job |
| `daemoneye schedule windows` | List leftover tmux windows from failed scheduled jobs (`de-*`) |

---

## Configuration

DaemonEye stores its configuration in `~/.daemoneye/etc/config.toml`. The file is created automatically on first launch with default values.

### Full example

```toml
# The default model — used unless the session has a /model override.
[models.default]
provider = "anthropic"
api_key  = "sk-ant-..."
model    = "claude-sonnet-4-6"

# Optional additional models selectable via /model <name> in chat,
# or referenced by name in runbook frontmatter (model: opus).
# [models.opus]
# provider = "anthropic"
# model    = "claude-opus-4-6"
#
# [models.local]
# provider  = "ollama"
# model     = "llama3.2"
# base_url  = "http://localhost:11434/v1"
# context_window_tokens = 8192
#
# [models.gpt]
# provider = "openai"
# model    = "gpt-4o"

[ai]
prompt = "sre"

# [masking]
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]

# [ghost]
# max_ghost_turns = 20   # hard ceiling; individual runbooks may set lower

# [daemon]
# tmux_session = "daemoneye"   # session the daemon creates/owns at startup
# auto_create_session = true   # create the session if it doesn't exist (default: true)

# [webhook]
# enabled = false
# port = 9393
# bind_addr = "127.0.0.1"   # set to "0.0.0.0" to expose on all interfaces
# secret = ""               # Bearer token; empty = no auth
# auto_analyze = true
# severity_threshold = "warning"   # "info" | "warning" | "critical"
# dedup_window_secs = 300
```

### `[models.<name>]` sections

Each named model is a separate TOML table. `[models.default]` is required and used when no session-level override is active. Additional entries are selectable via the `/model <name>` slash command in chat, or by setting `model: <name>` in a runbook's frontmatter for ghost shell sessions.

| Key | Type | Default | Description |
|---|---|---|---|
| `provider` | string | `"anthropic"` | AI backend to use. See valid values below. |
| `api_key` | string | `""` | API key for the chosen provider. If empty, falls back to the provider's environment variable. Not required for `ollama` or `lmstudio`. |
| `model` | string | `"claude-sonnet-4-6"` | Model name passed to the provider API. |
| `base_url` | string | *(provider default)* | Override the API base URL. Useful for pointing at a remote Ollama host, LM Studio instance, or any OpenAI-compatible proxy. |
| `context_window_tokens` | integer | *(model lookup)* | Override the context-window size in tokens. Set this for local models where the automatic lookup is inaccurate. |

#### Valid `provider` values

| Value | Provider | Default API endpoint | API key required |
|---|---|---|---|
| `"anthropic"` | Anthropic (Claude) | `https://api.anthropic.com/v1/messages` | Yes |
| `"openai"` | OpenAI (or any OpenAI-compatible API) | `https://api.openai.com/v1` | Yes |
| `"gemini"` | Google Gemini | `https://generativelanguage.googleapis.com/v1beta/` | Yes |
| `"ollama"` | Ollama (local, OpenAI-compatible) | `http://localhost:11434/v1` | No |
| `"lmstudio"` | LM Studio (local, OpenAI-compatible) | `http://localhost:1234/v1` | No |

For `ollama`, start the server with `ollama serve` and pull a model (`ollama pull llama3.2`).
For `lmstudio`, start the local server from the LM Studio app and load a model.

### `[ai]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `prompt` | string | `"sre"` | Name of a prompt file in `~/.daemoneye/etc/prompts/` (without `.toml`). |

### `[masking]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `extra_patterns` | list of strings | `[]` | Additional regex patterns to redact before context is sent to the AI. Each match is replaced with `<REDACTED>`. Built-in patterns always run; these extend the set. |

Example:

```toml
[masking]
extra_patterns = [
  "MYCO-[A-Z0-9]{32}",       # internal API token format
  "sk_live_[A-Za-z0-9]{32}", # Stripe live secret key
]
```

### `[notifications]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `on_alert` | string | `""` | Shell command to run when a watchdog alert fires. Available env vars: `$DAEMONEYE_JOB` (job name), `$DAEMONEYE_MSG` (alert message). |

Example:

```toml
[notifications]
on_alert = "notify-send '$DAEMONEYE_JOB' '$DAEMONEYE_MSG'"
```

### `[webhook]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Start the HTTP webhook server on daemon startup. |
| `port` | integer | `9393` | TCP port to listen on. |
| `bind_addr` | string | `"127.0.0.1"` | IP address to bind the webhook listener. Set to `"0.0.0.0"` to expose on all interfaces. |
| `secret` | string | `""` | Bearer token for authentication. Empty = no auth. |
| `auto_analyze` | bool | `true` | Run runbook-based AI analysis when a matching runbook exists. |
| `severity_threshold` | string | `"warning"` | Minimum severity to trigger AI analysis and `on_alert`. One of `"info"`, `"warning"`, `"critical"`. |
| `dedup_window_secs` | integer | `300` | Suppress duplicate alerts with the same fingerprint within this many seconds. |

#### Prometheus Alertmanager integration

Add a DaemonEye receiver to your Alertmanager configuration:

```yaml
receivers:
  - name: daemoneye
    webhook_configs:
      - url: http://localhost:9393/webhook
        # If webhook.secret is set:
        # http_config:
        #   authorization:
        #     credentials: <your-secret>

route:
  receiver: daemoneye
```

### `[ghost]` section

Daemon-wide hard limits for autonomous Ghost Shells. These are ceilings — individual runbooks can set lower values but cannot exceed them.

| Key | Type | Default | Description |
|---|---|---|---|
| `max_ghost_turns` | integer | `20` | Hard upper limit on AI turns per ghost shell. A runbook's `max_ghost_turns` is clamped to this value. Set lower in production to constrain blast radius. |

### `[daemon]` section

Controls daemon startup and session ownership. Use this when running DaemonEye as a systemd user service so ghost shells, scheduled jobs, and webhook-triggered automation work without any `daemoneye chat` client connected.

| Key | Type | Default | Description |
|---|---|---|---|
| `tmux_session` | string | `""` | Name of the tmux session the daemon creates (or adopts, if it already exists) at startup. Empty = legacy behaviour: the daemon borrows whatever session the first `daemoneye chat` client connects from. |
| `auto_create_session` | bool | `true` | Create the session with `tmux new-session -d` if it does not already exist. Only applies when `tmux_session` is set. If the session is killed, the daemon recreates it automatically. |

When `tmux_session` is set, `daemoneye chat` invoked **outside** of tmux will open a new chat window inside the managed session and exec-attach to it, dropping the user straight into the right place.

---

## Ghost Shells & Autonomous Remediation

Ghost Shells are unattended AI agents that DaemonEye can spawn automatically in response to incoming webhook alerts. When triggered, a ghost shell runs inside a dedicated `de-incident-*` tmux window on the daemon host, investigates the alert, and executes pre-approved remediation steps — all without a human present. Start, completion, and failure events appear in the next catch-up brief when you re-attach.

### How it works end-to-end

```
Alertmanager / Grafana / curl
        │
        ▼
POST /webhook  ──→  DaemonEye dedup + mask
        │
        ▼
Runbook lookup (alertname → kebab-case filename)
        │
        ▼
Watchdog AI analysis (reads runbook, emits GHOST_TRIGGER: YES|NO)
        │  YES + runbook has  enabled: true
        ▼
GhostManager::start_session()
  • Allocates de-incident-<name>-<ts> tmux window
  • Loads ghost_config from runbook frontmatter
  • Injects [Ghost Shell Started] into all active chat sessions
        │
        ▼
Ghost AI turn loop (up to max_ghost_turns)
  • Reads runbook + alert context as system prompt
  • Issues run_terminal_command (background mode only)
  • Policy gate: non-sudo commands always allowed (OS permissions are the boundary);
    sudo commands must be in auto_approve_scripts + have a NOPASSWD sudoers rule
  • resolve_command() rewrites bare/relative script names
    to ~/.daemoneye/scripts/<name> (+ sudo prefix if run_with_sudo: true)
  • watch_pane blocks until command exits before next turn
        │
        ▼
[Ghost Shell Completed — session log: ~/.daemoneye/var/log/sessions/ghost-<name>-<uuid>.jsonl]
or [Ghost Shell Failed — session log: ...]
injected into all active sessions → appears in catch-up brief
Use read_file(<path>) to review the full ghost conversation
```

### Step 1 — Write a remediation script

Place scripts in `~/.daemoneye/scripts/`. DaemonEye sets them `chmod 700`.

```bash
# Use the AI tool or write directly:
daemoneye ask "write a script called restart-nginx.sh that restarts nginx and \
  checks its status, then tails the last 20 lines of /var/log/nginx/error.log"
```

Or write it manually:

```bash
cat > ~/.daemoneye/scripts/restart-nginx.sh << 'EOF'
#!/usr/bin/env bash
set -euo pipefail

echo "=== Restarting nginx ==="
systemctl restart nginx
sleep 2
systemctl is-active --quiet nginx && echo "nginx: OK" || { echo "nginx: FAILED"; exit 1; }

echo "=== Recent error log ==="
tail -20 /var/log/nginx/error.log
EOF
chmod 700 ~/.daemoneye/scripts/restart-nginx.sh
```

### Step 2 — Configure sudo NOPASSWD (optional)

If the script needs elevated privileges (e.g., `systemctl restart nginx`), create a sudoers drop-in so it can run without a password prompt. Ghost sessions run unattended — interactive `sudo` password prompts will cause the command to fail.

```bash
# Create a sudoers drop-in — use visudo -f to validate syntax
sudo visudo -f /etc/sudoers.d/daemoneye-ghost
```

```sudoers
# Allow the daemoneye user to restart nginx without a password
your-username ALL=(ALL) NOPASSWD: /home/your-username/.daemoneye/scripts/restart-nginx.sh
```

> **Important:** Use the **full absolute path** in the sudoers entry — the same path that DaemonEye will resolve to (`~/.daemoneye/scripts/<name>`). Wildcards in sudoers paths are dangerous; pin the exact filename.

Verify the entry works before testing ghost shells:

```bash
sudo ~/.daemoneye/scripts/restart-nginx.sh
```

### Step 3 — Create a ghost-enabled runbook

Runbook filenames must match the Prometheus alertname converted to kebab-case:
`NginxDown` → `nginx-down`, `HighDiskUsage` → `high-disk-usage`.

```bash
daemoneye ask "write a runbook for the NginxDown alert"
# or write it directly with write_runbook
```

Full runbook example:

````markdown
---
tags: [nginx, web, production]
memories: [nginx-config-notes]
enabled: true
auto_approve_scripts: [restart-nginx.sh]
run_with_sudo: true
max_ghost_turns: 10
---
# Runbook: nginx-down

## Purpose
Automated first-responder for the NginxDown alert. Restarts nginx and
captures the error log for post-incident review.

## Alert Criteria
- Prometheus rule: `up{job="nginx"} == 0` for > 2 minutes
- Severity: critical

## Remediation Steps
1. **Investigate**: Check nginx process status and recent error log.
2. **Restart**: Run `restart-nginx.sh` to restart nginx and verify recovery.
3. **Escalation**: If restart fails, page the on-call engineer. Do not retry
   more than once — leave the window open for manual inspection.

## Notes
- If nginx fails to start, check for config syntax errors: `nginx -t`
- Common cause: stale PID file at `/var/run/nginx.pid`
````

#### Frontmatter fields reference

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Allow DaemonEye to spawn an autonomous Ghost Shell for this alert. |
| `auto_approve_scripts` | list | `[]` | Script names in `~/.daemoneye/scripts/` pre-approved for **sudo** execution. Non-sudo commands run freely without listing them. Bare names, relative paths (`./name.sh`), and commands with arguments are all resolved to the absolute path. |
| `run_with_sudo` | bool | `false` | Auto-prepend `sudo` when executing scripts listed in `auto_approve_scripts`. The ghost AI can then write `script.sh` instead of `sudo script.sh`. Does **not** grant permission to run arbitrary sudo commands — the `auto_approve_scripts` whitelist is always enforced. Each approved script still requires a NOPASSWD sudoers rule via `daemoneye install-sudoers`. |
| `max_ghost_turns` | integer | `0` | Per-runbook turn cap. Clamped to the daemon ceiling (`ghost.max_ghost_turns` in `config.toml`). `0` means use the daemon ceiling. |
| `ssh_target` | string | *(none)* | SSH destination (e.g. `user@host` or `host`) for remote execution. When set, all commands are transparently wrapped in `ssh <target> <cmd>` before execution. Scripts are resolved to `~/.daemoneye/scripts/<name>` on the remote host. The AI is instructed not to SSH manually — omit this field for local-only execution. |

### Step 4 — Enable the webhook and configure Alertmanager

In `~/.daemoneye/etc/config.toml`:

```toml
[webhook]
enabled = true
port = 9393
bind_addr = "127.0.0.1"
secret = "change-me"          # set a Bearer token; leave empty to disable auth
auto_analyze = true
severity_threshold = "warning"
dedup_window_secs = 300
```

In your Alertmanager config:

```yaml
receivers:
  - name: daemoneye
    webhook_configs:
      - url: http://localhost:9393/webhook
        http_config:
          authorization:
            credentials: change-me   # matches webhook.secret

route:
  receiver: daemoneye
  group_by: [alertname]
  group_wait: 10s
  group_interval: 5m
  repeat_interval: 1h
```

Restart the DaemonEye daemon to pick up the config change:

```bash
daemoneye stop && daemoneye daemon
```

### Step 5 — Test end-to-end

Simulate an alert with curl to verify the full pipeline before a real incident:

```bash
curl -s -X POST http://localhost:9393/webhook \
  -H "Authorization: Bearer change-me" \
  -H "Content-Type: application/json" \
  -d '{
    "version": "4",
    "status": "firing",
    "alerts": [{
      "status": "firing",
      "labels": {
        "alertname": "NginxDown",
        "severity": "critical",
        "instance": "localhost:9113"
      },
      "annotations": {
        "summary": "nginx is down on localhost"
      },
      "fingerprint": "test-001"
    }]
  }'
```

Watch the ghost shell in real time:

```bash
# In another pane — attach to the incident window
tmux select-window -t de-incident-nginx-down-$(date +%s | head -c8 2>/dev/null || echo "")

# Or just watch daemon.log
daemoneye logs
```

Check the event log for the full audit trail:

```bash
grep "ghost\|webhook_analysis\|command_approval" ~/.daemoneye/var/log/events.jsonl | tail -30
```

### Monitoring active ghost shells

```bash
daemoneye status
```

The `Ghost Shells` section of the status output shows:

```
Ghost Shells
  Active:    1
  Launched:  3
  Completed: 2
  Failed:    0
```

List the incident windows currently open:

```bash
tmux list-windows | grep de-incident
```

### Security considerations

- **Non-sudo commands run as you.** The ghost runs as the same OS user as the daemon. Any command that doesn't require `sudo` runs within your existing file permissions — no additional policy needed.
- **Sudo requires two explicit approvals.** To allow a sudo command: (1) list the script in `auto_approve_scripts`, and (2) run `daemoneye install-sudoers <script>` to create the NOPASSWD sudoers rule. Both must be present. Any other sudo command is automatically denied.
- **Scope sudoers entries tightly.** `daemoneye install-sudoers` pins the exact absolute path in `/etc/sudoers.d/`. Never manually add `ALL` as the command or allow path wildcards.
- **Only list scripts you control.** `auto_approve_scripts` matches filenames in `~/.daemoneye/scripts/`. Scripts outside that directory are never auto-approved regardless of path.
- **`enabled: true` is opt-in per runbook.** Alerts without a matching runbook, or runbooks without `enabled: true`, never trigger a ghost shell.
- **Turn budget limits blast radius.** The daemon enforces a hard ceiling via `ghost.max_ghost_turns` in `config.toml` (default 20). Individual runbooks may set a *lower* limit with `max_ghost_turns` in their frontmatter, but can never exceed the daemon ceiling. A ghost shell is forcibly stopped when the limit is reached regardless of what it is doing.
- **All actions are logged.** Every command approval, execution, and result is recorded in `events.jsonl` for post-incident audit.

### Environment variables

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | API key for the `anthropic` provider (used if `api_key` is not set in config). |
| `OPENAI_API_KEY` | API key for the `openai` provider (used if `api_key` is not set in config). |
| `GEMINI_API_KEY` | API key for the `gemini` provider (used if `api_key` is not set in config). |
| `OPENAI_API_BASE` | Override the base URL for the `openai` provider (fallback; prefer `base_url` in config). |

---

## Project Structure

```
src/
├── main.rs          # CLI entry point — parses subcommands (daemon, stop, ping, logs, chat, ask, setup, scripts, sched)
├── ipc.rs           # Request/Response enums — the full wire protocol; GhostConfig struct
├── config.rs        # ~/.daemoneye/etc/config.toml parsing; GhostDaemonConfig; prompt loading; directory helpers
├── daemon/          # Background process: IPC server, session memory, background execution
│   ├── mod.rs       # Daemon entry point; supervise() task supervisor; hook installation
│   ├── server.rs    # IPC connection loop; AI prompt assembly; trigger_ghost_turn()
│   ├── executor.rs  # Tool call dispatch; approval gate (ToolCallOutcome); foreground/background execution
│   ├── background.rs # run_background_in_window(); notify_job_completion(); GC lifecycle
│   ├── digest.rs    # Session digest: structured compaction of conversation history at 30 messages
│   ├── ghost.rs     # GhostManager::start_session() — allocates de-incident-* tmux window
│   ├── policy.rs    # GhostPolicy — OS-delegation trust model: non-sudo always allowed; sudo requires auto_approve_scripts + install-sudoers
│   └── stats.rs     # Atomic ghost shell counters (launched / completed / failed / active)
├── cli/             # IPC client: chat interface, terminal rendering, subcommands
├── scheduler.rs     # ScheduledJob, ScheduleStore (JSON persistence), ScheduleKind, ActionOn, JobStatus
├── runbook.rs       # Runbook markdown loader (frontmatter parser, CRUD); watchdog AI system prompt builder
├── webhook.rs       # HTTP alert ingestion (axum); parse_payload(); process_alert(); parse_ghost_trigger()
├── memory.rs        # Persistent memory: session (auto-loaded), knowledge (on-demand), incidents (search-only)
├── search.rs        # Keyword search across runbooks, scripts, memory, and events.jsonl
├── scripts.rs       # Script management: list, write (chmod 700), read, delete, resolve
├── sys_context.rs   # One-time host audit (OS, uptime, memory, processes, shell history)
├── tmux/
│   ├── mod.rs       # tmux interoperability layer (capture-pane, send-keys, create/kill job windows, etc.)
│   ├── cache.rs     # Background poller; SessionCache; PaneState; get_labeled_context()
│   └── session.rs   # Session-level helpers: other_sessions_context(); client_dimensions(); session_exists()
├── config.rs        # ~/.daemoneye/etc/config.toml parsing, prompt loading, directory helpers
└── ai/
    ├── mod.rs       # AiClient trait; send_with_retry(); CircuitBreaker
    ├── types.rs     # PendingCall / AiEvent enums; Message; AiUsage
    ├── tools.rs     # Tool definitions (Anthropic/OpenAI); dispatch_tool_event()
    ├── backends/    # Per-provider SSE streaming: anthropic.rs, openai.rs, gemini.rs
    └── filter.rs    # Regex-based sensitive-data masking; init_masking()
```

---

## Command Audit Log

Every command the AI proposes — whether approved, denied, or timed out — is recorded as a JSON object in `~/.daemoneye/var/log/events.jsonl`:

```
[1748000000] session=abc123 mode=background pane=- status=approved cmd=ps aux --sort=-%mem out=USER PID ...
[1748000001] session=abc123 mode=foreground pane=%3 status=denied cmd=sudo rm -rf /tmp/old out=
```

Fields: Unix timestamp · session ID · `background` or `foreground` · tmux pane ID · `approved` / `denied` / `timeout` / `send-failed` · command · first 200 chars of output.

Control with `--command-log-file FILE` or `--no-command-log` on `daemoneye daemon`.

---

## Security Notes

Before sending terminal context to an AI provider, DaemonEye applies a regex-based
filter that masks:

- AWS access key IDs (`AKIA…`)
- PEM private key blocks (RSA, EC, OpenSSH, etc.)
- GCP service-account JSON `"private_key"` fields
- JWT bearer tokens
- GitHub personal access tokens — classic (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`) and fine-grained (`github_pat_`)
- Database / broker connection URLs with embedded credentials (`postgresql://`, `mysql://`, `mongodb+srv://`, `redis://`, `amqp://`, etc.)
- Password, token, secret, and API key assignments (`password=…`, `api_key: …`, etc.)
- URL query-param secrets (`?token=…`, `&password=…`)
- Credit card numbers (16-digit grouped format)
- US Social Security Numbers

Masked values are replaced with placeholder tokens (`<REDACTED>`, `<JWT>`, `<DB_URL>`, `<GITHUB_TOKEN>`, etc.). Review the context shown in the AI pane before submitting if you handle highly sensitive data.

To register organisation-specific patterns, add them to your config (see [masking] below). Built-in patterns always run — user patterns extend the set, never replace it. Redaction counts by type are tracked across the daemon's lifetime and displayed under **Redactions** in `daemoneye status`, giving operators a quick audit view of what categories of sensitive data have been filtered. All built-in types are always shown (including those with a zero count), and any hits from user-configured `extra_patterns` are tallied separately as `"User Defined"`.

### Sudo passwords

When a command (foreground or background) requires `sudo`, the daemon first checks
whether credentials are already cached (`sudo -n true`). If cached, the command
runs without any interruption. If not cached, the chat interface prompts for your
password with terminal echo disabled — you always type it in the chat pane, not
in the terminal pane, eliminating the risk of keystrokes landing in the wrong window.

Up to 3 attempts are permitted. A wrong password is detected from the pane output
("Sorry, try again.") and you are re-prompted automatically. If all attempts fail
or you cancel, the AI receives a structured error describing what happened and
suggesting `daemoneye install-sudoers` where appropriate.

The password is never written to disk, stored in a log file, or transmitted to the
AI. The in-memory credential is held in a `zeroize::Zeroizing<String>` that
overwrites the allocation on drop.

---

## License

MIT License

Copyright (c) 2026 Matt Ryanczak

