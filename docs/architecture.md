# DaemonEye Architecture

> Reverse-engineered from the existing codebase on 2026-06-21 as the design
> baseline for rexyMCP-driven development. This document describes the system as
> it is built today; the milestone roadmap (§5) is the forward-looking part and
> is scoped with the principal engineer before any phase is drafted.

DaemonEye is a Rust daemon that embeds an AI assistant directly into `tmux`. It
runs in the background, watches the user's terminal panes, and lets the user
converse with an LLM that can *see* the terminal context and *act* on it by
running commands, editing files, scheduling jobs, and delegating to autonomous
"ghost shells." Every action that touches the user's environment passes through
an approval gate.

---

## 1. System layers

The codebase decomposes into roughly four layers. Each is a directory (or small
set of files) with a single responsibility and a narrow interface to its
neighbors.

### 1.1 Transport & process layer

The daemon lifecycle and client↔daemon wire protocol.

- **`src/main.rs`** — synchronous CLI entry point. `main()` stays synchronous so
  `libc::fork()` can run *before* the tokio runtime starts (a hard invariant —
  forking inside an async context is unsound). Routes subcommands.
- **`src/daemon/mod.rs`** — `run_daemon`, the global statics (`DAEMON_START`,
  `BG_DONE_TX`, `FG_HOOK_COUNTER`, `BUFFER_COUNTER`), and `supervise()` — an
  exponential-backoff task supervisor (1→2→…→30 s cap, resets after 60 s stable)
  that keeps the four critical tasks (cache poller, scheduler, session cleanup,
  webhook) alive across panics.
- **`src/ipc.rs`** — the full wire protocol: the `Request` and `Response` enums,
  serialized as newline-delimited JSON over a Unix domain socket
  (`~/.daemoneye/daemoneye.sock`).
- **`src/cli/`** — the client side: terminal rendering (`render.rs`), readline
  input (`input.rs`), streaming display (`stream`), session-level approval
  state, and the `chat` / `ask` / `notify` / `status` commands. Slash commands
  (`/model`, `/session`) are parsed here.

### 1.2 Orchestration layer (`src/daemon/`)

The brain: turns a `Request::Ask` into an AI conversation that can act.

- **`server.rs`** — IPC dispatch and the `handle_ask` orchestrator; utility
  helpers (`build_catchup_brief`, pane-id validation).
- **`stream.rs`** — `run_conversation_loop`: the AI event streaming loop, tool
  execution, and response persistence.
- **`prompt.rs`** / **`memory_prompt.rs`** — system-prompt assembly via
  `PromptCtx`; tiered memory injection (stable ambient block + dynamic
  turn-relevant block).
- **`executor/`** — tool-call dispatch and the approval gate (`ToolCallOutcome`);
  coordinates background vs foreground execution; `ArtifactCtx` stamps
  session-origin onto created artifacts.
- **`background.rs`** — background command execution in dedicated `de-bg-*` tmux
  windows on the daemon host, with `pane-died`-hook completion detection and GC.
- **`hook.rs`** — handlers for the tmux-hook IPC notifications (`NotifyActivity`,
  `NotifyComplete`, `NotifyFocus`, `NotifyWindowChanged`, `NotifyResize`,
  client attach/detach, session-created, …).
- **`ghost.rs`** / **`policy.rs`** / **`briefing.rs`** — the Ghost Shell
  subsystem (see §3).
- **`digest.rs`** / **`auto_name.rs`** — conversation compaction at 30 messages;
  session auto-naming.

### 1.3 AI provider layer (`src/ai/`)

A provider-agnostic abstraction over three LLM backends.

- **`mod.rs`** — the `AiClient` trait, `dispatch_tool_event()`, and the
  `CircuitBreaker` (threshold 5 failures, 60 s cooldown) wrapping `send_with_retry`.
- **`types.rs`** — `PendingCall` (one variant per tool), `AiEvent`, `Message`,
  `AiUsage`. The single source of truth for what tools exist.
