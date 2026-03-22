# DaemonEye - The AI Powered Operator

DaemonEye is a lightweight background daemon that integrates with `tmux` to embed an AI assistant directly into your existing terminal workflow. It acts as an intelligent, context-aware senior principal site reliability engineer.

---

## Features

- **Native tmux Integration** — DaemonEye runs as a background process and interacts directly with your active `tmux` server.
- **Session State Caching** — The daemon actively monitors your `tmux` session, summarizing output from all panes. On the **first turn** of each session the full terminal snapshot (active pane contents, non-active pane summaries, session topology, and environment) is automatically included in context. The active pane content is captured from a `pipe-pane` log rather than `capture-pane` when available — giving the AI access to the full output history since the chat started, including content that has scrolled past the tmux scrollback buffer (build output, long test runs, etc.). ANSI colour codes in the captured output are converted to semantic markers: `[ERROR: text]` (red), `[WARN: text]` (yellow), and `[OK: text]` (green) — letting the AI immediately locate failures and confirmations without parsing escape sequences. When no pipe log exists, `capture-pane -e` preserves the colour codes so the same annotation logic applies. On **subsequent turns** the AI requests a fresh snapshot on demand via `get_terminal_context` — keeping mid-conversation messages lean while ensuring the AI always has an accurate view when it needs one. Non-active pane summaries are classified as visible panes (same window as chat), background panes (daemon-launched), or session panes (other user windows), each including the shell's current working directory and its OSC terminal title (set by applications like vim, ssh, and k9s). Non-active panes are also annotated with a temporal activity indicator — `[active Xs ago]`, `[idle Nm]`, or `[idle NhNm]` — derived from tmux's `#{pane_activity}` timestamp, giving the AI a sense of which panes were recently in use. When two or more tmux sessions exist, an `[OTHER SESSIONS]` block is also appended listing each non-current session's name, window count, last-activity age, and whether a client is attached — so the AI can reason about work happening in parallel sessions without switching context. High-signal tmux session environment variables (cloud account, Kubernetes cluster, runtime tier, language runtime, etc.) are captured via `tmux show-environment` against a curated allowlist.
- **Embedded AI Assistant** — Streams responses from Anthropic Claude, OpenAI, or Google Gemini with automatic context capture and sensitive-data masking.
- **Collaborative Execution (Tool Calling)** — The AI can propose commands to fix issues. Each tool call presents a three-option prompt: `[y]es` (approve once), `[a]pprove session` (auto-approve all commands of this class for the rest of the session), or `[N]o`. If the user types any other text at the approval prompt, the tool chain is aborted and the message is injected as a new conversation turn — allowing course-correction mid-chain without triggering a synthetic error. Two independent approval classes exist — *regular* and *sudo* — so sudo commands always prompt separately until explicitly session-approved. For foreground commands the approval panel shows the target pane's window-relative index and window name (e.g. `→ target: pane 1 in 'main' (%23)`) so the user can map the tool call to their visible tmux layout. Simultaneously, the target pane is highlighted with a dark-blue background tint (`tmux select-pane -P bg=colour17`) during the approval window — a visual anchor that makes the target immediately obvious. The highlight is removed when the command completes or when the user denies. Two execution modes: *background* (runs in a dedicated tmux window `de-bg-<session_name>-...` on the daemon host; returns immediately with the pane ID; when the command finishes a `[Background Task Completed]` context message is injected into the AI session with the exit code and captured output; the window persists for the session — up to 5 at a time — so the AI can run follow-up commands in the same shell; full scrollback is archived to `~/.daemoneye/pane_logs/`; the AI calls `close_background_window(pane_id)` when finished with a window, and windows still open 15 minutes after completion are auto-GC'd) and *foreground* (command is injected into a tmux pane via `send-keys`; completion uses a three-way branch — interactive commands like `ssh`, `mosh`, `telnet`, and `screen` return immediately once the remote shell prompt appears, with a `[Interactive session started]` result and instructions to use `target_pane` for follow-up commands in the open session; remote panes use output-stability polling; local panes use event-driven `pane-title-changed` hook detection). The AI uses `target_pane` (a pane ID from `list_panes` or context blocks) to direct foreground commands at specific panes other than the active one.
- **Pane Discovery & Identity** — The AI can call `list_panes` to see all active tmux panes in the session (pane ID, window-relative index, window name, command, working directory, title) and then target any of them with `run_terminal_command`. Every pane block in context — `[ACTIVE PANE]`, `[VISIBLE PANE]`, `[BACKGROUND PANE]`, `[SESSION PANE]` — now also carries `idx:N in 'window'`: the 0-based window-relative index that matches what the user sees when they press `ctrl+a q`. The AI is instructed to always address panes by both handle and index (e.g. "pane index 1 in 'main' (%23)") so the user can visually confirm the target before approving.
- **Command Scheduler & Watchdog** — Schedule commands to run once at a time or on a repeating interval. Set up watchdog monitors with AI-powered runbook analysis. Each scheduled job runs in its own tmux window (`de-<id>`), left in place on failure for inspection. Watchdog jobs can trigger alerts via a configurable notification hook (`[notifications] on_alert`).
- **Knowledge System** — Three-tier persistence for AI-generated knowledge: *runbooks* (`~/.daemoneye/runbooks/`, markdown with frontmatter) for watchdog procedures; *memory* (`~/.daemoneye/memory/{session,knowledge,incidents}/`) for durable facts and incident records; and *search* for cross-corpus keyword lookup across runbooks, scripts, memory, and the event log. Session memories are automatically injected into every AI turn. Runbook and memory writes are exposed as AI tools with approval gates for destructive operations.
- **Passive Pane Monitoring** — The daemon registers hooks at startup using the absolute path to the running binary. Four global hooks: `pane-died` (notifies the daemon when a background pane exits — triggers output capture, `[Background Task Completed]` history injection, and GC window cleanup), `after-new-session` (automatically installs all per-session hooks for any tmux session created after the daemon starts — no manual reconfiguration needed), `client-detached` (records detach time and history watermark on matching sessions so the next AI turn can generate a catch-up brief), and `client-attached` (clears the detach record so the catch-up brief fires only once per detach cycle). Three per-session hooks installed by `install_session_hooks()`: `alert-bell` (existing background-completion fallback), `pane-focus-in` (notifies the daemon whenever the user switches panes, updating the active-pane cache immediately rather than waiting up to 2 s for the next poll), and `session-window-changed` (notifies the daemon whenever the active window changes, triggering an instant window-topology refresh). When the user re-attaches after ≥ 30 seconds away and new events occurred (background task completions, webhook alerts, watchdog results, watch-pane outcomes, or autonomous ghost session starts/completions/failures), the daemon sends a `[Catch-up] N events while you were away (Xm): …` system message at the start of the next AI turn so nothing goes unnoticed. When a background pane exits, the daemon issues a `tmux display-message` overlay, injects a `[Background Task Completed]` context message into the AI's session history, and GC-kills the window. The AI can also passively monitor arbitrary panes via `watch_pane`.
- **Scripts Directory** — AI and users can create, read, list, and delete reusable scripts in `~/.daemoneye/scripts/`. Script writes and deletes are approval-gated. Scripts can be referenced by name in scheduled jobs.
- **Execution Context Awareness** — On every first turn the AI is told the daemon's hostname and whether your terminal pane is local or connected to a remote host via SSH or mosh. This ensures the AI targets the right machine when choosing between background and foreground execution.
- **Sudo Password Integration** — Background commands that require `sudo` trigger a password prompt in the chat interface (echo disabled). Foreground sudo commands notify you to type your password in the terminal pane.
- **Structured Event Logging** — Every executed command, AI turn usage, and lifecycle event is appended to `~/.daemoneye/events.jsonl` as a single structured JSON object.
- **Multi-Turn Chat Memory** — The `chat` subcommand maintains full conversation history across turns within a session. The bottom border of the user input box shows `turn N · Xk / Yk tokens · Z% remaining`, giving you a live read on context consumption relative to the model's context window. The indicator is color-coded: dim when comfortable, yellow past 50 %, bold red past 75 %.
- **Multi-line Chat Input** — The chat input box word-wraps long text across up to 5 rows instead of scrolling horizontally; the box grows upward as you type and collapses back on submission. The top border shows your `user@host`. Supports history navigation (↑/↓ arrow keys), in-line cursor movement (←/→, Home/End, Ctrl+A/E), and kill shortcuts (Ctrl+K/U). History persists for the lifetime of the chat session.
- **IPC Architecture** — A lightweight CLI client communicates with the background daemon via a Unix Domain Socket (`~/.daemoneye/daemoneye.sock`) for instant, non-blocking interaction. The socket lives in the user's home directory (not `/tmp`) so other local users cannot connect to it or pre-create a symlink at that path.
- **Webhook Alert Ingestion** — Optionally expose an HTTP endpoint (default port 9393) that accepts alerts from Prometheus Alertmanager, Grafana, or any generic JSON tool. Received alerts are deduplicated by fingerprint, masked for sensitive data, injected into active AI session histories, and displayed via `tmux display-message` in all chat panes. A matching runbook triggers automatic AI analysis via a watchdog prompt; the watchdog model emits `GHOST_TRIGGER: YES` or `GHOST_TRIGGER: NO` on its final line. If `YES` and the runbook has `enabled: true` in its `ghost_config` frontmatter, DaemonEye spawns an **Autonomous Ghost Session** in a dedicated incident window (`de-incident-*`) to handle the alert unattended. Ghost session start, completion, and failure events are injected into all active sessions and appear in the next catch-up brief. Protected by a configurable Bearer token. Use `GET /health` for liveness probes.

