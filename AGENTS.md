# AGENTS.md — DaemonEye

## Commands

```
cargo build --release              # release binary at target/release/daemoneye
cargo clippy --all-targets -- -D warnings   # CI gate (must pass)
cargo fmt --check                  # format check
cargo test                         # all tests (619 pass + 1 ignored)
cargo test <name>                  # single test
cargo test -- --nocapture          # tests with stdout
```

CI runs `cargo build --locked`, `cargo test --locked`, `cargo fmt --check`, `cargo clippy -- -D warnings -A clippy::too_many_arguments`.

## Critical invariants

- **`main()` is synchronous** — `libc::fork()` happens before the tokio runtime starts. Never move the fork into async. (`src/main.rs:206`)
- **Linux only** — uses `fork(2)`, Unix domain sockets, Linux-specific tmux hooks. Will not build on macOS/Windows.
- **Poisoned locks** — all `Mutex` lock sites use `.unwrap_or_log()` from `src/util.rs`. Never change to `.unwrap()`.

## Architecture at a glance

Rust daemon embedding an AI assistant into tmux. Forks to background, binds `~/.daemoneye/run/daemoneye.sock`, communicates with CLI clients via newline-delimited JSON over IPC.

| Directory | Role |
|---|---|
| `src/main.rs` | CLI entry, fork, subcommand routing |
| `src/ipc.rs` | `Request`/`Response` wire protocol |
| `src/daemon/` | IPC server, prompt assembly, streaming, executor, ghost shells, hooks |
| `src/daemon/executor/` | Tool dispatch, approval gate, bg/fg execution |
| `src/cli/` | Terminal UI, readline, chat/ask/notify clients |
| `src/ai/` | `AiClient` trait, tool defs, per-provider SSE backends (anthropic/openai/gemini) |
| `src/ai/filter.rs` | Regex sensitive-data masking (init at daemon start) |
| `src/tmux/` | All tmux subprocess calls; 2s background poll cache |
| `src/config.rs` | `~/.daemoneye/etc/config.toml` parsing |
| `src/scheduler.rs` | Schedule store (atomic JSON); `ActionOn::Ghost` routes to ghost |
| `src/memory/` | CRUD, FTS5 SQLite index, migrations, session tags |
| `src/session_store.rs` | Named session persistence (`meta.toml` + `messages.jsonl`) |
| `src/webhook.rs` | HTTP alert ingestion (axum port 9393), watchdog analysis, ghost trigger |
| `src/header.rs` | Artifact header parser/renderer; session-origin injection |
| `tests/integration.rs` | IPC round-trips, persistence, config parsing (no daemon/tmux needed) |

## Adding a new AI tool (8 steps)

1. `src/ai/types.rs`: `PendingCall::ToolName` variant + `to_tool_call()`/`id()`/`tool_name()` arms
2. `src/ai/types.rs`: `summary()` arm + `should_emit_tool_feedback()` arm (`true` = silent tool, `false` = approval-gated)
3. `src/ai/types.rs`: `AiEvent::ToolName` variant
4. `src/ai/tools.rs`: `ToolDef` in `TOOLS` slice + dispatch arm in `dispatch_tool_event()`
5. `src/daemon/stream.rs`: `AiEvent::ToolName` arm in streaming match
6. `src/daemon/executor.rs`: `PendingCall::ToolName` arm in `execute_tool_call()`
7. `assets/prompts/sre.toml`: document the new tool

Gemini tool defs are auto-generated from `TOOLS` via `render_gemini()` — no separate entry needed.

## tmux window naming

Format: `{prefix}{pane_num}-{unix_ts}-{cmd_slug}` (e.g. `de-bg-42-1712937600-cargo-build`)

| Prefix | Use |
|---|---|
| `de-bg-*` | Interactive background execution |
| `de-sj-*` | Regular scheduled jobs |
| `de-gs-bg-*` | Ghost shell background (webhook/interactive) |
| `de-gs-sj-*` | Ghost shell background (scheduler-triggered) |
| `de-gs-ir-*` | Ghost shell incident-response main windows |

Prefixes are used for GC filtering. The pane number uniquely identifies the tmux pane.

## Ghost shell essentials

- Detection signal: watchdog emits `GHOST_TRIGGER: YES`/`NO` as final line (`runbook.rs`)
- Each turn creates a **fresh** `(ai_tx, ai_rx)` channel — sender is moved, not cloned
- Turn budget: `max_ghost_turns` (runbook frontmatter, clamped to daemon ceiling of 20)
- Concurrency cap: default 3 active; `max_concurrent_ghosts = 0` disables
- Policy: non-sudo always allowed; sudo requires `auto_approve_scripts` entry + NOPASSWD sudoers rule
- `session_exists()` called before AI loop to guard against stale sessions

## Config & paths

- Config: `~/.daemoneye/etc/config.toml`
- Socket: `~/.daemoneye/run/daemoneye.sock`
- Daemon log: `~/.daemoneye/var/log/daemon.log`
- Events: `~/.daemoneye/var/log/events.jsonl`
- Sessions: `~/.daemoneye/var/sessions/<name>/` (named) vs `var/log/sessions/<id>.jsonl` (ephemeral)
- Scripts: `~/.daemoneye/scripts/` (chmod 700 on write)
- Runbooks: `~/.daemoneye/runbooks/`
- Memory: `~/.daemoneye/memory/` + FTS5 index at `var/index/memory.db`
- `read_file` blocked from `etc/config.toml` and `etc/prompts/sre.toml` only
- `edit_file` blocked from entire `~/.daemoneye/`

## Approval gates

Five scopes: terminal commands, sudo, scripts, runbooks, file edits.
`[A]pprove for session` grants class-wide approval. Configurable defaults in `[approvals]` section.
Ctrl+C or `/approvals revoke` resets to config defaults.

## Session context format

`idx:K` = 0-based window-relative pane index (what user sees with `ctrl+a q`).
`[OTHER_SESSIONS]` appended only when ≥2 tmux sessions exist.
`[Catch-up]` sent after ≥30s detachment, scans for event prefixes.

## Global statics

- `BG_DONE_TX` — broadcast sender for pane activity
- `FG_HOOK_COUNTER` — unique hook slot per concurrent watcher
- `DAEMON_START` — recorded at startup for uptime
- `BUFFER_COUNTER` — unique tmux buffer names for local-pane file reads
