# DaemonEye Architecture

> Reverse-engineered from the existing codebase on 2026-06-21 as the design
> baseline for rexyMCP-driven development. This document describes the system as
> it is built today; the milestone roadmap (§5) is the forward-looking part and
> is scoped with the principal engineer before any phase is drafted.
>
> **Related docs.** This is the *concise* design baseline. For implementation
> depth (IPC variants, completion-detection internals, cost schema, hook
> formats) see [`design-reference.md`](design-reference.md); for product vision and
> requirements see [`PRODUCT_DEFINITION.md`](PRODUCT_DEFINITION.md),
> [`REQUIREMENTS.md`](REQUIREMENTS.md), and [`ROADMAP.md`](ROADMAP.md).

DaemonEye is a Rust daemon that embeds an AI assistant directly into `tmux`. It
runs in the background, watches the user's terminal panes, and lets the user
converse with an LLM that can *see* the terminal context and *act* on it by
running commands, editing files, scheduling jobs, and delegating to autonomous
"ghost shells."

Its organizing principle is a **trust spectrum**: autonomy is not binary. The
same engine operates across four levels of earned trust —
(1) **supervised** pair-work where every tool call is approved per-call;
(2) **session-scoped** trust where the user approves a class of action for the
rest of the session; (3) **scheduled / watchdog** operations that run
unattended against pre-vetted runbooks and scripts; and
(4) **autonomous ghost shells** that respond to alerts on their own under
runbook + policy control. Every action that touches the user's environment
passes through a gate appropriate to its trust level; there is no global
approval-bypass.

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
  (`~/.daemoneye/var/run/daemoneye.sock`, resolved via
  `config::default_socket_path()`). IPC payloads are capped (1 MiB).
- **`src/cli/`** — the client side: terminal rendering (`render.rs` +
  `render_ratatui.rs` + the `markdown` submodule), readline input
  (`input/{tty,editor}`), streaming display (`stream`), session-level approval
  state, and the `chat` / `ask` / `notify` / `status` commands. Slash commands
  (`/model`, `/session`) are parsed in the `commands/chat` submodule. The chat UI
  uses a **`ratatui` inline viewport** (M2): the transcript commits to native
  terminal scrollback via `Terminal::insert_before`, and only the input box +
  status bar occupy a fixed bottom region. There is no DECSTBM scroll region — the
  previous absolutely-positioned-chrome path (which corrupted on tmux window
  switches) was removed. The input box is a full multi-line editor (visible cursor,
  word-wrap, multi-line paste); a two-press ESC / Ctrl+C interrupts a streaming turn.

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
- **`digest.rs`** / **`auto_name.rs`** — conversation compaction driven by
  prompt-token pressure (minimum `DIGEST_THRESHOLD` = 20 messages; history is
  bounded to `MAX_HISTORY` = 80), preserving tool-call/result pairs across the
  cut; session auto-naming.
- **`stats.rs`** + **`src/cost.rs`** / **`src/cli/status.rs`** — per-turn token
  accounting (input / output / cache-read / cache-write), per-model pricing, and
  cost attribution to chat vs ghost vs agent. Drives the status-bar cost
  readout, `daemoneye status`, and the `daemoneye costs` CLI.

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

**Core vs. deferred tools (on-demand loading).** Each `ToolDef` carries a
`deferred_group: Option<&'static str>`. `None` marks a **core** tool, rendered into
every request's tool schema. `Some(group)` marks a **deferred** tool — rarely used,
omitted from the default render to keep per-request context small, and grouped under
`group`. The per-request tool set a backend sends is therefore
`core ∪ {deferred tools the session has loaded}`, computed in
`get_*_tool_definition(loaded)` (the pure `render_*` helpers render whatever
selection they are handed). The model pulls a group in by calling the core
`load_tools` tool, whose own description carries a catalog generated from the
deferred set; the daemon records the loaded names in `SessionEntry.loaded_tools`, and
their schemas appear on subsequent turns. The split is self-declaring (adding a tool
sets one field; the compiler forces it) and the loaded set is a `HashSet<String>`
(an `unload_tools` mirror is a future, additive extension). This is distinct from
`ToolPolicy`/`GhostPolicy`, which gate tools at **execution** time, not render time.

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
  runbooks), `session_store.rs` (named-session persistence), `memory/` (CRUD + a SQLite
  FTS5 index at `var/index/memory.db`), `config.rs` (`~/.daemoneye/config.toml`),
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
indexed in a SQLite FTS5 database at `var/index/memory.db`, maintained
best-effort on every add/update/delete and rebuilt by `reconcile_index()`
whenever the index is found empty. To force a rebuild when the index is
populated but stale, run `daemoneye reindex`; it rebuilds from the memory files
on disk, reports the row count before and after, and is safe to run while the
daemon is up because the rebuild is a single transaction. Recall merges three
candidate sources — tag overlap, one-hop `relates_to` expansion, and
BM25-ranked FTS5 hits against the user's turn. The grep scan in `src/search.rs`
serves the `search_repository` tool and is **not** a fallback for recall.