---

## Requirements

| Dependency | Notes |
|---|---|
| Rust 1.75+ | Edition 2024 |
| tmux | Essential for the presentation layer and session management |

On Debian/Ubuntu:

```sh
sudo apt install tmux build-essential
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

Event records (command history, AI turn counts, lifecycle info) are written to `~/.daemoneye/events.jsonl` by default.

You can also manage the daemon with systemd — run `daemoneye setup` for the service file.

### 2. Configure tmux

Run `daemoneye setup` to get the recommended configuration. It prints three things:

**a) systemd service file** — follow the printed instructions to enable the daemon on login.

**b) tmux keybinding** — add to `~/.tmux.conf`:

```sh
# ~/.tmux.conf
bind-key T split-window -h 'daemoneye chat'
```

Reload your tmux config:

```sh
tmux source-file ~/.tmux.conf
```

**c) Shell hook for exit-code tracking** — add the appropriate snippet to your shell config so DaemonEye can record the real exit code of foreground commands in `daemoneye status`:

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

Without this hook, foreground commands are still tracked in `daemoneye status` but always recorded as succeeded regardless of their actual exit code.

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
| `daemoneye daemon --log-file FILE` | Write daemon log to `FILE` instead of `~/.daemoneye/daemon.log` |
| `daemoneye stop` | Stop the daemon gracefully |
| `daemoneye logs` | Tails the `daemon.log` file |
| `daemoneye chat` | Start an interactive multi-turn chat session |
| `daemoneye ask <query>` | Send a single question to the AI |
| `daemoneye setup` | Print the systemd service file and recommended tmux config |
| `daemoneye scripts` | List scripts in `~/.daemoneye/scripts/` |
| `daemoneye schedule list` | List scheduled jobs and their status |
| `daemoneye schedule cancel <id>` | Cancel a scheduled job |
| `daemoneye schedule windows` | List leftover tmux windows from failed scheduled jobs (`de-*`) |

---

## Configuration

DaemonEye stores its configuration in `~/.daemoneye/config.toml`. The file is created automatically on first launch with default values.

### Full example

```toml
[ai]
provider = "anthropic"
api_key  = "sk-ant-..."
model    = "claude-sonnet-4-6"
prompt   = "sre"

