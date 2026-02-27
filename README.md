# T1000

T1000 is a lightweight background daemon that integrates natively with `tmux` to embed an AI assistant directly into your existing terminal workflow. It acts as an intelligent, context-aware pair-sysadmin.

---

## Features

- **Native tmux Integration** — T1000 runs as a background process and interacts directly with your active `tmux` server.
- **Session State Caching** — The daemon actively monitors your `tmux` session, summarizing output from all panes to provide the AI with a global, real-time context of your terminal environment.
- **Embedded AI Assistant** — Streams responses from Anthropic Claude, OpenAI, or Google Gemini with automatic context capture and sensitive-data masking.
- **Collaborative Execution (Tool Calling)** — The AI can propose commands to fix issues. Upon your approval, the daemon securely injects and executes the commands directly in your active `tmux` pane.
- **Multi-Turn Chat Memory** — The `chat` subcommand maintains full conversation history across turns within a session.
- **IPC Architecture** — A lightweight CLI client communicates with the background daemon via a Unix Domain Socket (`/tmp/t1000.sock`) for instant, non-blocking interaction.

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
cd t1000
cargo build --release
```

The compiled binary is at `target/release/t1000`.

To install it into your `~/.cargo/bin` path:

```sh
cargo install --path .
```

---

## Usage

T1000 requires the daemon to be running in the background.

### 1. Start the daemon

```sh
t1000 daemon
```

Daemon output (startup messages, errors) is written to `~/.t1000/daemon.log` by default. To stream it live:

```sh
t1000 logs
```

To write logs to a custom path:

```sh
t1000 daemon --log-file /var/log/t1000.log
```

You can also manage the daemon with systemd — see the provided `t1000.service` file.

### 2. Configure tmux

Run `t1000 setup` to get the recommended `tmux` configuration and add the output to your `~/.tmux.conf`:

```sh
# ~/.tmux.conf
bind-key T split-window -h 't1000 chat'
```

Reload your tmux config:

```sh
tmux source-file ~/.tmux.conf
```

### 3. Interact with the AI

Press your configured hotkey (e.g., `Ctrl+b T`) inside a tmux session to open a new split pane connected to T1000. Ask it questions about errors in your other panes, or request it to execute commands.

You can also interact directly from the command line:

```sh
# Single question (non-interactive)
t1000 ask "why is nginx returning 502?"

# Interactive multi-turn chat
t1000 chat
```

### All subcommands

| Command | Description |
|---|---|
| `t1000 daemon [--log-file FILE]` | Start the background daemon |
| `t1000 stop` | Stop the daemon gracefully |
| `t1000 ping` | Check whether the daemon is running |
| `t1000 logs [--log-file FILE]` | Tail the daemon log (wraps `tail -f`) |
| `t1000 chat` | Start an interactive multi-turn chat session |
| `t1000 ask <query>` | Send a single question to the AI |
| `t1000 setup` | Print the recommended tmux configuration |

---

## Configuration

T1000 stores its configuration in `~/.t1000/config.toml`. The file is created automatically on first launch with default values.

### Full example

```toml
[ai]
provider = "anthropic"
api_key  = "sk-ant-..."
model    = "claude-sonnet-4-6"
prompt   = "sre"
```

### `[ai]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `provider` | string | `"anthropic"` | AI backend to use. See valid values below. |
| `api_key` | string | `""` | API key for the chosen provider. If empty, falls back to the provider's environment variable. |
| `model` | string | `"claude-sonnet-4-6"` | Model name passed to the provider API. |
| `prompt` | string | `"sre"` | Name of a prompt file in `~/.t1000/prompts/` (without `.toml`). |
| `position` | string | `"bottom"` | Where `t1000 setup` places the chat pane: `"bottom"`, `"top"`, `"right"`, or `"left"`. |

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
├── config.rs        # ~/.t1000/config.toml parsing and prompt loading
└── ai/
    ├── mod.rs
    ├── client.rs    # AiClient trait + Anthropic/OpenAI/Gemini streaming implementations
    └── filter.rs    # Sensitive-data masking before API submission
```

---

## Security Notes

Before sending terminal context to an AI provider, T1000 applies a regex-based
filter that masks:

- AWS access key IDs (`AKIA...`)
- Password, token, secret, and API key assignments
- PEM private key blocks
- Credit card numbers (16-digit grouped format)
- US Social Security Numbers

Masked values are replaced with placeholder tokens (`<REDACTED>`, `<AWS_KEY>`,
etc.). Review the context shown in the AI pane before submitting if you handle
highly sensitive data.

---

## License

MIT