- **`tools.rs`** — the `TOOLS` slice: one `ToolDef` per tool, shared by all three
  backends (Gemini definitions auto-generated via `render_gemini(TOOLS)`).
- **`backends/`** — per-provider SSE streaming (Anthropic / OpenAI / Gemini).
- **`filter.rs`** — regex-based sensitive-data masking, initialized at daemon
  start and applied to all captured terminal output and artifact content.

Models are configured as `[models.<name>]` TOML tables; `Config::resolve_model()`
resolves by key (falling back to `"default"`), and sessions can switch models at
runtime via `/model`.

### 1.4 tmux integration & persistence layer

- **`src/tmux/`** — every `tmux` subprocess call (one function per operation),
  the 2 s background poll (`cache.rs` → `SessionCache`, `PaneState`,
  `get_labeled_context()`), and session-level helpers (`session.rs`). This is
  the *only* place that shells out to `tmux`.
- **Persistence** — `scheduler.rs` (`ScheduleStore`, atomic JSON),
  `scripts.rs` (script CRUD, chmod 700, sudoers install), `runbook.rs` (TOML
  runbooks), `session_store.rs` (named-session persistence), `memory/` (CRUD +
  FTS5 index with grep fallback), `config.rs` (`~/.daemoneye/config.toml`),
  `header.rs` (artifact frontmatter / comment-header injection).
- **`agents/`** — named agents: config CRUD, `ToolPolicy` (allow/deny tool
  lists), and the mailbox for agent-to-agent delegation.
- **`webhook.rs`** — axum 0.8 HTTP alert ingestion (Alertmanager / Grafana /
  generic), dedup, masking, and AI-watchdog routing into ghost shells.

---

## 2. Major data flows

### 2.1 Interactive request/response

1. The CLI client reads `$TMUX_PANE`, connects to the socket, and sends
   `Request::Ask` (carrying `tmux_session` and a client-resolved `target_pane`).
2. The daemon captures the user's pane, applies the masking filter, assembles
   the system prompt + a `[SESSION TOPOLOGY]` / `[ACTIVE PANE]` / … context
   snapshot, and streams tokens from the resolved model.
3. On a tool call the daemon sends `Response::ToolCallPrompt`; the client shows
   the target pane (highlighted) and prompts `[Y]es / [A]pprove session / [N]o /
   or type a message to redirect`.
4. Approved commands run **background** (daemon-host `de-bg-*` window) or
   **foreground** (injected into the user's pane via `send-keys`, with a
   three-way completion detection: interactive-prompt pattern, remote
   output-stability poll, or local PID-change poll).
5. The daemon returns `Response::ToolResult`; the loop continues until the model
   produces a final answer.

A typed message at the approval prompt aborts the pending tool chain and
re-enters the loop with the text as a plain user turn (course-correction without
a synthetic tool error).

### 2.2 Event-driven / asynchronous flows

- **tmux hooks → IPC notifications** keep the `SessionCache` fresh without
  polling lag (focus, window-change, resize, client attach/detach, new session).
- **Catch-up brief**: after a client is detached ≥ 30 s and events accumulate,
  the next `Ask` emits a `Response::SystemMsg` summarizing what happened (built
  by `build_catchup_brief()` scanning new event-prefixed messages).
- **Webhook → watchdog → ghost**: an inbound alert is parsed, masked, and run
  past an AI watchdog (`use_tools=false`) whose final-line `GHOST_TRIGGER: YES`
  routes it into an autonomous ghost shell.
- **Scheduler**: `ScheduleStore` fires `Once` / `Every` / `Cron` jobs whose
  `ActionOn` is `Alert`, `Script`, or `Ghost { runbook }`.

### 2.3 Knowledge flow

Memory, runbooks, and scripts created inside a *named* session are stamped with
`session_origin` frontmatter and tracked on the session; unnamed sessions get a
retroactive backfill on first save. Ghost sessions are excluded. Memory is
indexed in an FTS5 SQLite db with a grep fallback for search.