### 2.4 Remote-host execution model

DaemonEye gives its agents the full agency of a human operator on remote hosts —
**without** storing anything on them. The governing principle: the **daemon host
is the only place DaemonEye keeps its managed artifacts** (scripts, runbooks,
memory, config). A remote host is an *execution target*, never a *storage
target* — because the daemon may lack write privileges there, the remote
filesystem may be read-only, or its only writable storage may be volatile.

Tools fall into three classes by how they treat a remote:

- **Managed-artifact tools** — `write_script` / `read_script` / `list_scripts` /
  `delete_script`, `write_runbook` / … / `delete_runbook`, `add_memory` / …
  These curate DaemonEye's own knowledge base and are **daemon-host only**; they
  carry no `target_pane` and never write to a remote.
- **Operator-filesystem tools** — `read_file`, `edit_file`. These act on whatever
  host `target_pane` points at: editing a remote `/etc/...` through an existing
  SSH/mosh pane is legitimate operator parity, not artifact storage.
- **Execution tools** — `run_terminal_command`, remote *script execution*, and
  ghost `ssh_target` / runbook loops. These route *execution* to the remote. A
  daemon-host **script** is instantiated on the remote *transiently* and run; a
  daemon-host **runbook** is followed by the agent loop with its issued commands
  routed to the remote. The runbook file itself never leaves the daemon host.

Remote script execution has two mechanisms:

- **Default — stream, no remote disk.** The hex-decoded script content is piped
  to a remote interpreter's stdin (`… | bash -s -- <args>`), so nothing touches
  the remote filesystem. This is what makes read-only / volatile remotes work.
- **Sudo exception — persistent materialize.** A NOPASSWD `sudoers` rule can only
  authorize a *fixed path*; neither streamed stdin nor a random `mktemp` path can
  be pre-authorized. So a remote script that must run under `sudo` is materialized
  to the sudoers-authorized `~/.daemoneye/scripts/<name>` path before execution.
  This is the one case that requires a writable, persistent remote location plus a
  matching remote sudoers rule; where that is unavailable, remote-sudo-script
  execution fails loud rather than silently degrading.

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
- **No web UI.** DaemonEye is terminal-native by deliberate choice; the audit
  log is plain JSONL that existing tools (`grep`, `jq`, Grafana) consume.
- **No unattended action without a gate, and no global approval-bypass.**
  Interactive tool calls require per-call (or session-scoped) approval;
  autonomous ghost actions require a runbook + dual policy. There is no "just run
  everything" switch that flattens the trust spectrum.
- **No long-running agents that own infrastructure.** Named agents are reusable
  *configuration identities* (prompt, model, tool policy, memory namespace),
  not persistent processes. They are instantiated per ghost/session and exit
  when the work does.
- **No provider lock-in.** The three backends sit behind one trait; no provider's
  wire format leaks above `src/ai/backends/`.
- **No secret exfiltration surface.** The masking filter runs on all captured
  output and artifact content; `read_file` is blocked from the credential files
  (`etc/config.toml`, `etc/prompts/sre.toml`) and `edit_file` from `~/.daemoneye/`.
- **No DaemonEye agent on the far side, and no remote artifact storage.** A
  single daemon serves the tmux sessions on its host; it acts on remote hosts by
  routing *execution* there (through an existing SSH/mosh pane, or a ghost
  `ssh_target`), never by running a second DaemonEye on the remote and never by
  storing its managed artifacts (scripts, runbooks, memory) on the remote. Remote
  hosts are execution targets, not storage targets (see § 2.4). Operator parity —
  an agent doing on a remote whatever a human at that pane could do — is a goal;
  a far-side daemon or remote-resident DaemonEye state is not.
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
- **Knowledge system** — runbooks, persistent memory (with a BM25-ranked FTS5
  index and per-agent namespacing), repository search, named agents with tool
  policies and persistent briefings.