# --- Local LLM (no API key required) ---
# [ai]
# provider = "ollama"
# model    = "llama3.2"
# # base_url = "http://localhost:11434/v1"   # default; change if Ollama runs elsewhere
# # context_window_tokens = 8192             # set if the model's context differs from the 32k default

# [ai]
# provider = "lmstudio"
# model    = "lmstudio-community/Meta-Llama-3-8B-Instruct-GGUF"
# # base_url = "http://localhost:1234/v1"    # default LM Studio port

# [masking]
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]

# [webhook]
# enabled = false
# port = 9393
# bind_addr = "127.0.0.1"   # set to "0.0.0.0" to expose on all interfaces
# secret = ""               # Bearer token; empty = no auth
# auto_analyze = true
# severity_threshold = "warning"   # "info" | "warning" | "critical"
# dedup_window_secs = 300
```

### `[ai]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `provider` | string | `"anthropic"` | AI backend to use. See valid values below. |
| `api_key` | string | `""` | API key for the chosen provider. If empty, falls back to the provider's environment variable. Not required for `ollama` or `lmstudio`. |
| `model` | string | `"claude-sonnet-4-6"` | Model name passed to the provider API. |
| `prompt` | string | `"sre"` | Name of a prompt file in `~/.daemoneye/prompts/` (without `.toml`). |
| `position` | string | `"bottom"` | Where `daemoneye setup` places the chat pane: `"bottom"`, `"top"`, `"right"`, or `"left"`. |
| `base_url` | string | *(provider default)* | Override the API base URL. Useful for pointing at a remote Ollama host, LM Studio instance, or any OpenAI-compatible proxy. |
| `context_window_tokens` | integer | *(model lookup)* | Override the context-window size in tokens. Set this for local models where the automatic lookup is inaccurate. |