---

## 3. The Ghost Shell subsystem

Ghost shells are autonomous, runbook-driven AI loops that act *without a human at
the prompt* — the system's answer to "respond to this alert at 3 a.m." They are
the highest-risk subsystem and carry the most guardrails:

- **Runbook-bound**: each ghost follows a named TOML runbook (frontmatter sets
  `model`, `max_ghost_turns`, tool policy, agent).
- **Two independent policy gates**: `GhostPolicy` (per-runbook approval rules)
  and `ToolPolicy` (agent-level allow/deny) must *both* pass; a ghost with no
  policy refuses to proceed.
- **Concurrency cap**: `max_concurrent_ghosts` (default 3).
- **Delegation depth cap**: coordinator (depth 0) → specialist (depth 1) only;
  depth 2+ is rejected.
- **Lifecycle events** (`[Ghost Shell Started/Completed/Failed/Skipped]`) are
  injected into all sessions for catch-up visibility, and a mailbox carries the
  final result back to a delegating coordinator.
- **Window prefixes** (`de-gs-bg-*`, `de-gs-sj-*`, `de-gs-ir-*`) segregate ghost
  windows for GC and listing.

---

## 4. Non-goals

What DaemonEye explicitly does **not** do:

- **No terminal multiplexer of its own.** It is a tmux *client*, not a
  replacement. Everything visual is a tmux pane/window; DaemonEye never draws its
  own UI surface.
- **No unattended action without a gate.** Interactive tool calls require
  per-call approval; autonomous ghost actions require a runbook + dual policy.
  There is no "just run everything" mode.
- **No provider lock-in.** The three backends sit behind one trait; no provider's
  wire format leaks above `src/ai/backends/`.
- **No secret exfiltration surface.** The masking filter runs on all captured
  output and artifact content; `read_file` is blocked from the credential files
  (`etc/config.toml`, `etc/prompts/sre.toml`) and `edit_file` from `~/.daemoneye/`.
- **No cross-host orchestration fabric.** A single daemon serves the tmux
  sessions on its host; remote panes are reached *through* tmux (SSH/mosh), not
  via a DaemonEye agent on the far side.
- **No durable conversation store beyond named sessions.** Ephemeral per-session
  JSONL logs are for crash recovery, not a queryable history product.

---

## 5. Milestone roadmap

DaemonEye is an established codebase; the work below the line is **shipped** and
forms the baseline. Forward milestones are intentionally left unscoped here —
they are defined with the principal engineer (via `/rexymcp:architect`) before
any phase doc is drafted, so the roadmap reflects real intent rather than
speculation.

### Shipped baseline (pre-rexyMCP)

- **Pane/session context** — topology inventory, CWD/title/env, scroll & copy
  mode, dead-pane status, activity timestamps, client viewport, cross-session
  context, pipe-pane logging.
- **Foreground/background execution** — hook-based completion, PID-based local
  completion, respawn-pane retry, persistent background windows.
- **Knowledge system** — runbooks, persistent memory (with FTS5 index and G2
  schema), repository search, named agents with tool policies.
- **Ghost Shell architecture** — watchdog detection, scheduled ghost jobs,
  concurrency/depth caps, delegation mailbox, briefings.
- **Multi-model support** — per-session model switching, per-runbook model
  override, circuit breaker, `daemoneye status`.
- **Session persistence** — named sessions, auto-naming, session-origin
  stamping, `session import`.
- **Webhook ingestion** — Alertmanager/Grafana/generic parsers, rate limiting,
  AI auto-analysis.

### Next milestone — to be defined

`docs/dev/NEXT.md` currently points at **none**. The next milestone README is
written under `docs/dev/milestones/M<n>-<slug>/` once its goal, scope, and
non-scope are agreed with the principal engineer. Run `/rexymcp:architect` to
scope it, then `/rexymcp:architect next` to draft the first phase.