- **Scheduled operations** — `Once` / `Every` / `Cron` jobs with `Alert` /
  `Script` / `Ghost` actions; the unattended (Level 3) tier of the trust
  spectrum, with `daemoneye install-sudoers` for pre-vetted privileged scripts.
- **Ghost Shell architecture** — watchdog detection, scheduled ghost jobs,
  concurrency/depth caps, delegation mailbox, briefings.
- **Multi-model support** — per-session model switching, per-runbook model
  override, circuit breaker, `daemoneye status`.
- **Cost accounting** — per-turn token + cost tracking attributed to chat /
  ghost / agent, status-bar readout, and the `daemoneye costs` CLI.
- **Session persistence** — named sessions, auto-naming, session-origin
  stamping, `session import`.
- **Webhook ingestion** — Alertmanager/Grafana/generic parsers, rate limiting,
  AI auto-analysis.

### Shipped under rexyMCP (M1–M3)

Three milestones have been delivered through the architect/executor workflow.
Each has a retrospective in its `docs/dev/milestones/M<n>-<slug>/README.md`.

- **M1 — Agent Tooling Improvements** (11 phases, complete 2026-06-23). Full
  operator parity on local + SSH-connected remote hosts under the §2.4 model:
  remote `read_file`/`edit_file`, streamed (no-remote-disk) remote script
  execution with a sudo-only materialize exception, and the interactive-pane
  analogue. Security hardening interleaved per subsystem (SSH escaping, sudoers
  quoting, script-name allowlist, symlink/canonicalization guards, namespace
  ACL). Foreground completion correctness (the `DE_EXIT` latch + non-zero exit
  surfacing). **On-demand tool loading** — the core/deferred `TOOLS` split and the
  `load_tools` tool described in §1.3.
- **M2 — TUI Renderer Overhaul** (16 phases, complete 2026-06-27). Replaced the
  DECSTBM scroll-region chat renderer with the `ratatui` inline-viewport model
  described in §1.1 (committed scrollback + fixed bottom region), fixing
  window-switch transcript corruption; added the multi-line input editor and the
  two-press streaming interrupt. Also split the ten remaining >1000-line files
  into cohesive submodule directories (code-issue C5).
- **M3 — Polish & Maintenance** (10 phases, complete 2026-06-28). Correctness and
  hermeticity fixes (`TEST_HOME_LOCK` discipline), UX papercuts (one approval-prompt
  format, no `{:?}` error leak, ellipsis truncation markers, completed `/help`),
  and codebase health (the `daemon/utils.rs` + `webhook.rs` splits, the 7
  `TODO(M2)` signature consolidations, and first-ever unit coverage for the
  `executor/knowledge/` handlers). No behavior regressions.

### Shipped — M4 Context Management Overhaul

Scoped 2026-07-07 (PE sign-off). Design doc:
[`docs/design/context-management.md`](design/context-management.md); milestone
README: `docs/dev/milestones/M4-context-management/README.md` (ten phases).
Goal: survive hundreds-of-days daemon uptimes and thousands-of-turns sessions —
event-log rotation, token-budgeted compaction with hysteresis, an append-only
per-session archive, an epoch/chapter summary chain (O(log turns) in-context
representation), a `recall_context` tool over the archive, asynchronous
compaction, session-meta persistence across restarts, and ghost-session
coverage. The pre-rexyMCP [`ROADMAP.md`](ROADMAP.md) remains a reference
baseline, not a commitment.

### Shipped — M5 UX & Stability

Closed 2026-07-30. Milestone README:
`docs/dev/milestones/M5-ux-stability/README.md` (46 phases, 36 approved first try).
Closed two axes rather than the three UX papercuts it was scoped for. **Stalls:** all
three mechanisms from [`docs/design/daemon-stalls.md`](design/daemon-stalls.md) are
shut *and self-reporting* — a re-entrant `SessionStore` acquisition panics on an
always-on assertion instead of deadlocking; every tmux subprocess is timeout-bounded
(44 via `bounded_output`, 26 via `off_runtime`, 9 direct); and a silent AI provider
fails at a 120 s idle read with a diagnosable message. **Instance ownership**, added
mid-milestone after a live two-daemon incident
([`docs/design/daemon-instance.md`](design/daemon-instance.md)): one daemon per
`$HOME` enforced by an exclusive `flock` acquired before any startup side effect,
liveness reporting that distinguishes wedged from dead, a PID on every event record,
and a fork readiness handshake so `daemoneye daemon` stops reporting success before
the child has bound its socket.