### `[masking]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `extra_patterns` | list of strings | `[]` | Additional regex patterns to redact before context is sent to the AI. Each match is replaced with `<REDACTED>`. Built-in patterns always run; these extend the set. |

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

Example:

```toml
[masking]
extra_patterns = [
  "MYCO-[A-Z0-9]{32}",       # internal API token format
  "sk_live_[A-Za-z0-9]{32}", # Stripe live secret key
]
```

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
├── daemon/          # Background process: IPC server, session memory, background execution
│   ├── server.rs    # Core IPC connection loop and AI prompt orchestration
│   ├── executor.rs  # Dispatches AI tool calls
│   ├── background.rs # Manages dedicated tmux windows for background tasks
│   └── ...
├── cli/             # IPC client: chat interface, terminal rendering, subcommands
├── ipc.rs           # Shared data structures for Unix Socket communication
├── scheduler.rs     # ScheduledJob, ScheduleStore (JSON persistence), ScheduleKind, ActionOn, JobStatus
├── runbook.rs       # Runbook markdown loader (frontmatter parser, CRUD); watchdog AI system prompt builder
├── memory.rs        # Persistent memory: session (auto-loaded), knowledge (on-demand), incidents (search-only)
├── search.rs        # Keyword search across runbooks, scripts, memory, and events.jsonl
├── scripts.rs       # Script management: list, write (chmod 700), read, delete, resolve
├── sys_context.rs   # One-time host audit (OS, uptime, memory, processes, shell history)
├── tmux/
│   ├── mod.rs       # tmux interoperability layer (capture-pane, send-keys, create/kill job windows, etc.)
│   └── cache.rs     # Background poller that caches and summarizes all tmux panes
├── config.rs        # ~/.daemoneye/config.toml parsing, prompt loading, directory helpers
└── ai/
    ├── mod.rs
    ├── backends/    # Anthropic/OpenAI/Gemini streaming
    └── filter.rs    # Sensitive-data masking before API submission
```

---

## Command Audit Log

Every command the AI proposes — whether approved, denied, or timed out — is recorded as a JSON object in `~/.daemoneye/events.jsonl`:

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

When the AI requests a background command that requires `sudo`, the chat interface
prompts you for your password with terminal echo disabled. The password is piped
directly to `sudo -S` and is never written to disk, logged, or sent to the AI.

For foreground commands run in your terminal pane, you type the password directly
into the pane — DaemonEye never sees it.

---

## License

MIT
