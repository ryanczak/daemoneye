# DaemonEye - The AI Powered Operator 

DaemonEye is a lightweight background daemon that integrates with `tmux` to embed an AI assistant directly into your existing terminal workflow. It acts as an intelligent, context-aware senior principal site reliability engineer.

---

## Features

- **Native tmux Integration** — DaemonEye runs as a background process and interacts directly with your active `tmux` server.
- **Session State Caching** — The daemon actively monitors your `tmux` session, summarizing output from all panes to provide the AI with a global, real-time context of your terminal environment.
- **Embedded AI Assistant** — Streams responses from Anthropic Claude, OpenAI, or Google Gemini with automatic context capture and sensitive-data masking.
- **Collaborative Execution (Tool Calling)** — The AI can propose commands to fix issues. Each tool call presents a three-option prompt: `[y]es` (approve once), `[a]pprove session` (auto-approve all commands of this class for the rest of the session), or `[N]o`. Two independent approval classes exist — *regular* and *sudo* — so sudo commands always prompt separately until explicitly session-approved. Two execution modes: *background* (daemon subprocess, output summarized in chat and logged) and *foreground* (injected into your tmux active pane via `send-keys`, visible and interactive; the AI receives the output the moment the command finishes). Completion is detected automatically via Linux `/proc` child-process tracking so the injected command appears unmodified in your pane.
- **Execution Context Awareness** — On every first turn the AI is told the daemon's hostname and whether your terminal pane is local or connected to a remote host via SSH or mosh. This ensures the AI targets the right machine when choosing between background and foreground execution.
- **Sudo Password Integration** — Background commands that require `sudo` trigger a password prompt in the chat interface (echo disabled). Foreground sudo commands notify you to type your password in the terminal pane.
- **Command Audit Logging** — Every executed command is appended to `~/.daemoneye/commands.log` as a single structured line, including timestamp, session ID, execution mode, pane target, approval status, and output excerpt.
- **Multi-Turn Chat Memory** — The `chat` subcommand maintains full conversation history across turns within a session.
- **Readline-style Chat Input** — The chat input box supports history navigation (↑/↓ arrow keys), in-line cursor movement (←/→, Home/End, Ctrl+A/E), and kill shortcuts (Ctrl+K/U). The viewport scrolls horizontally for long inputs. History persists for the lifetime of the chat session.
- **IPC Architecture** — A lightweight CLI client communicates with the background daemon via a Unix Domain Socket (`/tmp/daemoneye.sock`) for instant, non-blocking interaction.

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

Daemon output (startup messages, errors) is written to `~/.daemoneye/daemon.log` by default. To stream it live:

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

Command execution events are written to `~/.daemoneye/commands.log` by default. To change the path or disable the audit log:

```sh
daemoneye daemon --command-log-file /var/log/daemoneye-commands.log
daemoneye daemon --no-command-log
```

You can also manage the daemon with systemd — run `daemoneye setup` for the service file.

### 2. Configure tmux

Run `daemoneye setup` to get the recommended `tmux` configuration and add the output to your `~/.tmux.conf`:

```sh
# ~/.tmux.conf
bind-key T split-window -h -e "DAEMONEYE_SOURCE_PANE=#{pane_id}" 'daemoneye chat'
```

Reload your tmux config:

```sh
tmux source-file ~/.tmux.conf
```

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
| `daemoneye daemon --command-log-file FILE` | Write command audit log to `FILE` |
| `daemoneye daemon --no-command-log` | Disable command audit logging |
| `daemoneye stop` | Stop the daemon gracefully |
| `daemoneye ping` | Check whether the daemon is running |
| `daemoneye logs [--log-file FILE]` | Tail the daemon log (wraps `tail -f`) |
| `daemoneye chat` | Start an interactive multi-turn chat session |
| `daemoneye ask <query>` | Send a single question to the AI |
| `daemoneye setup` | Print the systemd service file and recommended tmux config |

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

# [masking]
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]
```

### `[ai]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `provider` | string | `"anthropic"` | AI backend to use. See valid values below. |
| `api_key` | string | `""` | API key for the chosen provider. If empty, falls back to the provider's environment variable. |
| `model` | string | `"claude-sonnet-4-6"` | Model name passed to the provider API. |
| `prompt` | string | `"sre"` | Name of a prompt file in `~/.daemoneye/prompts/` (without `.toml`). |
| `position` | string | `"bottom"` | Where `daemoneye setup` places the chat pane: `"bottom"`, `"top"`, `"right"`, or `"left"`. |

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

#### Valid `provider` values

| Value | Provider | API endpoint |
|---|---|---|
| `"anthropic"` | Anthropic (Claude) | `https://api.anthropic.com/v1/messages` |
| `"openai"` | OpenAI (or any OpenAI-compatible API) | `https://api.openai.com/v1/chat/completions` — override with `OPENAI_API_BASE` env var |
| `"gemini"` | Google Gemini | `https://generativelanguage.googleapis.com/v1beta/` |

### Environment variables

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | API key for the `anthropic` provider (used if `api_key` is not set in config). |
| `OPENAI_API_KEY` | API key for the `openai` provider (used if `api_key` is not set in config). |
| `GEMINI_API_KEY` | API key for the `gemini` provider (used if `api_key` is not set in config). |
| `OPENAI_API_BASE` | Override the base URL for the `openai` provider (useful for local models via Ollama, LM Studio, etc.). |

---

## Project Structure

```
src/
├── main.rs          # CLI entry point — parses subcommands (daemon, stop, ping, logs, chat, ask, setup)
├── daemon.rs        # Background process: IPC server, AI orchestration, tool execution, session polling
├── client.rs        # IPC client: chat, ask, ping, stop, logs
├── ipc.rs           # Shared data structures for Unix Socket communication
├── sys_context.rs   # One-time host audit (OS, uptime, memory, processes, shell history)
├── tmux/
│   ├── mod.rs       # tmux interoperability layer (capture-pane, send-keys, list-panes, etc.)
│   └── cache.rs     # Background poller that caches and summarizes all tmux panes
├── config.rs        # ~/.daemoneye/config.toml parsing and prompt loading
└── ai/
    ├── mod.rs
    ├── client.rs    # AiClient trait + Anthropic/OpenAI/Gemini streaming implementations
    └── filter.rs    # Sensitive-data masking before API submission
```

---

## Command Audit Log

Every command the AI proposes — whether approved, denied, or timed out — is recorded as a single line in `~/.daemoneye/commands.log`:

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

To register organisation-specific patterns, add them to your config (see [masking] below). Built-in patterns always run — user patterns extend the set, never replace it.

### Sudo passwords

When the AI requests a background command that requires `sudo`, the chat interface
prompts you for your password with terminal echo disabled. The password is piped
directly to `sudo -S` and is never written to disk, logged, or sent to the AI.

For foreground commands run in your terminal pane, you type the password directly
into the pane — DaemonEye never sees it.

---

## License

MIT