### Shipped — M6 Verification & Hygiene

Scoped 2026-07-30 (PE sign-off); closed 2026-07-31. Milestone README:
`docs/dev/milestones/M6-verification-and-hygiene/README.md` (thirteen phase docs —
phase 06 split into 06a/06b — all delivered). The milestone delivered: an isolated
test harness with a throwaway `HOME` and private tmux server; a path-audit gate that
fails on any wrong or superseded path literal in prompt and knowledge-memory assets;
corrections to six pre-`var/` stale paths across seven knowledge memories; an
operator-facing `daemoneye audit-prompts` command (report-only, never rewriting); a
severity gate that logs and emits events on every discard; an end-to-end
webhook→ghost-shell pipeline scenario against a canned-AI stub; a unified artifact
lifecycle policy table with a test that fails on any uncovered class; `daemon.log`
rotation under that policy; operator-tunable retention for `panes/` and agent
mailboxes; a pane-preference redesign using fingerprint validation and pruning;
runtime-tree hygiene (orphan removal, `lib/` stopped on install, doc-comment
corrections); and this phase's own roadmap correction. Phase 12 is the last in-scope
phase; the milestone retrospective and close belong to the human gate.

### Active milestone — M11 Unified Knowledge Index

Scoped 2026-08-03 (PE sign-off); design doc `docs/design/knowledge-index.md`;
milestone README `docs/dev/milestones/M11-knowledge-index/README.md`. Extends
the M7 FTS5 index into a unified knowledge index: five corpora (memories,
runbooks/scripts, epoch narratives, archived turns, events) in one
`var/index/memory.db`, with contentless tables + byte-offset sidecar maps for
the two high-volume append-only corpora. Read surfaces upgraded in-milestone:
`recall_context` query mode (BM25, cross-session `scope`), `search_repository`
(index routing, new kinds), and per-turn prompt assembly (real BM25 scores, no
directory walks). Prerequisite phase closes the two mask-on-write gaps
(`append_epoch`, `log_event`) before their corpora are indexed. Seven phases,
none dispatched yet.

**M10 — Residual Hygiene closed 2026-08-02**, three phases, all
`approved_first_try`, zero bugs. It cleared the four items carried out of M7 and
M8: the tty tests now fail in five seconds where a starved `read_key` used to
hang the suite indefinitely; the memory category→directory mapping is derived
from `MemoryCategory::ALL` in all three callers instead of being hardcoded; the
last real-clock sleep in the test suite is gone; and `daemoneye reindex` is
documented in both `CLAUDE.md` and this file, with a `doc_truth` tripwire that
reads only the durable part of the file so a mention surviving in the
milestone-roadmap section cannot satisfy it.

**M9 — Operator Tooling closed 2026-08-02**, one phase. `daemoneye reindex`
rebuilds the memory index from the files on disk and reports the row count before
and after. It needs no daemon, tolerates a bare `$HOME`, and is idempotent; the
rebuild is a single transaction, so it is safe to run while the daemon is serving
searches. Before it, `reconcile_index()` had one caller that fired only when the
index was empty, leaving a *stale* index unreachable and unfixable short of
deleting `var/index/memory.db` by hand.

**M8 — Test Suite Reliability closed 2026-08-02**, two phases. The isolation
suite went from a measured **5 failures / 100 runs** to **0 / 300**: the test
harness's port allocator released each probe listener before its consumer bound
the port. It now holds the listener until hand-off.

**M7 — Memory Search & Maintenance closed 2026-08-02**, ten phases. It made
memory search real (BM25-ranked FTS5 over `var/index/memory.db`, maintained on
every write, with `reconcile_index()` covering the fresh-install case), landed
four drift gates, and fixed three latent defects found along the way.

No milestone is scoped. The carried list is empty apart from
`hooks_land_on_private_server`, a flake that has not reproduced in 300 runs.
