# DaemonEye: Agentic Operator

DaemonEye is a lightweight background daemon that integrates an AI-powered systems, software, and security expert directly into your tmux session. It understands your full terminal state — scrollback buffer, environment variables, running programs, command history, pane topology — and can act on it. Ghost Shells let it autonomously investigate and remediate incidents while you sleep. Named Agents give those ghost shells persistent identities: a specialist's system prompt, a dedicated model, a scoped memory pool, and a daemon-enforced tool policy that carries across every invocation. Webhooks wire it into Prometheus, Grafana, and any service that can POST JSON.

I wrote DaemonEye after discovering OpenClaw and being completely blown away by its agency and power — and then turning it off because I was afraid of it running amok. DaemonEye is the result of wanting that power with boundaries I trust. It limits command execution to what is explicitly allowed, provides mechanisms to make complex tasks autonomous (even those requiring root access), and tracks exactly what it spends on your behalf.

**Linux only.** DaemonEye uses `fork(2)`, Unix domain sockets, and Linux-specific tmux hooks — it will not build or run on macOS or Windows.

---

## Quickstart

```sh
# 1. Build
git clone <repo> && cd daemoneye
cargo build --release

# 2. Install — creates ~/.daemoneye/, copies the binary, writes a systemd
#    user service, and prints the tmux keybinding to add
./target/release/daemoneye setup

# 3. Add your API key
$EDITOR ~/.daemoneye/etc/config.toml     # set [models.default] api_key

# 4. Start the daemon
daemoneye daemon

# 5. Ask it something
daemoneye ask "why is nginx returning 502?"

# …or open an interactive chat pane inside tmux
daemoneye chat
```

To have the daemon start on login instead of step 4:

```sh
systemctl --user daemon-reload
systemctl --user enable --now daemoneye
```

Add the printed keybinding to `~/.tmux.conf` so you can summon a chat pane with `Ctrl+b T`:

```sh
bind-key T split-window -v '~/.daemoneye/bin/daemoneye chat'
```

Then `tmux source-file ~/.tmux.conf`. The bind-key uses the full path so it works even when `~/.cargo/bin` is not in the `PATH` tmux inherits.

---

## Requirements

| Dependency | Notes |
|---|---|
| Linux | `fork(2)`, Unix domain sockets, Linux-specific tmux hooks |
| Rust 1.79+ | Required by Rust edition 2024 |
| tmux 2.6+ | Required for hook support (`pane-focus-in`, `client-attached`, `after-new-session`) |

```sh
sudo apt install tmux      # Debian/Ubuntu
sudo dnf install tmux      # Fedora
```

To install the binary into `~/.cargo/bin` instead of building in place: `cargo install --path .`

---

## How it works

DaemonEye runs as a background daemon holding a Unix domain socket. The chat client connects to it, and the daemon does the work:

```
you type ──→ chat client ──→ daemon
                               │  captures your pane, masks secrets,
                               │  assembles context, streams from the model
                               ▼
                          model wants to run something
                               │
                               ▼
                    approval prompt in your chat pane
                    [Y]es  [A]pprove for session  [N]o  ·  or type a redirect
                               │
                     approved  ▼
                    command runs — foreground in your pane,
                    or background in a dedicated de-bg-* window
                               │
                               ▼
                    output captured → back to the model → loop
```

Three properties are load-bearing:

- **Nothing runs without a gate.** Every command, script write, runbook write, and file edit goes through an explicit approval prompt. Typing a message instead of `Y`/`A`/`N` redirects the agent mid-stream.
- **Context is masked before it leaves the machine.** A regex filter scrubs credentials, keys, tokens, and connection strings before any provider sees them. See [Security](#security).
- **Privilege escalation needs two keys.** A `sudo` command in an autonomous session requires *both* an `auto_approve_scripts` entry in the runbook *and* a matching NOPASSWD sudoers rule. Either alone is insufficient.

---

## ✨ Key Features

### 👻 Autonomous Ghost Shells

When a critical alert matches a runbook with `enabled: true`, DaemonEye spawns a Ghost Shell — an unattended AI agent that works the problem in dedicated `de-gs-bg-*` tmux windows on the daemon host without a human present. When you next attach, a catch-up brief tells you what happened.

- **Unattended Remediation** — Runs pre-approved remediation scripts, monitors output, and escalates when something unexpected happens.
- **Policy Gating** — Non-sudo commands run freely within your OS permissions; `sudo` is gated to scripts explicitly listed in `auto_approve_scripts` that also have a NOPASSWD sudoers rule installed via `daemoneye install-sudoers`. Two keys are required for every privilege escalation.
- **Turn Budget** — A configurable hard ceiling on AI turns (default 20) ensures the agent cannot loop indefinitely. Individual runbooks may set a lower limit, but never a higher one.
- **Catch-up Brief** — On re-attach, new completions, alerts, and watchdog results are summarised in a `[Catch-up]` message before the first AI token. AI spend during the detach window is included: `Cost during detach: $0.34 (architect $0.20 · ghost-anonymous $0.14)`.

Full walkthrough: [Ghost Shells & Autonomous Remediation](#ghost-shells--autonomous-remediation).

---

### 🤖 Named Agents

Named Agents give ghost shells a persistent executor identity. Where a runbook defines *what* to do, an agent defines *who does it* — the role, the model, the memory namespace, and the tool policy that carry across every separate invocation.

**Define once, reuse everywhere.** An agent is a `~/.daemoneye/agents/<name>/config.toml` file. Any runbook references it with a single `agent:` frontmatter field. Update the agent once and every runbook that uses it benefits immediately.

```toml
# ~/.daemoneye/agents/security-auditor/config.toml
name        = "security-auditor"
description = "Reads IAM policies and audit logs; never writes to production"
prompt      = "You are a cautious security auditor. Analyse before acting. Never modify production systems."
model       = "opus"
auto_approve_read_only = true

[tools]
allow = ["read_file", "read_memory", "list_memories", "search_repository", "run_terminal_command"]
```

Manage them from the CLI with `daemoneye agent list|show|create|delete|briefing`, or from chat with the `create_agent` / `read_agent` / `list_agents` / `delete_agent` tools.

**Persistent briefing state.** After each clean ghost exit, the daemon asks the model to summarise what it found, what it did, and what to watch for next time. This briefing is written to `~/.daemoneye/agents/<name>/briefing.md` (masked before write) and injected as context on the next invocation. A `postgres-dba` agent accumulates six months of port mappings, slow-query patterns, and incident history — and brings all of it to every new alert automatically.

**Memory namespacing.** Each agent writes to and reads from its own scoped memory pool. Security findings don't bleed into the database agent's namespace. Global memories remain accessible to all agents; agent-scoped memories stay private. The main interactive session always reads from global only.

**Daemon-enforced tool policy.** Specify an allowlist or denylist of AI tools — the daemon rejects out-of-scope tool calls at the IPC layer, before they execute. A read-only analyst genuinely cannot call `run_terminal_command` or `edit_file`, regardless of what the model is instructed to do.

**Coordinator pattern.** A coordinator agent can spawn specialist sub-agents in parallel. Each specialist writes its result to a mailbox file; the coordinator synthesises all results and writes a consolidated report. Total elapsed time is the longest specialist, not their sum. Spawn depth is capped at 2 (coordinator + specialists) — the daemon enforces this; the model cannot override it.

```
security-coordinator  (depth 0)
  ├── access-auditor  (depth 1)  — checks IAM policies
  └── log-analyst     (depth 1)  — searches for anomalous patterns
```

**Composability rules** when a runbook specifies an agent:
1. Agent system prompt takes precedence; the runbook task is injected as the first user turn.
2. Runbook-level `model:` beats agent default.
3. Tool policy is the intersection — deny wins; an agent cannot expand `GhostPolicy`.
4. Memory queries search agent namespace → global namespace in that order.

---

### 📡 Webhook Alert Ingestion

Expose an optional HTTP endpoint (default port 9393) to receive alerts from Prometheus Alertmanager, Grafana, or any tool that can POST JSON.

- **Deduplication** — Alerts are fingerprinted; duplicates within a configurable window are suppressed automatically.
- **Sensitive-data masking** — Alert payloads pass through the same redaction filter as terminal context before entering the AI conversation.
- **Watchdog Analysis** — Each incoming alert is automatically analysed against its matching runbook to determine whether autonomous remediation is warranted. Ghost shells are spawned only when the watchdog model emits `GHOST_TRIGGER: YES`.

---

### 🛠️ Collaborative Execution & Safety

The AI doesn't just suggest — it acts. Every proposed action goes through an explicit approval prompt before anything runs.

**Terminal commands** show a three-option prompt, `[Y]es [A]pprove for <label> [N]o` — one consistent format and option order shared across the command, script/runbook, and `edit_file` approval flows:

- **`[Y]es`** — Approve a single execution.
- **`[A]pprove for <label>`** — Trust the AI for this command class for the rest of the session.
- **`[N]o`** — Reject, or type a message to redirect the AI mid-stream.

**Script and runbook writes** show an ANSI diff before asking for approval:

- New files display all lines in green with `+` prefixes so you can read exactly what will be written.
- Modifications display a standard unified diff — red `-` for removed lines, green `+` for added lines, with `@@` hunk headers — so you can see precisely what changed.
- **`[A]pprove for session`** is available here too: once approved, future writes to that specific script or runbook auto-proceed (the diff is still shown, the gate is skipped).

**Script headers:** Scripts carry an inline comment header immediately after the shebang (`# --- daemoneye ---` … `# --- /daemoneye ---`) that holds `tags`, `summary`, and `relates_to` fields. The field names are identical to memory YAML frontmatter, so the same mental model applies to all three artifact types. Tags are indexed by `list_scripts` and `search_repository` without any sidecar files.

**Visual Anchors:** During the command approval window the target pane is highlighted with a dark-blue background (`colour17`) so you always know exactly where a command will land before you commit.

**Pane Targeting:** DaemonEye pins the AI to a specific pane ID on every turn so it never has to guess where to run foreground commands.

- **`[FOREGROUND TARGET]`** — injected into every AI turn, names the exact `%N` pane ID the AI must pass to `run_terminal_command`. When absent (first chat with no sibling panes), the AI asks before acting rather than guessing.
- **`[PANE MAP]`** — also present every turn, mapping window-relative indices to pane IDs (`idx:0=%3* bash | idx:1=%7 vim`) with the active pane marked `*`. Used by the AI to translate user references like "pane 1" into the correct `%N`.
- **`/pane`** — type this at the chat prompt to list available panes and see which one is the current target. Use `/pane %N` to pin a specific pane; the preference is saved to `pane_prefs.json` and survives daemon restarts. The AI is notified of the change immediately.
- **Drift detection** — if the foreground target changes between turns (pane closed, user moved focus), the daemon sends a `[Pane target changed]` system message before the first AI token so the model updates its mental model without manual intervention.
- **Format validation** — if the AI passes a malformed pane ID (e.g. `"1"` instead of `"%7"`), the daemon returns an error with the correct ID so the model can self-correct on the same turn.

**Remote script streaming** — When a foreground command targets a pane that is SSH'd or mosh'd to a remote host and the command names a daemon-host script (one stored in `~/.daemoneye/scripts/`), DaemonEye automatically detects that the bare filename does not exist on the remote and streams the script's content instead: the script is hex-encoded and piped into the remote interpreter's stdin (`python3`/`perl` decode → `bash /dev/stdin <args>`), so the script runs on the remote with no disk write and no remote-side setup. A local pane or a command that is not a managed script is sent verbatim, unchanged. Running a daemon-host script under `sudo` on a remote interactive pane is not supported on this path (a NOPASSWD sudoers rule must authorize a fixed file path, which streaming cannot provide); DaemonEye returns an advisory pointing at the Ghost Shell `ssh_target` mechanism for that case.

**Tool call history** — For silent tools (reads, memory, search, terminal context, etc.) that don't require an approval prompt, DaemonEye renders a compact history line in chat: `▸ tool(args)` when the tool starts, then `⎿ detail · Xs` when it finishes. The spinner shows a live elapsed timer between the two lines so you always know what the AI is doing and how long it has been running. The status bar shows a session-cumulative count (`· tools: N`) so you can see at a glance how many silent tool calls have occurred in the conversation.

**`/approvals`** — type this at the chat prompt to inspect which approvals are currently active across all five scopes: terminal commands (regular and sudo), scripts, runbooks, and file paths. Use `/approvals revoke` to instantly revoke all session approvals, or revoke a single class with `/approvals revoke commands`, `/approvals revoke scripts`, `/approvals revoke runbooks`, or `/approvals revoke files`. The status bar shows a compact count-based summary (e.g. `⚡approvals: all · files: 2 · Ctrl+C revokes`) so you always know what the AI can do without opening `/approvals`. Cumulative write-approval and denial counts for scripts, runbooks, and file edits are tracked by the daemon and shown in `daemoneye status`.

**Configurable defaults** — Add an `[approvals]` section to `~/.daemoneye/etc/config.toml` to seed the initial approval state at the start of every session (both `daemoneye chat` and `daemoneye ask`):

```toml
[approvals]
commands    = true    # non-sudo terminal commands (default: true — the baseline)
sudo        = false   # sudo commands (default: false)
scripts     = false   # all script writes (default: false — per-script [A] still works)
runbooks    = false   # all runbook writes (default: false)
file_edits  = false   # all file edits (default: false)
ghost_commands = false # explicitly tell ghost shells they may run investigation commands freely
```

When a class flag is `true`, approval is pre-granted for the entire class at session start — the `[A]pprove for session` prompt is suppressed since it would be redundant. Ctrl+C or `/approvals revoke` resets all flags back to the config defaults.

---

### 🔍 Full-View tmux Awareness

The agent sees your whole tmux world, not just the pane you are typing in.

- **Every session, not just yours.** The pane cache tracks windows and panes across *all* tmux sessions. Panes outside your own session appear labelled with their session name; their content is fetched on demand rather than polled, so watching a second session costs nothing until you ask.
- **Live status per pane.** Each pane is classified every 2 s — `Running`, `Idle 4m`, `AwaitingInput`, `Bell`, `Dead(1)` — and that status shows up everywhere panes are listed. An idle shell is never mistaken for a prompt waiting on input.
- **Any pane's contents, one call away.** `read_pane` returns a chosen depth of any pane's scrollback, ANSI-annotated and masked. `find_in_panes` regex-searches every pane at once, so "which pane has the error?" is one call instead of a hunt. The chat pane is always refused — its contents are this conversation.
- **The agent can drive tmux, with the gate on.** `tmux_control` focuses, zooms, splits, renames and kills windows — every action behind the same approval prompt as a shell command, because navigation moves your attention too. It refuses to kill a daemon-managed window or the window holding your chat pane, and autonomous Ghost Shells are denied the tool outright unless an agent's `ToolPolicy` names it explicitly.
- **`/panes` is worth reading.** A window-grouped inspector: cwd, status, activity age and a preview line per pane, with the pinned foreground target marked.

---

### 🖥️ Terminal-Native Chat Interface

The chat client is built on a `ratatui` inline viewport that treats your terminal the way a good CLI tool should.

- **Real scrollback** — The conversation transcript is committed to your terminal's native scrollback. Scroll up in tmux copy-mode and the whole history is there, clean and selectable; only the input box and status bar occupy a small fixed region at the bottom.
- **Survives window switches and resizes** — Switching tmux windows away from and back to the chat pane, or resizing it, leaves the transcript and chrome intact. The renderer keeps a history of every committed line; when tmux shifts the grid in ways the client can't observe (screen scrolls, height reflows), the re-anchor clears the stale region and repaints the transcript tail from that history instead of trusting row arithmetic — no orphaned borders, no eaten lines.
- **Multi-line input editor** — Visible cursor, word-wrap, multi-line editing, and multi-line paste. A pasted block lands whole instead of submitting at its first newline.
- **Two-press interrupt** — While the agent is streaming, press ESC or Ctrl+C once to warn, twice to abort the turn.
- **Color-coded panels** — Committed command-output panels use a blood-red border and deep-yellow title so executed actions stand out in the scrollback.

---

### 🧰 On-Demand Tool Loading

DaemonEye's AI tools are split into a **core** set (sent with every request) and **deferred** groups that are omitted by default to keep each request's context small. When the model needs a rarely-used capability it pulls the group in with a single `load_tools` call, and those tool schemas appear on subsequent turns. This is a context-budget optimization and is independent of the `ToolPolicy` / `GhostPolicy` gates, which restrict tools at execution time.

See [AI tools](#ai-tools) for the full inventory.

---

### 🧠 Context Management

Long sessions don't get truncated at an arbitrary message count — DaemonEye manages the context window by **token pressure**, and nothing is ever lost: every message is archived before it can be dropped, and the AI can retrieve any of it on demand.

**Token-pressure ladder.** After each turn the daemon estimates the working set's token usage as a percentage of the active model's context window, calibrating the estimate against the provider's reported usage with an exponential moving average (so a model whose tokenizer runs denser than the estimator is corrected within a couple of turns). Three thresholds, all configurable in `[compaction]`:

| At | What happens |
|---|---|
| `elide_at_pct` (50 %) | Oversized *old* tool results are replaced with `[elided: tool X produced N chars at turn K; archived]` placeholders. Recent turns are untouched. |
| `compact_at_pct` (60 %) | A background task summarises the older turns into an **epoch record** and cuts the working set back to `target_pct` (40 %). |
| `emergency_pct` (85 %) | Synchronous compaction on the interactive path — structured tally only, never a model call, so the turn is never blocked. |

**Compaction runs off the interactive path.** The normal (non-emergency) pass is a `tokio::spawn` that builds the epoch, calls the cheap `[models.digest]` model for a narrative summary, and swaps the compacted history in with a staleness check. Your turn returns at full speed; the context shrinks between turns.

**Epochs, ledger, and chapters.** Each compaction appends an epoch record to `~/.daemoneye/var/log/sessions/<id>.epochs.jsonl` — a turn range, a narrative summary, and a structured tally (commands ok/failed, files edited, alerts, ghosts spawned, tokens). The compacted head is regenerated from the whole chain, so the AI always sees a running `Session ledger:` line plus the most recent epochs. Once uncovered epochs exceed `rollup_after` (10), the oldest five fold into a single **chapter** record, keeping the head from growing linearly with session length.

**Append-only archive + `recall_context`.** Every message is written to `<id>.archive.jsonl` *before* it can be elided or compacted away. The `recall_context` tool retrieves originals by substring query, by turn range, or both — so when the model hits an `[elided: …]` placeholder or an epoch summary that's too coarse, it pulls the real text back rather than guessing. Retrieved excerpts are masked and truncated like any other tool result.

**Sessions survive daemon restarts.** Per-session continuity state (turn counters, token calibration, epoch boundaries) is persisted to `<id>.meta.json`, so a restart mid-conversation resumes with the same compaction ladder rather than starting blind.

**Ghost shells get a synchronous guard.** Autonomous sessions can't wait on a background task, so they run a model-call-free variant: aggressive elision, a structured-only epoch (no narrative), and a working-set cut — all synchronous, no network.

**Optional memory extraction.** Set `extract_memories = true` in `[compaction]` and each epoch build additionally asks the digest model to propose up to three durable facts, written to persistent memory as `knowledge` entries stamped `source: compaction`. Off by default — it costs a small model call per epoch and writes to shared memory.

**Retention.** Session archives are kept forever unless you set `[sessions] archive_retention_days`. The event log rotates into dated segments (`var/log/events/events-YYYYMMDD.jsonl`) with a 90-day default retention.

---

### 📖 Runbooks, Memory & Search

- **Procedure Runbooks** — Store troubleshooting steps in `~/.daemoneye/runbooks/` as Markdown with YAML frontmatter. When an alert fires, DaemonEye finds the matching runbook and uses it to guide the investigation.
- **Durable Memory** — Three-tier persistence for session context (`session`), knowledge facts (`knowledge`), and incident records (`incidents`). Session memories are injected into every AI turn automatically; knowledge and incident memories are available on demand. Entries carry structured frontmatter — `tags` (with synonyms for broader matching), `summary` (one-liner surfaced in listings), `relates_to` (links to related memories, runbooks, or scripts), and `expires` (TTL for time-bounded facts). Use `update_memory` to change individual fields in place without a full rewrite.
- **Full-text memory search** — Memory is indexed in a SQLite **FTS5** database at `~/.daemoneye/var/index/memory.db`, maintained best-effort on every add, update, and delete. Recall merges three candidate sources: tag overlap, one-hop `relates_to` expansion, and **BM25-ranked** full-text hits against your turn. The index is namespaced, so agent-scoped memories stay separate from global ones. It rebuilds automatically whenever it is found empty — and when it is populated but stale, `daemoneye reindex` forces a rebuild (single transaction, safe to run while the daemon is up).
- **Built-in Guides** — Seven knowledge memory files are seeded on first run — `agent-runtime-layout`, `ghost-shell-guide`, `runbook-format`, `runbook-ghost-template`, `scheduling-guide`, `scripts-and-sudoers`, and `webhook-setup` — so the AI can reference them without any manual setup.
- **Named Sessions** — Save and resume conversation history with `/session save <name>`. Artifacts (runbooks, scripts, memories) created during a named session are tagged with `session_origin` so you can trace which session produced them.

---

### 🐕 Command Scheduler & Watchdog

- **Flexible Scheduling** — Run commands or Ghost Shell tasks once at a specific time, on a repeating interval, or on a full cron expression.
- **Watchdog Monitors** — Active monitors use AI-powered analysis to evaluate system state on a schedule and trigger remediation when something looks wrong.
- **Failure Isolation** — Each job runs in its own dedicated tmux window (`de-sj-*`), left open on failure for manual inspection and cleaned up automatically on success.
- **Bell Recovery** — Every 2-second poll checks `#{window_flags}` for uncleared bells (`!`) and unseen activity (`#`) on all windows. Newly-discovered bells are logged to `events.jsonl` so notifications missed during a daemon restart are recovered automatically.

---

### 💰 Cost Accounting

DaemonEye tracks the dollar cost of every AI call — interactive sessions, ghost shells, named agents, scheduled jobs — and surfaces it where you need it.

- **Status bar**: every chat turn shows a live session cost (`· $0.08`) next to the model name. A `+` suffix (`$0.08+`) means at least one call used a model with unknown pricing. Cost is omitted when zero.
- **`daemoneye status`**: daemon-wide "Cost (today)" section shows total spend broken down by provider and by agent. Hidden when total is $0.
- **`daemoneye costs`**: reads `events.jsonl` directly (no daemon required, works when the daemon is down). Supports `--since`, `--until`, `--by day|agent|provider|model|session`, `--agent <name>`, `--json` for integration with external tooling.
- **Catch-up brief**: AI spend during a detach window is included alongside event summaries. If only local providers ran: `$0.00 (local providers only)`. If no AI calls occurred, the cost line is omitted entirely.
- **Local models always $0.00** — `ollama` and `lmstudio` providers are priced at zero; costs are never guessed for unknown models (flagged with `+` instead).

Built-in rates cover Anthropic (Sonnet, Opus, Haiku), OpenAI (GPT-4o, o1, o3-mini), and Gemini (2.5 Pro, 2.5 Flash). Override or extend with per-model cost fields in `[models.<name>]` config blocks.

---

## Command reference

### CLI subcommands

| Command | Description |
|---|---|
| `daemoneye daemon` | Start the background daemon |
| `daemoneye daemon --console` | Log to the console instead of a file (troubleshooting; required for `Type=simple` systemd) |
| `daemoneye daemon --log-file FILE` | Write the daemon log to `FILE` instead of `~/.daemoneye/var/log/daemon.log` |
| `daemoneye daemon --session NAME` | Override the managed tmux session name from config |
| `daemoneye stop` | Stop the daemon gracefully |
| `daemoneye ping` | Check whether the daemon is running |
| `daemoneye status` | Daemon status: uptime, sessions, ghost shells, cost today, redactions, circuit state |
| `daemoneye logs` | Tail `daemon.log` |
| `daemoneye chat` | Start an interactive multi-turn chat session |
| `daemoneye chat --session NAME` | Open a chat window in a specific tmux session and attach to it |
| `daemoneye ask <query>` | Send a single question to the AI |
| `daemoneye setup` | Initialise `~/.daemoneye/`, install the binary, write the systemd service, print tmux config |
| `daemoneye setup --overwrite-bin` | Re-copy the current binary to `~/.daemoneye/bin/daemoneye` |
| `daemoneye setup --overwrite-memory` | Refresh the built-in knowledge memory files from the current binary |
| `daemoneye setup --overwrite-all` | Refresh binary, memories, and the built-in SRE prompt (your `config.toml` is never touched) |
| `daemoneye prompts` | List available prompts in `~/.daemoneye/etc/prompts/` |
| `daemoneye scripts` | List scripts in `~/.daemoneye/scripts/` |
| `daemoneye agent list` | List all named agents |
| `daemoneye agent show <name>` | Show the full config for a named agent |
| `daemoneye agent create <name>` | Create an agent (opens `$EDITOR` with a starter config) |
| `daemoneye agent delete <name>` | Delete a named agent |
| `daemoneye agent briefing <name>` | Show or clear an agent's rolling briefing |
| `daemoneye schedule list` | List scheduled jobs and their status |
| `daemoneye schedule cancel <id>` | Cancel a scheduled job by UUID |
| `daemoneye schedule delete <id>` | Permanently delete a scheduled job by UUID |
| `daemoneye schedule windows` | List leftover `de-*` tmux windows from failed scheduled jobs |
| `daemoneye session import <id> --name <name>` | Import an orphaned ephemeral session log into the named session store (no daemon required) |
| `daemoneye costs` | Show AI spend from the event log (no daemon required) |
| `daemoneye costs --since DATE --until DATE --by agent` | Filter and group the cost report; `--agent NAME`, `--json` also available |
| `daemoneye install-sudoers <script>` | Write a NOPASSWD sudoers drop-in for `~/.daemoneye/scripts/<script>` |
| `daemoneye reindex` | Rebuild the memory search index from the memory files on disk; reports rows before and after |
| `daemoneye audit-prompts` | Audit installed prompt and knowledge memory files for stale path references; exits non-zero on findings, never writes |
| `daemoneye notify` | Internal — out-of-band notifications from tmux hooks; not intended for direct use |

Both `reindex` and `audit-prompts` run without a daemon. `reindex` is safe to run while one is up: the rebuild is a single transaction, so a concurrent search sees the old index or the new one, never a half-empty one.

### In-chat slash commands

| Command | Description |
|---|---|
| `/help` | Show the in-app command list (aliases: `help`, `?`, `/?`) |
| `/exit` | Quit the chat session (alias: `/quit`) |
| `/clear` | Reset the session (alias: `/new`) |
| `/refresh` | Resync host context |
| `/model` | List or switch the active model (alias: `/models`) |
| `/prompt` | List or switch the system prompt |
| `/pane` | Window-grouped pane inspector — cwd, status, activity age and a preview line per pane; `/pane %N` or `/pane <n>` pins the foreground target (alias: `/panes`) |
| `/approvals` | Inspect approval state; `on`/`off`/`revoke [class]` (alias: `/approval`) |
| `/limits` | Show active limits and live session counters; `/limits reset` |
| `/session` | `save`/`load`/`list`/`delete`/`rename`/`diff`/`tag` (alias: `/sessions`) |

At a tool-approval prompt, typing a message instead of `Y`/`A`/`N` redirects the agent. Up/Down navigate the input; at the top or bottom edge they recall history.

---

## AI tools

The model has **36 tools**. The 27 **core** tools are sent with every request; the 9 **deferred** tools are omitted by default and pulled in on demand with a single `load_tools` call — a context-budget optimisation independent of the policy gates that restrict tools at execution time.

Tools marked **⚠** require explicit user approval before they execute.

### Core

| Tool | What it does |
|---|---|
| `run_terminal_command` **⚠** | Run a bash command — foreground in your pane, or background in a dedicated tmux window |
| `edit_file` **⚠** | Create, edit, delete, or copy a file; shows a coloured unified diff before approval |
| `write_script` **⚠** | Create or update a script in `~/.daemoneye/scripts/` (written `chmod 700`) |
| `delete_script` **⚠** | Delete a script |
| `write_runbook` **⚠** | Create or update a runbook in `~/.daemoneye/runbooks/` |
| `delete_runbook` **⚠** | Delete a runbook |
| `tmux_control` **⚠** | Act on your tmux: `focus` a pane, `zoom`/`unzoom`, `split`, `rename_window`, `kill_window`. Refuses to kill daemon-managed windows or the chat pane's window |
| `read_file` | Paginated file read with optional grep filter; masks sensitive data |
| `search_repository` | Search runbooks, scripts, memory, or the event log |
| `get_terminal_context` | Capture a fresh tmux snapshot on demand; `scope` selects the window, the session (default), or all sessions |
| `list_panes` | Enumerate panes grouped by window, with ID, index, command, cwd, title and live status — plus a labelled section for panes in other tmux sessions |
| `read_pane` | Read any pane's buffer on demand at a requested scrollback depth, including other sessions and daemon background windows; ANSI-annotated, optionally regex-filtered, masked |
| `find_in_panes` | One regex search across every pane's buffer — answers "which pane has the error?" in a single call |
| `watch_pane` | Block until a pane matches a regex, the command exits, or a timeout elapses |
| `close_background_window` | Close a background tmux window that is no longer needed |
| `add_memory` | Store a persistent memory entry |
| `read_memory` | Read a memory entry by key and category |
| `update_memory` | Update individual fields of a memory entry in place |
| `list_memories` | List memory keys, optionally filtered by category |
| `recall_context` | Retrieve archived turns from this session by query, turn range, or both |
| `schedule_command` **⚠** | Schedule a one-shot, interval, or cron job — command, script, or ghost shell |
| `list_schedules` | List scheduled jobs with status and next fire time |
| `cancel_schedule` | Cancel a scheduled job |
| `delete_schedule` | Permanently delete a scheduled job |
| `spawn_ghost_shell` | Delegate a task to an autonomous background Ghost Shell |
| `await_agent_result` | Wait for a spawned agent ghost shell and return its result |
| `load_tools` | Pull a deferred tool group into the active tool set |

### Deferred

| Group | Tools |
|---|---|
| `agents` | `create_agent` **⚠**, `read_agent`, `list_agents`, `delete_agent` **⚠** |
| `runbooks` | `read_runbook`, `list_runbooks` |
| `scripts` | `read_script`, `list_scripts` |
| `memory` | `delete_memory` |

---

## Installation details

### `daemoneye setup`

Run it once after building. It initialises the full `~/.daemoneye/` tree, copies the binary to a stable location, writes a systemd user service file, and prints the tmux keybinding.

| Flag | Effect |
|---|---|
| `--overwrite-bin` | Copy the current binary to `~/.daemoneye/bin/daemoneye`, replacing the installed copy. |
| `--overwrite-memory` | Overwrite the built-in knowledge memory files in `~/.daemoneye/memory/knowledge/` with the versions bundled in the new binary. User-created memories are not affected. |
| `--overwrite-all` | Combines both, and also refreshes `~/.daemoneye/etc/prompts/sre.toml`. Your `config.toml` is never overwritten. |

On first run all seeded files (binary, memories, prompt) are written automatically regardless of flags. Directories and files that already exist are never overwritten, so re-running `setup` after an upgrade is safe.

### Directory layout

`~/.daemoneye/` is the shared root for both the daemon process and the AI agent. Everything — configuration, scripts, runbooks, memory, agent profiles, logs — lives in one place. `setup` creates the core tree; paths marked *(on first use)* appear the first time something needs them:

```
~/.daemoneye/
  bin/
    daemoneye             ← copy of the running binary; the service file and bind-key point here
  etc/
    config.toml           ← main configuration (created once; your edits are preserved)
    prompts/
      sre.toml            ← built-in SRE system prompt (recreated only if missing)
  lib/                    ← shared SDK modules or Python helpers (on first use)
  agents/                 ← named agent profiles
    <name>/
      config.toml         ← AgentConfig: prompt, model, tool policy, memory namespace
      briefing.md         ← rolling summary of last invocation (generated by daemon, masked)
      mailbox/            ← agent-to-agent delegation results (<job_id>.json)
  memory/
    knowledge/            ← seven built-in guides, seeded once:
      agent-runtime-layout.md      ← agent runtime directory layout
      ghost-shell-guide.md         ← guide to ghost shell usage
      runbook-format.md            ← runbook markdown format reference
      runbook-ghost-template.md    ← ghost-enabled runbook template
      scheduling-guide.md          ← scheduler usage guide
      scripts-and-sudoers.md       ← scripts and sudoers setup guide
      webhook-setup.md             ← webhook integration guide
    session/              ← session-context memories (injected into every turn)
    incidents/            ← incident records (on first use)
  runbooks/               ← your procedure runbooks (Markdown + frontmatter)
  scripts/                ← your automation scripts (set chmod 700 on write)
  var/
    log/
      daemon.log          ← daemon process log (tailed by `daemoneye logs`)
      events.jsonl        ← legacy structured event log (read for history; never rotated or deleted)
      events/             ← (on first use)
        events-YYYYMMDD.jsonl  ← dated event segments (command history, AI turns, costs, lifecycle)
      sessions/
        <id>.jsonl        ← live working-set history for an ephemeral session
        <id>.archive.jsonl ← append-only archive of every message (source for recall_context)
        <id>.epochs.jsonl ← epoch + chapter records (compaction summaries and tallies)
        <id>.meta.json    ← session continuity state (turn counters, token calibration)
      panes/              ← archived background-command output (one .log per job window)
      pipes/              ← pipe-pane capture logs (raw terminal output, runtime only)
    run/
      daemoneye.sock      ← Unix domain socket (created when the daemon starts)
      pane_prefs.json     ← per-session target-pane preferences
      schedules.json      ← scheduled job store
    sessions/             ← named session store, <name>/meta.toml + messages.jsonl (on first use)
    index/
      memory.db           ← FTS5 full-text search index, namespaced (built on first index write)
```

### systemd user service

`daemoneye setup` writes `~/.config/systemd/user/daemoneye.service` — a user-scoped service that runs `~/.daemoneye/bin/daemoneye daemon --console` automatically on login. The `--console` flag is required for `Type=simple` systemd services: without it the daemon forks, the parent exits, and systemd loses track of the process.

When running as a systemd service the daemon starts outside tmux and automatically creates (and owns) a tmux session named `"daemoneye"` — ghost shells, scheduled jobs, and webhook-triggered automation are available immediately with no interactive client connection required.

To use a different session name, add a `[daemon]` section to `~/.daemoneye/etc/config.toml`:

```toml
[daemon]
tmux_session = "myserver"   # override the default "daemoneye" session name
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now daemoneye     # enable and start on login
systemctl --user status daemoneye           # check status
systemctl --user restart daemoneye          # restart after a config change
systemctl --user stop daemoneye
systemctl --user disable daemoneye          # disable autostart
```

View logs with `daemoneye logs` (tails `~/.daemoneye/var/log/daemon.log`) or through journald: `journalctl --user -u daemoneye -f`.

### Shell hook (optional)

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

# [models.local]
# provider  = "ollama"
# model     = "llama3.2"
# base_url  = "http://localhost:11434/v1"
# context_window_tokens = 8192

# [models.gpt]
# provider = "openai"
# model    = "gpt-4o"

# [masking]
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]

# [ghost]
# max_ghost_turns = 20   # hard ceiling; individual runbooks may set lower
# max_concurrent_ghosts = 3  # 0 = unlimited

# [limits]
# per_tool_batch            = 100    # max consecutive calls of one non-approval tool per turn (0 = unlimited)
# total_tool_calls_per_turn = 0      # max total non-approval tool calls per turn (0 = unlimited)
# tool_result_chars         = 16000  # max chars fed back to the AI per tool result (0 = unlimited)
# max_turns                 = 0      # max AI turns per chat session (0 = unlimited; ghosts use max_ghost_turns)
# max_tool_calls_per_session = 0     # cumulative non-approval tool calls per session (0 = unlimited)

# [limits.per_tool]
# read_file         = 200   # override per_tool_batch for this tool only (0 = unlimited for this tool)
# search_repository = 50

# [compaction]
# elide_at_pct     = 50     # elide oversized old tool results at this % of the context window
# compact_at_pct   = 60     # build an epoch and cut the working set at this %
# target_pct       = 40     # post-compaction working-set target
# emergency_pct    = 85     # synchronous, model-call-free backstop
# rollup_after     = 10     # fold the oldest 5 epochs into a chapter past this many uncovered
# extract_memories = false  # opt-in: extract durable facts to memory on each epoch build

# [digest]
# narrative_enabled = true  # spend a [models.digest] call on a natural-language epoch summary

# [events]
# retention_days = 90       # delete dated event segments older than this (0 = keep forever)

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
| `input_cost_per_mtok` | float | *(built-in default)* | Override input cost in USD per million tokens. |
| `output_cost_per_mtok` | float | *(built-in default)* | Override output cost in USD per million tokens. |
| `cache_read_cost_per_mtok` | float | *(built-in default)* | Override cache-read cost in USD per million tokens. |
| `cache_write_cost_per_mtok` | float | *(built-in default)* | Override cache-write cost in USD per million tokens. |

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
| `max_ghost_turns` | integer | `20` | Hard upper limit on AI turns per ghost shell. A runbook's `max_ghost_turns` is clamped to this value. |
| `max_concurrent_ghosts` | integer | `3` | Maximum ghost shells running simultaneously. Set to `0` to disable the cap. |

### `[limits]` section

Controls how many tool calls the AI can make per turn and per session, and how much output is fed back. All values default to the legacy hardcoded constants so existing configs are unaffected. Set any value to `0` to remove that limit entirely.

| Key | Type | Default | Description |
|---|---|---|---|
| `per_tool_batch` | integer | `100` | Maximum consecutive calls of a single non-approval tool within one AI turn. Approval-gated tools are always exempt. |
| `total_tool_calls_per_turn` | integer | `0` | Hard cap on all non-approval tool calls within one turn. `0` = unlimited. |
| `tool_result_chars` | integer | `16000` | Maximum characters of output fed back to the AI per tool result. Longer results are truncated. `0` = unlimited. |
| `max_turns` | integer | `0` | Maximum AI turns per interactive chat session. Ghost shells use `ghost.max_ghost_turns` instead. `0` = unlimited. |
| `max_tool_calls_per_session` | integer | `0` | Cumulative cap on non-approval tool calls across the entire session. `0` = unlimited. Reset with `/limits reset` in chat. |

Per-tool overrides via `[limits.per_tool]` let you raise or lower `per_tool_batch` for specific tools:

```toml
[limits.per_tool]
read_file         = 200   # allow more read_file calls than the global batch cap
search_repository = 25    # tighten search calls
```

Use `/limits` in the chat pane to inspect the active values and live session counters. Use `/limits reset` to zero the per-session tool counter without ending the session.

There is deliberately **no message-count cap** on session history. History length is governed entirely by token pressure — see `[compaction]` below.

### `[compaction]` section

Controls token-pressure-driven context management. All values are percentages of the active model's context window. See [Context Management](#-context-management) for how the ladder works.

| Key | Type | Default | Description |
|---|---|---|---|
| `elide_at_pct` | integer | `50` | Replace oversized *old* tool results with `[elided: …]` placeholders at this pressure. The full text stays in the session archive. |
| `compact_at_pct` | integer | `60` | Build an epoch record and cut the working set. Runs in the background, off the interactive path. |
| `target_pct` | integer | `40` | Post-compaction working-set target. Must be below `compact_at_pct` or hysteresis is lost — the daemon logs a `[compaction]` warning and falls back to a safe default if it isn't. |
| `emergency_pct` | integer | `85` | Synchronous compaction threshold. This path never makes a model call, so an interactive turn is never blocked on one. |
| `rollup_after` | integer | `10` | When uncovered epochs exceed this count, the oldest five are folded into one chapter record. |
| `extract_memories` | bool | `false` | Opt-in: ask the digest model to propose 0–3 durable facts per epoch and write them to persistent memory (category `knowledge`, `source: compaction`). |

### `[digest]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `narrative_enabled` | bool | `true` | Spend a `[models.digest]` call (falling back to `[models.default]`) on a natural-language summary of the compacted turns. Set `false` to keep only the structured tally. The emergency path ignores this and never calls a model. |

Define a cheap model for this work with a `[models.digest]` block — it is also used for session auto-naming:

```toml
[models.digest]
provider = "anthropic"
model    = "claude-haiku-4-5-20251001"
```

### `[events]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `retention_days` | integer | `90` | Delete dated event segments (`var/log/events/events-YYYYMMDD.jsonl`) older than this many days. `0` = keep forever. The legacy `var/log/events.jsonl` is never deleted. |

### `[daemon]` section

Controls daemon startup and session ownership. Use this when running DaemonEye as a systemd user service so ghost shells, scheduled jobs, and webhook-triggered automation work without any `daemoneye chat` client connected.

| Key | Type | Default | Description |
|---|---|---|---|
| `tmux_session` | string | `""` | Name of the tmux session the daemon creates (or adopts, if it already exists) at startup. Empty = legacy behaviour: the daemon borrows whatever session the first `daemoneye chat` client connects from. |
| `auto_create_session` | bool | `true` | Create the session with `tmux new-session -d` if it does not already exist. Only applies when `tmux_session` is set. If the session is killed, the daemon recreates it automatically. |

When `tmux_session` is set, `daemoneye chat` invoked **outside** of tmux will open a new chat window inside the managed session and exec-attach to it, dropping the user straight into the right place.

### `[sessions]` section

Controls named session persistence — saving and resuming conversation history across daemon restarts.

| Key | Type | Default | Description |
|---|---|---|---|
| `auto_name_enabled` | bool | `true` | After `auto_name_turn_threshold` turns, prompt the user with an AI-suggested session name in the chat pane. |
| `auto_name_turn_threshold` | integer | `10` | Number of user turns before an auto-name suggestion is offered. |
| `load_recent_turns` | integer | `10` | Number of most-recent turns loaded when resuming a saved session with `/session load`. `0` loads the complete history (may exceed the context window). |
| `archive_retention_days` | integer | `0` | Delete session archive files (`<id>.archive.jsonl`) whose mtime is older than this many days. `0` = keep forever. Archives belonging to active sessions are never swept. |

**In-chat session commands:**

| Command | Description |
|---|---|
| `/session save [name]` | Save the current conversation under `name` (prompts if omitted) |
| `/session tag [name]` | Alias for `/session save` |
| `/session load <name>` | Resume a previously saved session (replaces current history) |
| `/session list` | List all saved sessions with turn counts and descriptions |
| `/session rename <old> <new>` | Rename a saved session |
| `/session delete <name>` | Delete a saved session |
| `/session diff [name]` | Show a summary of what changed since the session was last saved |

Artifacts (runbooks, scripts, memories) created during a named session are tagged with `session_origin: "<name>"` in their frontmatter, so you can trace which session produced them. On first save, any artifacts created before the session was named are retroactively tagged.

### Environment variables

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | API key for the `anthropic` provider (used if `api_key` is not set in config). |
| `OPENAI_API_KEY` | API key for the `openai` provider (used if `api_key` is not set in config). |
| `GEMINI_API_KEY` | API key for the `gemini` provider (used if `api_key` is not set in config). |
| `OPENAI_API_BASE` | Override the base URL for the `openai` provider (fallback; prefer `base_url` in config). |

---

## Ghost Shells & Autonomous Remediation

Ghost Shells are unattended AI agents that DaemonEye can spawn automatically in response to incoming webhook alerts. When triggered, a ghost shell investigates the alert and executes pre-approved remediation steps — all without a human present. The ghost itself runs headless inside the daemon; each command it executes gets its own tmux window on the daemon host, created lazily and prefixed `de-gs-bg-*` (webhook- and interactively-triggered ghosts) or `de-gs-sj-*` (scheduler-triggered). Start, completion, and failure events appear in the next catch-up brief when you re-attach.

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
  • Ensures the host tmux session exists (command windows created lazily)
  • Loads ghost_config from runbook frontmatter
  • Applies named agent profile if runbook specifies agent: <name>
  • Injects prior briefing state as [Previous Session Summary] context
  • Injects [Ghost Shell Started] into all active chat sessions
        │
        ▼
Ghost AI turn loop (up to max_ghost_turns)
  • Reads runbook + alert context as system prompt
  • Issues run_terminal_command (background mode only)
  • Policy gate: non-sudo commands always allowed (OS permissions are the boundary);
    sudo commands must be in auto_approve_scripts + have a NOPASSWD sudoers rule;
    named agent tool policy enforced independently at IPC layer
  • Script dispatch: without ssh_target, bare/relative names resolve to
    ~/.daemoneye/scripts/<name> (+ sudo prefix if run_with_sudo: true);
    with ssh_target, scripts are hex-streamed to the remote interpreter's
    stdin by default (no remote disk write) — sudo cases only materialize
    the script to the sudoers-authorized path on the remote host
  • watch_pane blocks until command exits before next turn
        │
        ▼
On clean exit: generate_and_save_briefing() writes masked summary to
  ~/.daemoneye/agents/<name>/briefing.md (if agent-scoped)
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
# Use daemoneye install-sudoers (recommended — pins the exact path automatically):
daemoneye install-sudoers restart-nginx.sh

# Or manually with visudo:
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
| `agent` | string | *(none)* | Named agent profile to use. The agent's prompt, model, tool policy, and memory namespace are applied at spawn time. |
| `auto_approve_scripts` | list | `[]` | Script names in `~/.daemoneye/scripts/` pre-approved for **sudo** execution. Non-sudo commands run freely without listing them. Bare names, relative paths (`./name.sh`), and commands with arguments are all resolved to the absolute path. |
| `run_with_sudo` | bool | `false` | Auto-prepend `sudo` when executing scripts listed in `auto_approve_scripts`. The ghost AI can then write `script.sh` instead of `sudo script.sh`. Does **not** grant permission to run arbitrary sudo commands — the `auto_approve_scripts` whitelist is always enforced. Each approved script still requires a NOPASSWD sudoers rule via `daemoneye install-sudoers`. |
| `max_ghost_turns` | integer | `0` | Per-runbook turn cap. Clamped to the daemon ceiling (`ghost.max_ghost_turns` in `config.toml`). `0` means use the daemon ceiling. |
| `model` | string | *(agent or default)* | Model key for this ghost shell. Beats the agent-level model if both are set. |
| `ssh_target` | string | *(none)* | SSH destination (e.g. `user@host` or `host`) for remote execution. When set, all commands are transparently wrapped in `ssh <target> <cmd>` before execution. Daemon-host scripts are **streamed** to the remote via hex-decode → interpreter stdin by default (no remote disk write); when `sudo` is required the script is materialized to its `~/.daemoneye/scripts/<name>` path on the remote (a NOPASSWD sudoers rule must cover that path). The AI is instructed not to SSH manually — omit this field for local-only execution. |
| `auto_approve_commands` | bool | `false` | Explicitly tell the ghost shell it may run non-sudo investigation commands freely. Non-sudo commands are always permitted by OS permissions regardless of this flag; setting it to `true` makes that explicit in the system prompt so the model does not withhold useful investigation commands. Can also be enabled daemon-wide via `[approvals] ghost_commands = true` in `config.toml`; the two sources are OR-ed together. |

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
# In another pane — list the ghost's command windows and select one
tmux list-windows -a | grep de-gs-
tmux select-window -t <window-name>

# Or just watch daemon.log
daemoneye logs
```

Check the event log for the full audit trail:

```bash
grep "ghost\|webhook_analysis\|command_approval\|ai_cost" ~/.daemoneye/var/log/events.jsonl | tail -30
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

Cost (today)
  Total:     $0.47
  By agent:  ghost-anonymous $0.28 · nginx-responder $0.19
  By provider: anthropic $0.47
```

List the ghost command windows currently open:

```bash
tmux list-windows -a | grep de-gs-
```

### Security considerations

- **Non-sudo commands run as you.** The ghost runs as the same OS user as the daemon. Any command that doesn't require `sudo` runs within your existing file permissions — no additional policy needed.
- **Sudo requires two explicit approvals.** To allow a sudo command: (1) list the script in `auto_approve_scripts`, and (2) run `daemoneye install-sudoers <script>` to create the NOPASSWD sudoers rule. Both must be present. Any other sudo command is automatically denied.
- **Named agent tool policy is a second gate.** If a runbook specifies an agent, both `GhostPolicy` (sudo gating) and `ToolPolicy` (tool allowlist/denylist) must pass independently. An agent cannot expand the tool set beyond `GhostPolicy` allows.
- **Scope sudoers entries tightly.** `daemoneye install-sudoers` pins the exact absolute path in `/etc/sudoers.d/`. Never manually add `ALL` as the command or allow path wildcards.
- **Only list scripts you control.** `auto_approve_scripts` matches filenames in `~/.daemoneye/scripts/`. Scripts outside that directory are never auto-approved regardless of path.
- **`enabled: true` is opt-in per runbook.** Alerts without a matching runbook, or runbooks without `enabled: true`, never trigger a ghost shell.
- **Turn budget limits blast radius.** The daemon enforces a hard ceiling via `ghost.max_ghost_turns` in `config.toml` (default 20). Individual runbooks may set a *lower* limit with `max_ghost_turns` in their frontmatter, but can never exceed the daemon ceiling.
- **Coordinator depth capped at 2.** A coordinator ghost can spawn specialist sub-agents; specialists cannot spawn further agents. The daemon enforces this at the IPC layer.
- **All actions are logged.** Every command approval, execution, result, and AI cost is recorded in `events.jsonl` for post-incident audit.

---

## Security

### Trust model & full hardening details

[`docs/security.md`](docs/security.md) is the canonical reference for the
threat model and every guard: SO_PEERCRED IPC authentication (only your own
processes can drive the daemon), the `~/.daemoneye/` filesystem lockdown
(`0700` dirs / `0600` files, covering API keys, transcripts, and logs),
webhook fail-closed startup, per-IP webhook rate limiting, strict
`GHOST_TRIGGER: YES` parsing, temp-file hardening, and shell-string hygiene.
It also spells out which protections are *not* boundaries — the approval gate,
path guards, and masking all assume your own account is trusted.

### Sensitive-data redaction

Before sending terminal context to an AI provider, DaemonEye applies a regex-based filter that masks:

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

Add organisation-specific patterns to `extra_patterns` in `[masking]`. Built-in patterns always run — user patterns extend the set, never replace it. Redaction counts by type are tracked across the daemon's lifetime and displayed under **Redactions** in `daemoneye status`, giving a continuous audit view of what categories have been filtered. All built-in types are always shown (including those with a zero count), and hits from user-configured `extra_patterns` are tallied separately as `"User Defined"`.

The same filter runs on webhook alert payloads before they enter the conversation, and on ghost briefings before `briefing.md` is written to disk — so a model cannot launder a secret through a briefing file.

### Sudo passwords

When a command (foreground or background) requires `sudo`, the daemon first checks whether credentials are already cached (`sudo -n true`). If cached, the command runs without any interruption. If not cached, the chat interface prompts for your password with terminal echo disabled — you always type it in the chat pane, not in the terminal pane, eliminating the risk of keystrokes landing in the wrong window.

Up to 3 attempts are permitted. A wrong password is detected from the pane output ("Sorry, try again.") and you are re-prompted automatically. If all attempts fail or you cancel, the AI receives a structured error describing what happened and suggesting `daemoneye install-sudoers` where appropriate.

The password is never written to disk, stored in a log file, or transmitted to the AI. The in-memory credential is held in a `zeroize::Zeroizing<String>` that overwrites the allocation on drop.

### `sudoers.d` integration

`daemoneye install-sudoers <script>` writes a NOPASSWD drop-in to `/etc/sudoers.d/daemoneye-<name>` that pins the exact absolute path of the approved script — no wildcards, no `ALL`. Privilege escalation requires both an `auto_approve_scripts` entry in the runbook and a matching sudoers rule; either alone is insufficient.

### Agent config protection

Agent configs cannot be written by AI tools without user approval (the same gate as `edit_file`). An agent cannot modify its own config or another agent's config.

---

## Command audit log

Every command the AI proposes — whether approved, denied, or timed out — is recorded as a JSON object in a dated segment under `~/.daemoneye/var/log/events/` (`events-YYYYMMDD.jsonl`, UTC). AI cost records are written to the same segment after each completed turn:

```
[1748000000] session=abc123 mode=background pane=- status=approved cmd=ps aux --sort=-%mem out=USER PID ...
[1748000001] session=abc123 mode=foreground pane=%3 status=denied cmd=sudo rm -rf /tmp/old out=
{"event":"ai_cost","ts":"2026-05-16T10:23:01Z","agent_name":"chat","provider":"anthropic","cost":{"total_cost_usd":0.0847},...}
```

Fields for command records: Unix timestamp · session ID · `background` or `foreground` · tmux pane ID · `approved` / `denied` / `timeout` / `send-failed` · command · first 200 chars of output.

Segments older than `[events] retention_days` (default 90) are swept automatically by the daemon. A pre-rotation `var/log/events.jsonl` is still read by every consumer (`daemoneye costs`, `daemoneye status`, `search_repository`, epoch tallies) and is never rotated or deleted — date-ranged reads span the legacy file and the segments transparently.

---

## Architecture & contributing

The design documentation lives alongside the code and is kept current by tests:

| Document | Contents |
|---|---|
| [`docs/architecture.md`](docs/architecture.md) | System layers, major data flows, non-goals, milestone roadmap |
| [`CLAUDE.md`](CLAUDE.md) | Module-by-module map, key invariants, the add-a-tool checklist |
| [`docs/dev/STANDARDS.md`](docs/dev/STANDARDS.md) | Engineering Definition of Done |
| [`docs/dev/WORKFLOW.md`](docs/dev/WORKFLOW.md) | Phase lifecycle and review process |

A `tests/doc_truth.rs` tripwire guards these documents (and this README) against reintroducing claims that have stopped being true, and against silently dropping ones that must stay.

```sh
cargo build
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo fmt --all --check
```

---

## License

MIT License

Copyright (c) 2026 Matt Ryanczak
