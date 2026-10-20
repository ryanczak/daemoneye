# DaemonEye: Agentic Operator

DaemonEye is a Linux daemon that puts an AI systems, software, and security operator beside your terminal. Ask it something from any terminal and it investigates the way you would: it runs commands on this host, or on your configured remote hosts over SSH, in shells the daemon owns. Every byte of those shells is recorded, and every action waits at an approval gate you control. When an alert fires at 3 a.m. it can work the problem on its own. **Ghost Shells** run unattended remediation under a policy you wrote. **Named Agents** give them persistent specialist identities, each with its own model, memory and tool policy. A **coordinator agent** fans the work out to specialists in parallel. **Docker Sandboxes** microVMs keep agent work off your host. Webhooks wire it into Prometheus, Grafana, and any service that can POST JSON.

I wrote DaemonEye after discovering OpenClaw and being completely blown away by its agency and power — and then turning it off because I was afraid of it running amok. DaemonEye is the result of wanting that power with boundaries I trust. It limits command execution to what is explicitly allowed, provides mechanisms to make complex tasks autonomous (even those requiring root access), and tracks exactly what it spends on your behalf.

**Linux only.** DaemonEye uses `fork(2)`, PTYs and Unix domain sockets. It will not build or run on macOS or Windows. It needs no tmux: it works from any terminal (see [Requirements](#requirements)).

---

## What's new in 2.0

DaemonEye 1.x drove your tmux panes. 2.0 runs its own shells, reaches remote hosts directly, and can isolate agents in microVMs. Design of record: [`docs/design/daemoneye-2.0.md`](docs/design/daemoneye-2.0.md).

- **Any terminal, no tmux.** `daemoneye chat` runs in any terminal, and nothing the daemon runs goes through tmux. Inside tmux the agent can still *read* your pane summaries for context. It never types into, creates, or changes a pane. See [Inside tmux (optional)](#-inside-tmux-optional).
- **Daemon-owned PTY shells.** Commands run in PTY shells hosted by detached `shell-host` processes, so they survive a daemon restart. A marker protocol reports each command's real exit code. Every shell is recorded byte for byte as an asciicast v2 log that you or the agent can replay later. See [PTY Shells & Remote Hosts](#-pty-shells--remote-hosts).
- **Remote hosts as first-class targets.** Declare a host once in `[hosts.<name>]`. The AI can then open a shell there and run commands, read files and edit files on it with `host: "<name>"`. Connections reuse one SSH ControlMaster per host. Password, passphrase and remote `sudo` prompts are asked in the chat and never reach a log.
- **Multi-agent orchestration.** A coordinator ghost can spawn specialist agents in parallel, on this host or on a remote one, and collect their results with `await_agent_result`. The daemon enforces each agent's model, memory namespace and tool policy, and caps delegation at two levels. See [Named Agents](#-named-agents--multi-agent-orchestration).
- **Docker Sandboxes isolation.** With `[sandbox] enabled = true`, each ghost run gets its own `sbx` microVM, and chat background commands run in a per-session microVM. Sandboxes have no network by default, their egress is audited, and they fail closed. See [Sandboxed Agents](#-sandboxed-agents--docker-sandboxes-opt-in).
- **One run model.** A chat turn, a ghost, a scheduled job and a delegated agent are all *runs*. Each run has its own turn log, a structured result and a briefing. Cancel any run with `daemoneye stop <run-id>` or `/stop`. Resume a finished ghost with new instructions using `daemoneye run resume`.
- **Watch and join your agents.** `/shells` and `/tail` follow any shell live. `/agents` attaches to a running ghost so you can pause it, resume it, interrupt it, or type into it. `/logs` and `/log` replay finished recordings. `/hosts` lists the configured hosts, and `/shell` opens a shell of your own.
- **Shell tools for the AI.** New tools: `list_shells`, `read_shell`, `find_in_shells`, `open_shell`, `close_shell`, `watch_shell` and `send_input`. There is also a deferred `history` group, `list_shell_logs` and `read_shell_log`, for reading past shell recordings.
- **Alerts reach every chat, live.** Webhook alerts, ghost lifecycle events and background completions are pushed to every connected chat and written to each session's history. A chat reconnects by itself after a daemon restart. `chat --resume` opens with a `[Catch-up]` brief of everything that happened while you were away.
- **Tool calls that correct themselves.** When the model sends a mistyped argument to a real tool, the error names the bad parameter and the model retries. Before, the call was silently dropped.
- **`daemoneye doctor`.** Prints one line per readiness check: config, API key, shell, daemon, sandbox, hosts and tmux.

**Upgrading from 1.x.**
- `[execution] backend = "tmux"` still loads, with a startup warning, and commands run in PTY shells.
- A scheduled job with the retired `Command` action is refused with a logged reason. Recreate it as a script job.
- `daemoneye schedule windows` lists any `de-*` tmux windows 1.x left behind.

---

## Quickstart

```sh
# 1. Build
git clone <repo> && cd daemoneye
cargo build --release

# 2. Install — creates ~/.daemoneye/, copies the binary and writes a
#    systemd user service
./target/release/daemoneye setup

# 3. Add your API key
$EDITOR ~/.daemoneye/etc/config.toml     # set [models.default] api_key

# 4. Start the daemon, and check the host is ready
daemoneye daemon
daemoneye doctor

# 5. Ask it something
daemoneye ask "why is nginx returning 502?"

# …or open an interactive chat, in any terminal
daemoneye chat
```

To have the daemon start on login instead of step 4:

```sh
systemctl --user daemon-reload
systemctl --user enable --now daemoneye
```

---

## Requirements

| Dependency | Notes |
|---|---|
| Linux | `fork(2)`, PTYs, Unix domain sockets |
| Rust 1.79+ | Required by Rust edition 2024 |
| OpenSSH client | Optional. Needed only for `[hosts.<name>]` remote hosts |
| Docker Sandboxes (`sbx`) | Optional. Needed only with `[sandbox] enabled = true`; `daemoneye sandbox status` checks it |
| tmux 2.6+ | Optional. When you run `daemoneye` inside tmux, the prompt carries read-only pane summaries for your session (capture and list only). Without it everything else works the same. The daemon never creates a tmux session |

To install the binary into `~/.cargo/bin` instead of building in place: `cargo install --path .`

---

## How it works

DaemonEye runs as a background daemon holding a Unix domain socket. The chat client connects to it, and the daemon does the work:

```
you type ──→ chat client ──→ daemon
                               │  gathers context, masks secrets,
                               │  assembles context, streams from the model
                               ▼
                          model wants to run something
                               │
                               ▼
                    approval prompt in your chat
                    [Y]es  [A]pprove for session  [N]o  ·  or type a redirect
                               │
                     approved  ▼
                    command runs in a daemon-owned PTY shell —
                    in this session's shell, a dedicated one for
                    `wait: false`, or a sandbox when enabled
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

When a critical alert matches a runbook with `enabled: true`, DaemonEye spawns a Ghost Shell — an unattended AI agent that works the problem on the daemon host without a human present, in a PTY shell it owns. Watch it with `/tail`, or join it with `/agents` to pause, interrupt or type into it. When you next attach, a catch-up brief tells you what happened.

- **Unattended Remediation** — Runs pre-approved remediation scripts, monitors output, and escalates when something unexpected happens.
- **Policy Gating** — Non-sudo commands run freely within your OS permissions; `sudo` is gated to scripts explicitly listed in `auto_approve_scripts` that also have a NOPASSWD sudoers rule installed via `daemoneye install-sudoers`. Two keys are required for every privilege escalation.
- **Turn Budget** — A configurable hard ceiling on AI turns (default 20) ensures the agent cannot loop indefinitely. Individual runbooks may set a lower limit, but never a higher one.
- **Catch-up Brief** — On re-attach, new completions, alerts, and watchdog results are summarised in a `[Catch-up]` message before the first AI token. AI spend during the detach window is included: `Cost during detach: $0.34 (architect $0.20 · ghost-anonymous $0.14)`.

Full walkthrough: [Ghost Shells & Autonomous Remediation](#ghost-shells--autonomous-remediation).

---

### 🤖 Named Agents & Multi-Agent Orchestration

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

**Coordinator pattern.** A coordinator agent can spawn specialist sub-agents in parallel with `spawn_ghost_shell`, each on the daemon host or on a configured remote host (`host?`). Each specialist writes its result to a mailbox file. The coordinator collects the results with `await_agent_result`, then writes a consolidated report. Total elapsed time is the longest specialist, not their sum. Spawn depth is capped at 2 (coordinator + specialists) — the daemon enforces this; the model cannot override it.

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

**Where a command lands:** The approval panel names the shell and host before you commit. Commands run in this session's PTY shell by default, in a dedicated shell for `wait: false`, or on a configured host with `host`.

**Remote script streaming** — A **Ghost Shell** that runs a daemon-host script (one in `~/.daemoneye/scripts/`) on its remote `ssh_target` host streams the script instead of copying it: the script is hex-encoded and piped into the remote interpreter's stdin (`python3`/`perl` decode → `bash /dev/stdin <args>`), so it runs on the remote with no remote disk write and no remote-side setup. Under `sudo` the script is written to its `~/.daemoneye/scripts/<name>` path on the remote instead, because a NOPASSWD sudoers rule must name a fixed path. See the `ssh_target` row of the runbook fields table.

**Remote hosts — first connection** — The first connection to a configured `[hosts]` entry is made **before any shell opens there**: if no ControlMaster is running for the host, the daemon runs `ssh -M -N -f` itself and handles what comes back. A password or key-passphrase prompt is asked **in the chat** (never typed into a shell, so a shell's first command can never be eaten by ssh's own prompt); an unknown host key is refused — accept the fingerprint once from your own terminal, and later connections reuse it. Shells opened after that reuse the master for 10 minutes (`ControlPersist=10m`) without asking again. A Ghost Shell never asks: its connections run with `BatchMode=yes`, so a host that wants a credential fails for it at once with a clear error instead of hanging.

**Tool call history** — For silent tools (reads, memory, search, terminal context, etc.) that don't require an approval prompt, DaemonEye renders a compact history line in chat: `▸ tool(args)` when the tool starts, then `⎿ detail · Xs` when it finishes. The spinner shows a live elapsed timer between the two lines so you always know what the AI is doing and how long it has been running. The status bar shows a session-cumulative count (`· tools: N`) so you can see at a glance how many silent tool calls have occurred in the conversation.

**`/approvals`** — type this at the chat prompt to inspect which approvals are currently active across all five scopes: terminal commands (regular and sudo), scripts, runbooks, and file paths. Use `/approvals revoke` (alias `/approvals reset`) to instantly revoke every session approval and return to fully gated. `/approvals on` and `/approvals off` toggle just the terminal-command class. The status bar shows a compact count-based summary (e.g. `⚡approvals: all · files: 2 · Ctrl+C revokes`) so you always know what the AI can do without opening `/approvals`. Cumulative write-approval and denial counts for scripts, runbooks, and file edits are tracked by the daemon and shown in `daemoneye status`.

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

When a class flag is `true`, approval is pre-granted for the entire class at session start — the `[A]pprove for session` prompt is suppressed since it would be redundant. `/approvals revoke` resets all flags back to fully-gated for the rest of the session.

---

### 🔒 Sandboxed Agents — Docker Sandboxes (opt-in)

> With `[sandbox] enabled = true`, agent work runs inside **Docker
> Sandboxes** (`sbx`) microVMs instead of on your host. The sandbox stays
> **off by default**, so `sbx` is not needed to build, test or run
> DaemonEye. Check it with `daemoneye sandbox status`.

Every approval prompt above is a *behavioural* control: it depends on the
operator reading the command correctly. None of it stops a
degenerate-but-approved command, a script that writes outside its lane, or a
prompt injection that yields a plausible-looking command. Without the
sandbox a compromised agent has exactly the daemon's privileges.

With it:

- **Each ghost run gets its own microVM** (`de-ghost-<id>`), created on
  its first command and destroyed when the ghost exits — on every path,
  including failure and cancel. Its profile is the ghost's agent name.
- **Chat background commands** (`wait: false`) run in your session's own
  sandbox (`de-chat-<id>`), removed once it has sat unused for ten minutes.
- **No network by default.** A profile's `network = "none"` denies all
  egress. `network = "proxy"` uses your global `sbx` network policy;
  `proxy_allow` adds hosts to it and `proxy_deny` blocks them.
- **Every request is audited.** Before a sandbox is removed, its `sbx`
  policy log — the hosts it reached and the hosts that were blocked — is
  written to the run's log and to `events.jsonl`.
- **Only the script you approved goes in.** An approved script is copied
  into the sandbox each time it runs; the script library, your config,
  memory and `.ssh` are never mounted.
- **No credentials in the sandbox.** The daemon talks to the model
  provider; the sandbox holds no API key.
- **Fails closed.** If `sbx` is missing or its daemon is not running, a
  sandboxed command is refused with the reason — it never falls back to
  your host.
- **Pause means pause.** `Ctrl+P` on an attached ghost stops the job
  inside the VM, not just the client.

Foreground commands — the ones run in your session's PTY shell — stay on the
host by design: they are visible, approved, and run in your shell.
Commands on a configured `[hosts.<name>]` host run there, not in a sandbox.

Design of record: [`docs/design/daemoneye-2.0.md`](docs/design/daemoneye-2.0.md) § 2.9.

---

### 🐚 PTY Shells & Remote Hosts

Every command the agent runs, in chat, in a ghost, or in a scheduled job, goes to a PTY shell the daemon owns.

- **Shells outlive the daemon.** Each shell is hosted by a detached `daemoneye shell-host` process, so a daemon restart adopts its running shells instead of killing them.
- **Real exit codes.** A marker protocol wraps each command, so the agent gets the command's actual exit status, never a guess.
- **Everything is recorded.** Each shell is logged byte for byte as asciicast v2 under `~/.daemoneye/var/log/shells/`, with a per-command index. The agent reads it with `read_shell`, `find_in_shells` and the `history` tools. You replay it with `/logs` and `/log <id>`. All output is masked before the model sees it.
- **Follow and join.** `/shells` lists the live shells, and `/tail <id>` follows one. `/agents` attaches to a running ghost: pause it (`Ctrl+P` stops the job itself, inside a sandbox too), resume it, interrupt it, or type into it. `/shell [host]` opens a shell of your own.
- **Remote hosts.** Declare `[hosts.<name>]` once, and `open_shell`, `run_terminal_command`, `read_file` and `edit_file` accept `host: "<name>"`. The connection runs in a local PTY shell using your own keys and agent, through one SSH ControlMaster per host. Nothing is installed on the remote host, and a command there never starts a tmux server. `/hosts` lists the configured hosts.
- **The agent sees its shells.** Each turn carries `[HOSTS]` and `[SHELLS]` blocks: the configured hosts, and this session's shells with their state. `get_shell_context` gives a fresh snapshot on demand.

---

### 🪟 Inside tmux (optional)

tmux is never required. When you do run `daemoneye chat` inside tmux, the daemon *reads* your session for context and never writes to it. A test gate in `tests/tmux_read_only.rs` rejects any tmux write verb in the source.

- **Pane awareness.** The context carries read-only summaries of your session's panes, each with a live status: `Running`, `Idle 4m`, `AwaitingInput`, `Bell`, `Dead(1)`. `[PANE MAP]` maps window-relative indices to pane IDs (`idx:0=%3* bash | idx:1=%7 vim`), so "what's in pane 1?" resolves. Panes in your other tmux sessions are listed too; their contents are fetched only when asked for.
- **`/pane`.** A window-grouped inspector: cwd, status, activity age and a preview line per pane. `/pane %N` pins the pane whose new activity refreshes the context snapshot. The pin is saved to `pane_prefs.json` and survives a daemon restart.
- **Bell recovery.** Bells raised in your windows while the daemon was down are noticed on the next poll and logged to `events.jsonl`.
- **A chat key binding.** `daemoneye setup` prints one you can add to `~/.tmux.conf`:

  ```sh
  bind-key T split-window -v '~/.daemoneye/bin/daemoneye chat'
  ```

  It uses the full path, so it works even when `~/.cargo/bin` is not on the `PATH` tmux inherits.
- **Outside tmux,** `[daemon] tmux_session` can name an existing session for the daemon to observe. The daemon never creates one.

---

### 🖥️ Terminal-Native Chat Interface

The chat client is built on a `ratatui` inline viewport that treats your terminal the way a good CLI tool should.

- **Real scrollback** — The conversation transcript is committed to your terminal's native scrollback. Scroll up in your terminal (or tmux copy-mode) and the whole history is there, clean and selectable; only the input box and status bar occupy a small fixed region at the bottom.
- **Survives resizes and window switches** — Resizing the terminal, or switching tmux windows away from the chat and back, leaves the transcript and chrome intact. The renderer keeps a history of every committed line. When the grid shifts in ways the client can't observe (screen scrolls, height reflows), it clears the stale region and repaints the transcript tail from that history instead of trusting row arithmetic. No orphaned borders, no eaten lines.
- **Multi-line input editor** — Visible cursor, word-wrap, multi-line editing, and multi-line paste. A pasted block lands whole instead of submitting at its first newline.
- **Two-press interrupt that actually stops the work** — While the agent is streaming, press ESC or Ctrl+C once to warn, twice to abort. The second press sends a cancel to the daemon over a fresh connection, so the daemon aborts the in-flight provider stream instead of billing it to completion; the partial response is kept in history with a `⊘ cancelled` marker.
- **No infinite spinner** — The daemon signals liveness at least every 15 s for the whole turn, including during long tool runs. The client holds it to that: 90 s of silence before any output, or 120 s mid-response, ends the wait with an error that says which happened and what to check, rather than spinning forever against a wedged daemon.
- **Silent conditions are surfaced** — Response truncation at `max_tokens`, a provider refusal or content filter, a dropped unknown-tool call, malformed provider frames, and an entirely empty reply each produce a visible `⚠` notice. Previously these only reached `daemon.log` while the chat showed a short or blank answer.
- **Full transcript on demand** — Command output is capped at 10 lines inline (`… N more lines · ctrl+o`). Press `ctrl+o` at any time — including mid-turn while the agent is streaming — to open the whole conversation, every byte included, in an alternate-screen viewer. See [Full-Transcript Viewer](#-full-transcript-viewer-ctrlo).
- **Color-coded panels** — Committed command-output panels use a blood-red border and deep-yellow title so executed actions stand out in the scrollback.

---

### 📜 Full-Transcript Viewer (`ctrl+o`)

Inline panels stay short so the scrollback stays readable — a `run_terminal_command` that printed 4 000 lines shows 9 of them plus `… 3991 more lines · ctrl+o`. The rest is not lost. Press **`ctrl+o`** from the chat prompt, or mid-turn while the agent is still streaming, and the client opens an alternate-screen pager over the full conversation.

- **Every byte is there** — the viewer renders the exact `Response::ToolResult` payload the daemon sent, not a re-read of the event log (which caps output at 200 chars) or the session JSONL (truncated at `limits.tool_result_chars`). Masking is applied on the way in, so nothing redacted in the inline panel appears unmasked in the viewer, and it never reads `var/log/panes/`.
- **Modal and additive** — the inline `insert_before` streaming surface is untouched, so native terminal scrollback, copy-mode and drag selection keep working exactly as before. `esc` restores the inline chat byte-identically: input box, status bar and prior scrollback intact, including after a pane resize while the viewer was open.
- **Block-structured** — user turns, assistant replies, tool panels, command output and system notices are separate blocks you can focus, collapse and expand individually, or all at once.
- **Search** — `/` opens an incremental search across the whole transcript, including inside expanded output bodies; `n` / `N` walk the matches and the status line shows `k/N`.
- **Copy** (inside tmux) — `y` loads the focused block into a tmux buffer (`tmux load-buffer -w`, so it also reaches the system clipboard where the terminal supports OSC 52). Paste it with your tmux paste key. The copied text is the block's real content, decoration-free and independent of whether it is collapsed.
- **Mouse** — wheel scroll and click-to-toggle work inside the viewer. Mouse tracking is enabled *only* while the viewer is up, so it never steals drag-select from the terminal on the main screen.
- **Rehydration** — `/session load <name>` refills the transcript from the saved session, so a fresh client on a resumed conversation shows the prior turns under `ctrl+o` too.
- **Bounded** — the client keeps at most 500 blocks or 8 MiB, whichever binds first, evicting oldest-first. When anything has been evicted the status line says so (`N older blocks evicted`).

Keys are listed in [Transcript viewer keys](#transcript-viewer-keys).

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

**Retention.** Session archives are kept forever unless you set `[sessions] archive_retention_days`. Per-session chat logs are kept forever unless you set `[sessions] log_retention_days`. The event log rotates into dated segments (`var/log/events/events-YYYYMMDD.jsonl`) with a 90-day default retention.

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
- **Failure Isolation** — Each job runs in its own scheduler-owned PTY shell with a real exit code recorded in the shell's log; the shell is swept when the job is done.

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
| `daemoneye daemon --session NAME` | Name the existing tmux session to observe read-only, overriding `[daemon] tmux_session` |
| `daemoneye stop` | Stop the daemon gracefully |
| `daemoneye sandbox status` | Show the sandbox runtime (the `sbx` version, or why it is unusable) and the daemon's own sandboxes |
| `daemoneye ping` | Check whether the daemon is running |
| `daemoneye status` | Daemon status: uptime, sessions, ghost shells, cost today, redactions, circuit state |
| `daemoneye logs` | Tail `daemon.log` |
| `daemoneye chat` | Start an interactive multi-turn chat session |
| `daemoneye ask <query>` | Send a single question to the AI |
| `daemoneye setup` | Initialise `~/.daemoneye/`, install the binary, write the systemd service, print an optional tmux key binding |
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
| `daemoneye schedule windows` | List leftover `de-*` tmux windows from 1.x scheduled jobs (2.0 creates none) |
| `daemoneye session import <id> --name <name>` | Import an orphaned ephemeral session log into the named session store (no daemon required) |
| `daemoneye costs` | Show AI spend from the event log (no daemon required) |
| `daemoneye costs --since DATE --until DATE --by agent` | Filter and group the cost report; `--agent NAME`, `--json` also available |
| `daemoneye install-sudoers <script>` | Write a NOPASSWD sudoers drop-in for `~/.daemoneye/scripts/<script>` |
| `daemoneye reindex` | Rebuild the memory search index from the memory files on disk; reports rows before and after |
| `daemoneye audit-prompts` | Audit installed prompt and knowledge memory files for stale path references; exits non-zero on findings, never writes |
| `daemoneye doctor` | One line per readiness check (config, api key, shell, daemon, sandbox, hosts, tmux); exit 1 only on a FAIL; tmux is optional and never fails it |

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
| `/pane` | Window-grouped pane inspector — cwd, status, activity age and a preview line per pane; `/pane %N` or `/pane <n>` pins the pane whose activity refreshes the context snapshot (alias: `/panes`) |
| `/approvals` | Inspect approval state; `on`/`off` toggle commands, `revoke` gates everything (alias: `/approval`) |
| `/limits` | Show active limits and live session counters; `/limits reset` |
| `/session` | `save`/`load`/`list`/`delete`/`rename`/`diff`/`tag` (alias: `/sessions`) |
| `/shells` | The PTY shells the daemon runs: id, host, owner (`this session`, `user`, `ghost:…`), live or exited, age and last command |
| `/hosts` | `local`, then each configured `[hosts.<name>]` with its ssh destination, ControlMaster state (`master:up`/`down`) and live-shell count |
| `/tail <id>` | A live, read-only view of one shell in the alternate screen, with follow mode, scrollback and search (alias: `/watch`) |
| `/logs` | Past shell recordings, newest first: `/logs [host] [--owner chat\|ghost\|adopted] [--since 2h]` |
| `/log <id>` | Open one recording by the log id `/logs` prints: `n`/`p` step through its commands, `space` replays it at its recorded speed |
| `/attach <id>` | Type into a live shell, full screen: every key goes to the shell except the chords in [Attached mode keys](#attached-mode-keys). A bare `/attach` opens the picker |
| `/agents` | Pick a live shell to attach to from a list in the chat viewport, ghost-owned shells first: `↑`/`↓` select, `Enter` attaches, `Esc` closes |
| `/shell [host]` | Open a shell you own, on the daemon host or a configured host, and attach to it, for example to type a password once before an agent needs the host. Agents see it in their shell list but can drive it only through an approval that is always asked |
| `/runs` | Recent runs (chat turns and ghost runs) with their state |
| `/run <id>` | The last records of one run's turn log |
| `/stop <id>` | Ask a running run to stop (it ends at its next turn boundary) |

At a tool-approval prompt, typing a message instead of `Y`/`A`/`N` redirects the agent. Up/Down navigate the input; at the top or bottom edge they recall history.

### Chat keys

| Key | Effect |
|---|---|
| `ctrl+o` | Open the full-transcript viewer — works at the prompt and mid-turn while the agent is streaming |
| `Esc` / `Ctrl+C` | Once warns, twice aborts the in-flight turn (the daemon cancels the provider stream) |
| `Ctrl+C` at the prompt | Clear the input line; press twice within a second to quit |
| `Ctrl+D` on an empty line | Quit the chat session |
| `Ctrl+A` / `Ctrl+E` | Move to start / end of the current line |
| `Ctrl+U` / `Ctrl+K` | Kill to start / end of the current line |
| `Up` / `Down` | Move within the input; at the top or bottom edge, recall history |

### Transcript viewer keys

Active only while the alternate-screen viewer is open (`ctrl+o`).

| Key | Effect |
|---|---|
| `↑` / `↓` | Scroll one line |
| `PgUp` / `PgDn` | Scroll one page |
| `Home` / `End` | Jump to the top / bottom |
| `[` / `]` | Focus the previous / next block |
| `Enter` | Collapse or expand the focused block |
| `c` | Collapse every output block |
| `a` | Expand everything |
| `/` | Open incremental search |
| `n` / `N` | Next / previous match |
| `y` | Inside tmux: copy the focused block to a tmux buffer (and the system clipboard where supported) |
| Wheel | Scroll |
| Click | Focus and toggle the block under the cursor |
| `Esc` / `q` / `ctrl+o` | Close the viewer and restore the inline chat |

While the search prompt is open, typing edits the query, `Enter` commits it and `Esc` cancels it.

### Shell viewer keys

Active in `/tail` and `/log`, which share the alternate-screen shell viewer.

| Key | Effect |
|---|---|
| `↑` / `↓`, `PgUp` / `PgDn`, `Home` / `End` | Scroll; scrolling up in `/tail` pauses follow |
| `f` | `/tail`: follow the newest output again |
| `/`, then `n` / `N` | `/tail`: search, then the older / newer match |
| `n` / `p` | `/log`: put the next / previous recorded command at the top, with its exit code in the status line |
| `space` | `/log`: replay the recording at its recorded speed (pauses longer than 2 s replay as 2 s); `space` again pauses and resumes, any other key skips to the end |
| `Esc` / `q` / `Ctrl+C` / `ctrl+o` | Close the viewer |

### Attached mode keys

Active while attached (`/attach`, `/agents`, `/shell`). Every other key goes
to the shell.

| Key | Effect |
|---|---|
| `Ctrl+P` | Pause the running command — the shell and its foreground job stop together, so the rest of a loop or command line cannot run on; press again to resume. A ghost run that owns the shell waits at its next turn and is told how long it was paused |
| `Ctrl+C` | Interrupt the foreground job, and cancel the run that owns the shell (it ends at its next turn boundary) |
| `Ctrl+D` | Detach back to chat. Detaching while paused resumes the job first |
| `Ctrl+D` `Ctrl+D` | Send one `Ctrl+D` (EOF) to the shell |

What you type while attached is recorded in the shell's own log (scrubbed
at password prompts), and as a byte count, never the bytes, in
`events.jsonl`. The agent that owns the shell is told at its next turn that
you typed into it.

---

## AI tools

The model has **43 tools**. The 29 **core** tools are sent with every request; the 14 **deferred** tools are omitted by default and pulled in on demand with a single `load_tools` call — a context-budget optimisation independent of the policy gates that restrict tools at execution time.

Tools marked **⚠** require explicit user approval before they execute.

### Core

| Tool | What it does |
|---|---|
| `run_terminal_command` **⚠** | Run a command in a PTY shell. `wait?` picks the mode: omitted or `true` waits and returns the captured output with the real exit code; `wait: false` returns immediately with the shell id, which you then follow with `watch_shell` or `read_shell` (the outcome arrives as a `[Background Task Completed]` message). `shell?` targets a PTY shell (`"s3"` from `list_shells`, or `"new"` for one fresh shell; omitted, the command reuses this session's lowest-numbered open shell and another session's shell is refused); `host?` runs it on a configured `[hosts.<name>]` host (omitted or `"local"` = the daemon host; a shell id on a different host is refused, never relocated); `timeout_secs?` bounds the wait (default 45 s, capped 3600); `interactive: true` launches a program that never exits — a TUI like `vim`, `top` or `claude`, or a REPL — by typing it in and returning the settled screen, with nothing waiting for the program (drive it with `send_input`, read it with `read_shell_screen`; `ssh`/`mosh` launch this way automatically) |
| `edit_file` **⚠** | Create, edit, delete, or copy a file; shows a coloured unified diff before approval. `host?` writes on a configured `[hosts.<name>]` host — omitted or `"local"` = the daemon host; the operation runs in a shell there after the same approval prompt; `sudo: true` edits a root-owned file on that host (`edit` only), with the password asked in the chat and no session approval |
| `write_script` **⚠** | Create or update a script in `~/.daemoneye/scripts/` (written `chmod 700`) |
| `delete_script` **⚠** | Delete a script |
| `write_runbook` **⚠** | Create or update a runbook in `~/.daemoneye/runbooks/` |
| `delete_runbook` **⚠** | Delete a runbook |
| `read_file` | Paginated file read with optional grep filter; masks sensitive data. `host?` reads on a configured `[hosts.<name>]` host through a shell there — omitted or `"local"` = the daemon host |
| `search_repository` | Search runbooks, scripts, memory, or the event log |
| `get_shell_context` | Capture a fresh terminal snapshot on demand; `scope` selects the shells only, the session (default), or all sessions |
| `list_shells` | List the PTY shells the daemon is running — id (`s3`), owner, age, flags (`dedicated` / `exited`) and what held the shell when last observed (`running <cmd>`, `running <cmd> (TUI)`, `paused <cmd>`); `scope` selects this session's shells (default) or all |
| `read_shell` | Read a PTY shell's recorded output — the per-command log behind its asciicast recording, not the live screen. `since_command` starts at a command index, `filter` regex-selects lines, `lines` clamps the tail (default 200, max 2000); output is masked |
| `read_shell_screen` | Read a PTY shell's live screen as of the call — what a person at that terminal sees now, masked. The header names what holds the terminal (a shell prompt, a running command, a full-screen TUI), and a menu's selected row shows as `[SEL: …]`. `lines` tails the visible rows, `scrollback` adds rows that scrolled off the top, `pattern` regex-filters rows. The wait parameters make it block first: `wait_for` until the rendered screen shows a regex, `settle_ms` until the screen stops changing (the "Claude Code finished its turn" primitive; a screen that never settles — `top -d 0.2`, a clock, a spinner, anything redrawing faster than the settle window, while plain `top` redraws every 3 s and can settle between redraws — is a result, not an error), `timeout_secs` bounding either (default 30, cap 300). The screen is lossy; `read_shell` is the record |
| `find_in_shells` | One regex search across every in-scope shell's recorded output — answers "which shell has the error?" in a single call. Reports shell id, 1-indexed line number and the ±1 context; capped at 50 matches across all shells, last 2000 lines per shell; unreadable shells are skipped and named. `scope` selects this session's shells (default) or all |
| `open_shell` | Start a new PTY shell and return its id — on the daemon host by default, or on a configured host with `host`. `rows` / `cols` size its terminal for the program you are about to run (defaults from `[shells]`, clamped to rows 10–200 and cols 40–500); the result names the size it actually opened at. Commands keep running in this session's lowest-numbered open shell by default — pass `shell` to `run_terminal_command` to target the new one, or `shell: "new"` for one command's fresh shell. Readable via `read_shell` / `find_in_shells`, ended via `close_shell` |
| `close_shell` | End this session's PTY shell by id — the id from `list_shells`. Hangs the shell up (ending any command running in it) and reports whether the shell's host actually went away; another session's or a ghost job's shell is refused |
| `watch_shell` | Block on a live PTY shell's new output — the `.cast` recording, read as it is written. With `pattern` (regex) it waits until a line of output written *after the call* matches and returns that line with context; without one it waits until the shell's running command finishes, and reports nothing-running, paused, exited, gone or timeout distinctly. `timeout_secs` optional (default 300, capped 3600). For what a shell already recorded, use `read_shell` / `find_in_shells` |
| `send_input` **⚠** | Type text, press named keys (`keys: ["Escape"]`, `["C-c"]`, `["BTab"]`), or send a multi-line text as one bracketed paste into a live PTY shell by id — answer a program's stdin prompt or drive an interactive process. The approval panel names what the shell is doing, including the honest hazard that a shell at its prompt will run the text as a command. There is no exit code, no wait and no output: read the effect with `read_shell` / `read_shell_screen` / `watch_shell`. Every byte lands in the shell's cast log as an `"i"` event, which no read tool surfaces. A ghost shell may not use it unless its agent `ToolPolicy` explicitly allows it |
| `add_memory` | Store a persistent memory entry |
| `read_memory` | Read a memory entry by key and category |
| `update_memory` | Update individual fields of a memory entry in place |
| `list_memories` | List memory keys, optionally filtered by category |
| `recall_context` | Retrieve archived turns from this session by query, turn range, or both |
| `schedule_command` **⚠** | Schedule a one-shot, interval, or cron job — command, script, or ghost shell |
| `list_schedules` | List scheduled jobs with status and next fire time |
| `cancel_schedule` | Cancel a scheduled job |
| `delete_schedule` | Permanently delete a scheduled job |
| `spawn_ghost_shell` | Delegate a task to an autonomous background Ghost Shell; `host?` runs the specialist on a configured host or "local", fenced by both the runbook's and your own `hosts:` lists |
| `await_agent_result` | Wait for a spawned agent ghost shell and return its result |
| `load_tools` | Pull a deferred tool group into the active tool set |

### Deferred

| Group | Tools |
|---|---|
| `agents` | `create_agent` **⚠**, `read_agent`, `list_agents`, `delete_agent` **⚠** |
| `runbooks` | `read_runbook`, `list_runbooks` |
| `scripts` | `read_script`, `list_scripts` |
| `memory` | `delete_memory` |
| `runs` | `run_log_tail`, `run_log_search`, `get_turn` |
| `history` | `list_shell_logs`, `read_shell_log` |

* `run_log_tail` reads the last N records of one run's turn log (`~/.daemoneye/var/log/runs/<run_id>.jsonl`); limit clamped inside the reader at 200, default 50 — the header says when a request was clamped.
* `run_log_search` searches one run's turn log for records containing a substring, case-insensitively, under the same clamp.
* `get_turn` reads every record one turn logged — the one uncapped escape hatch, since the caller has already narrowed to a single turn.
* `list_shell_logs` lists the PTY shell recordings in `~/.daemoneye/var/log/shells/`, newest first — live and finished sessions alike. Each row carries the **log id** (`s3-1788470000`, shell id plus start timestamp), its age, its command count and the first command as a hint. Recordings without a readable command index are listed with `commands: unknown`, never 0. `since` (unix timestamp) keeps the recordings at or after it; `limit` defaults to 50, capped at 500, and the result says when a request was clamped.
* `read_shell_log` reads one recording by its **log id** — the id `list_shell_logs` prints. A bare shell id (`"s3"`) is refused, because ids are reused across daemon restarts and picking the newest match would read the wrong session. `command_index` reads one command (its exit code is in the header; an index past the highest recorded is an error naming the highest), `filter` is a regex (invalid = error) and `lines` clamps the tail (default 200, max 2000). Output is masked.

---

## Installation details

### `daemoneye setup`

Run it once after building. It initialises the full `~/.daemoneye/` tree, copies the binary to a stable location, writes a systemd user service file, and prints an optional tmux key binding for summoning a chat (see [Inside tmux](#-inside-tmux-optional)).

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
      panes/              ← legacy; swept at startup (nothing writes here now)
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

The service needs no terminal and no tmux. Every command, whether from chat, a ghost, a scheduled job or webhook automation, runs in daemon-owned PTY shells. Alerts reach chat clients over IPC whenever one connects. To give the service read-only pane context from an existing tmux session, name it in `[daemon] tmux_session`. The daemon never creates that session.

```sh
systemctl --user daemon-reload
systemctl --user enable --now daemoneye     # enable and start on login
systemctl --user status daemoneye           # check status
systemctl --user restart daemoneye          # restart after a config change
systemctl --user stop daemoneye
systemctl --user disable daemoneye          # disable autostart
```

View logs with `daemoneye logs` (tails `~/.daemoneye/var/log/daemon.log`) or through journald: `journalctl --user -u daemoneye -f`.

---

## Configuration

DaemonEye stores its configuration in `~/.daemoneye/etc/config.toml`. The file is created automatically on first launch with default values. Every key below also appears — commented, with its default — in the seeded template (`assets/etc/config.toml`); a test gate keeps that template in sync with the config structs, so the file on disk is always a complete reference.

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

# [ai]
# prompt                   = "sre"  # prompt file in ~/.daemoneye/etc/prompts/ (without .toml)
# first_token_timeout_secs = 600     # max wait for the first token (retried, bounded)
# stream_idle_timeout_secs = 240     # max gap between chunks mid-stream (never retried)
# connect_timeout_secs     = 30      # TCP/TLS connect budget; there is no total request timeout

# [context]
# environment = "personal"   # "personal" | "development" | "staging" | "production"

# [masking]
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]

# [approvals]
# commands       = true    # non-sudo terminal commands auto-approve at session start
# sudo           = false
# scripts        = false
# runbooks       = false
# file_edits     = false
# ghost_commands = false

# [ghost]
# max_ghost_turns = 20   # hard ceiling; individual runbooks may set lower
# max_concurrent_ghosts = 3  # 0 = unlimited

# [limits]
# per_tool_batch            = 100    # max consecutive calls of one non-approval tool per turn (0 = unlimited)
# total_tool_calls_per_turn = 0      # max total non-approval tool calls per turn (0 = unlimited)
# tool_result_chars         = 16000  # max chars fed back to the AI per tool result (0 = unlimited)
# max_turns                 = 0      # max AI turns per chat session (0 = unlimited; ghosts use max_ghost_turns)
# max_tool_calls_per_session = 0     # cumulative non-approval tool calls per session (0 = unlimited)
# turn_timeout_secs         = 0      # wall-clock ceiling on one chat turn incl. tools (0 = no deadline)

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

# [logging]
# log_max_bytes  = 5242880  # rotate daemon.log past this size (default 5 MiB)
# log_keep_count = 5        # rotated copies to keep

# [retention]
# pane_log_retention_days = 7   # delete var/log/panes/*.log older than this (0 = keep forever)
# mailbox_retention_days  = 7   # delete agents/*/mailbox/*.json older than this (0 = keep forever)

# [daemon]
# tmux_session = "daemoneye"   # existing tmux session the daemon observes when launched outside tmux (never created); "" = no tmux

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
| `max_tokens` | integer | `8192` | Maximum tokens the model may generate per response. Raise it for long-form answers or models whose reasoning tokens count against the output budget. |
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
| `first_token_timeout_secs` | integer | `600` | Max wait for the **first** token of a response. Covers slow prefill on long prompts and cold local models. A stall here is retried a bounded number of times, because no output has reached you yet. |
| `stream_idle_timeout_secs` | integer | `240` | Max gap between chunks **once the stream has produced a token**. A stall here is never retried — tokens already reached you, and a re-issue would duplicate output. |
| `connect_timeout_secs` | integer | `30` | TCP/TLS connect budget. This is the **only** timeout on the shared HTTP client: there is deliberately no total request timeout, so a long generation is never killed for taking a long time. |

These are two-phase stream bounds, and the split matters: a generation may run
for as long as it likes provided tokens keep arriving. Only a genuine stall —
no first token within `first_token_timeout_secs`, or no data for
`stream_idle_timeout_secs` mid-stream — ends it, and the error names which of
the two happened.

### `[context]` section

Declares the operating environment so the AI calibrates caution, blast-radius assessment, and security posture. The value is injected into the system prompt.

| Key | Type | Default | Description |
|---|---|---|---|
| `environment` | string | `"personal"` | One of `"personal"`, `"development"`, `"staging"`, `"production"`. |

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

### `[approvals]` section

Seeds the approval state at the start of every session — for `daemoneye chat` and `daemoneye ask` alike. Setting a class to `true` pre-grants approval for that whole class, so the per-call `[Y]es / [A]pprove session / [N]o` prompt is skipped for it. `/approvals revoke` gates everything again for the rest of the session, regardless of these defaults. See § **Collaborative Execution & Safety** for the runtime `/approvals` controls.

| Key | Type | Default | Description |
|---|---|---|---|
| `commands` | bool | `true` | Non-sudo terminal commands. Defaults to `true` because they run as the daemon user and are bounded by OS file permissions — the same trust model ghost shells use. |
| `sudo` | bool | `false` | Sudo terminal commands. |
| `scripts` | bool | `false` | All `write_script` calls. |
| `runbooks` | bool | `false` | All `write_runbook` calls. |
| `file_edits` | bool | `false` | All `edit_file` calls. |
| `ghost_commands` | bool | `false` | Let ghost shells run non-sudo commands without the script being listed in the runbook's `auto_approve_scripts`. OR-ed with the per-runbook `auto_approve_commands` frontmatter field. |

### `[webhook]` section

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Start the HTTP webhook server on daemon startup. |
| `port` | integer | `9393` | TCP port to listen on. |
| `bind_addr` | string | `"127.0.0.1"` | IP address to bind the webhook listener. Set to `"0.0.0.0"` to expose on all interfaces — **only with `secret` set**: a non-loopback bind with an empty secret is refused and the daemon fails to start, because every webhook post can launch a ghost shell that runs arbitrary commands. |
| `secret` | string | `""` | Bearer token for authentication. Empty = no auth. |
| `auto_analyze` | bool | `true` | Run runbook-based AI analysis when a matching runbook exists. |
| `severity_threshold` | string | `"warning"` | Minimum severity to trigger AI analysis and `on_alert`. One of `"info"`, `"warning"`, `"critical"`. |
| `dedup_window_secs` | integer | `300` | Suppress duplicate alerts with the same fingerprint within this many seconds. |

The listener also enforces a per-IP rate limit of 30 requests per 60-second window (a compile-time constant, not configurable). A bind failure at startup is fatal — it is treated as a duplicate-instance signal, not a transient error.

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
| `turn_timeout_secs` | integer | `0` | Wall-clock ceiling on a single chat turn, including tool execution. On expiry the turn aborts with a visible error naming the limit. `0` = no deadline. Ghost shells use their own 300 s per-turn bound instead. |

Per-tool overrides via `[limits.per_tool]` let you raise or lower `per_tool_batch` for specific tools:

```toml
[limits.per_tool]
read_file         = 200   # allow more read_file calls than the global batch cap
search_repository = 25    # tighten search calls
```

Use `/limits` in the chat to inspect the active values and live session counters. Use `/limits reset` to zero the per-session tool counter without ending the session.

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

### `[logging]` section

Rotation for `~/.daemoneye/var/log/daemon.log`. The live log was observed reaching 25.8 MB in about twelve weeks; the defaults bound total on-disk log usage to roughly 25 MB while keeping enough history to debug from.

| Key | Type | Default | Description |
|---|---|---|---|
| `log_max_bytes` | integer | `5242880` (5 MiB) | Rotate `daemon.log` once it exceeds this size. Rotated files are `daemon.log.1` … `daemon.log.N`. |
| `log_keep_count` | integer | `5` | Number of rotated files to keep. |

### `[retention]` section

Periodic sweeps for the two artifact directories that otherwise grow without bound. Both are swept by the daemon; `0` disables the sweep for that class.

| Key | Type | Default | Description |
|---|---|---|---|
| `pane_log_retention_days` | integer | `7` | Delete captured pane logs (`var/log/panes/*.log`, legacy logs from 1.x background jobs; 2.0 writes none) older than this many days. `0` = keep forever. |
| `mailbox_retention_days` | integer | `7` | Delete agent mailbox results (`agents/<name>/mailbox/*.json`) older than this many days. `0` = keep forever. |

### `[execution]` section

Selects the substrate `run_terminal_command` runs on. `"pty"` is the only
substrate: the daemon's PTY shells. `"tmux"` was removed in 2.0; it still
loads, with a startup warning, and commands run in PTY shells.

| Key | Type | Default | Description |
|---|---|---|---|
| `backend` | string | `"pty"` | `"pty"` is the only substrate: the daemon's PTY shells, recorded as asciicast v2 logs in `~/.daemoneye/var/log/shells/`. `"tmux"` was removed in 2.0: it still loads, with a startup warning, and commands run in PTY shells. Any other value falls back to `"pty"` with a warning. |

### `[shells]` section

Settings for the PTY-backed shell engine (`[execution] backend = "pty"`). All
values are checked at daemon startup; see the lifecycle table in
`src/config/lifecycle.rs` for the artifact rows these sweeps cover.

| Key | Type | Default | Description |
|---|---|---|---|
| `max_per_owner` | integer | `5` | Maximum concurrent shells one owner (a chat session, a ghost) may hold. `0` = unlimited. |
| `exited_retention_secs` | integer | `300` | How long an exited shell's registry record stays listed before it is reaped. `0` = reap a dead shell on the pass after its host is first seen dead. The `.cast` log has a separate, day-based retention below. |
| `log_retention_days` | integer | `7` | Days to keep `var/log/shells/*.cast` (and their `.meta.json` sidecars). `0` = keep forever. A shell still in the registry is never swept, however old its log. |
| `scrollback_lines` | integer | `5000` | Rows of scrollback the screen model retains per shell. |
| `rows` | integer | `40` | Rows of the PTY every shell spawns with. Clamped to `10..=200`, so `0` cannot open an unusable terminal. |
| `cols` | integer | `120` | Columns of the PTY every shell spawns with. Clamped to `40..=500`. |

```toml
[shells]
max_per_owner        = 5
exited_retention_secs = 300
log_retention_days   = 7
scrollback_lines     = 5000
rows                 = 40
cols                 = 120
```

### `[hosts.<name>]` sections

Named remote hosts the AI may run on. Each entry makes `host: "<name>"` valid on `run_terminal_command`, `read_file`, `edit_file`, `open_shell` and `spawn_ghost_shell`. The daemon host is implicit and never configured here; `local` is a reserved name. Names match `[A-Za-z0-9._-]`, 1–64 characters.

```toml
[hosts.pinky]
ssh = "pinky"          # any ssh(1) destination; ~/.ssh/config is honoured
```

| Key | Type | Default | Description |
|---|---|---|---|
| `ssh` | string | *(required)* | The `ssh` destination. An entry without it is a config error. |
| `tags` | list of strings | `[]` | Free-form labels for grouping; not interpreted by the daemon. |
| `sudo_scripts` | list of strings | `[]` | Scripts a Ghost Shell may run under sudo on this host (the remote-sudoers model), enforced when a runbook's `ssh_target` resolves here. An empty list restricts nothing. |
| `sandbox_profile` | string | *(none)* | Container profile for this host's commands. Parsed, **not yet enforced**. |

**How a remote host is reached.** A remote shell is a PTY on the daemon host running your own `ssh`, with your keys and agent; nothing is installed on the remote. The first connection opens a ControlMaster under `~/.daemoneye/var/run/ssh/` (kept for 10 minutes), and later shells to the same host reuse it without authenticating again. A password, key passphrase or remote `sudo` password is asked **in the chat** and never reaches a shell log: the answer is recorded as `[redacted]`. An unknown host key is refused; accept it once from your own terminal. Ghost Shells connect with `BatchMode=yes` and may only use hosts their runbook's `hosts:` list names.

A host name that is **not** configured is still usable from chat (it is passed to `ssh` as-is), but its approval panel is classified as a **new host** and is never covered by a session approval.

### `[daemon]` section

Controls daemon startup and session ownership. Use this when running DaemonEye as a systemd user service so ghost shells, scheduled jobs, and webhook-triggered automation work without any `daemoneye chat` client connected.

| Key | Type | Default | Description |
|---|---|---|---|
| `tmux_session` | string | `"daemoneye"` | Name of an existing tmux session the daemon observes when launched **outside** tmux. The daemon never creates it. When launched inside tmux, the current session is used and this setting is ignored. `""` means no tmux at all. |

`daemoneye chat` runs in place and never attaches to a tmux window.

### `[sessions]` section

Controls named session persistence — saving and resuming conversation history across daemon restarts.

| Key | Type | Default | Description |
|---|---|---|---|
| `auto_name_enabled` | bool | `true` | After `auto_name_turn_threshold` turns, prompt the user with an AI-suggested session name in the chat. |
| `auto_name_turn_threshold` | integer | `10` | Number of user turns before an auto-name suggestion is offered. |
| `load_recent_turns` | integer | `10` | Number of most-recent turns loaded when resuming a saved session with `/session load`. `0` loads the complete history (may exceed the context window). |
| `archive_retention_days` | integer | `0` | Delete session archive files (`<id>.archive.jsonl`) whose mtime is older than this many days. `0` = keep forever. Archives belonging to active sessions are never swept. |
| `log_retention_days` | integer | `0` | Delete an inactive session's chat log (`<id>.jsonl`) and its resume state (`<id>.meta.json`) once the log is older than this many days. `0` = keep forever. Archives and epoch files are never touched. |

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

Ghost Shells are unattended AI agents that DaemonEye can spawn automatically in response to incoming webhook alerts. When triggered, a ghost shell investigates the alert and executes pre-approved remediation steps — all without a human present. The ghost itself runs headless inside the daemon. Its commands run in a PTY shell the ghost owns — in its sbx sandbox when one is enabled, otherwise on the daemon host or on a configured `[hosts]` entry its runbook's `ssh_target` names — reused for its later commands and closed when the ghost exits; the recording stays, for `/logs` and `/log`. An `ssh_target` that names no configured host is refused before the ghost starts, in every mode. Start, completion, and failure events appear in the next catch-up brief when you re-attach.

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
  • Loads ghost_config from runbook frontmatter
  • Applies named agent profile if runbook specifies agent: <name>
  • Injects prior briefing state as [Previous Session Summary] context
  • Injects [Ghost Shell Started] into all active chat sessions
        │
        ▼
Ghost AI turn loop (up to max_ghost_turns)
  • Reads runbook + alert context as system prompt
  • Issues run_terminal_command (wait=false only)
  • Policy gate: non-sudo commands always allowed (OS permissions are the boundary);
    sudo commands must be in auto_approve_scripts + have a NOPASSWD sudoers rule;
    named agent tool policy enforced independently at IPC layer
  • Script dispatch: without ssh_target, bare/relative names resolve to
    ~/.daemoneye/scripts/<name> (+ sudo prefix if run_with_sudo: true);
    with ssh_target, scripts are hex-streamed to the remote interpreter's
    stdin by default (no remote disk write) — sudo cases only materialize
    the script to the sudoers-authorized path on the remote host
  • wait=false leaves a dedicated PTY shell — the run is followed with watch_shell
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
| `ssh_target` | string | *(none)* | SSH destination (e.g. `user@host` or `host`) for remote execution. When the value resolves to a configured `[hosts]` entry (see below), the ghost's commands run in a PTY shell **on that host** — the same substrate chat uses — opened with `-o BatchMode=yes` (key auth only: a host that asks for a password or presents an unknown key fails at once, with the prompt cancelled and named), reused for the ghost's later commands on that host, capped at 300 s per command, and closed when the ghost ends. A value that resolves to no configured host is refused — the ghost's commands do not run. Daemon-host scripts are **streamed** to the remote via hex-decode → interpreter stdin by default (no remote disk write); when `sudo` is required the script is materialized to its `~/.daemoneye/scripts/<name>` path on the remote (a NOPASSWD sudoers rule must cover that path). At ghost start the value is resolved against `[hosts]` in `config.toml`: a value naming a `[hosts.<name>]` key, or matching exactly one entry's `ssh` destination, resolves to that host (recorded as `ghost_ssh_target_resolved`), and that host's `sudo_scripts` — when non-empty — limits which scripts may run under sudo there; a value matching two or more destinations resolves to none. A non-empty `hosts:` list must grant the resolved host, or every command is refused. The AI is instructed not to SSH manually — omit this field for local-only execution. |
| `auto_approve_commands` | bool | `false` | Explicitly tell the ghost shell it may run non-sudo investigation commands freely. Non-sudo commands are always permitted by OS permissions regardless of this flag; setting it to `true` makes that explicit in the system prompt so the model does not withhold useful investigation commands. Can also be enabled daemon-wide via `[approvals] ghost_commands = true` in `config.toml`; the two sources are OR-ed together. |
| `allow_tui_input` | bool | `false` | Let the ghost type with `send_input` into a shell whose foreground is a running program (line mode or TUI) — answering an installer's questions, working through a TUI. Without it a ghost may type only into a shell at its prompt. It widens **where** a permitted ghost may type, never **whether**: the agent `ToolPolicy` must still explicitly allow `send_input` first. A paused, exited or unknown shell stays refused either way. |

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

Watch the ghost shell in real time from `daemoneye chat`: `/agents` lists the
live agent shells (ghosts first); `Enter` attaches, `Ctrl+P` pauses and
resumes, `Ctrl+D` detaches. `/tail <id>` follows a shell read-only.

```bash
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

List the ghost shells currently open with `/shells` in `daemoneye chat`
(owner `ghost:…`).

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

Masked values are replaced with placeholder tokens (`<REDACTED>`, `<JWT>`, `<DB_URL>`, `<GITHUB_TOKEN>`, etc.). Review the context shown in the chat before submitting if you handle highly sensitive data.

Add organisation-specific patterns to `extra_patterns` in `[masking]`. Built-in patterns always run — user patterns extend the set, never replace it. Redaction counts by type are tracked across the daemon's lifetime and displayed under **Redactions** in `daemoneye status`, giving a continuous audit view of what categories have been filtered. All built-in types are always shown (including those with a zero count), and hits from user-configured `extra_patterns` are tallied separately as `"User Defined"`.

The same filter runs on webhook alert payloads before they enter the conversation, and on ghost briefings before `briefing.md` is written to disk — so a model cannot launder a secret through a briefing file.

### Sudo passwords

When a command (foreground or background) requires `sudo`, the daemon first checks whether credentials are already cached (`sudo -n true`). If cached, the command runs without any interruption. If not cached, the chat interface prompts for your password with terminal echo disabled — you always type it in the chat, never into a shell, eliminating the risk of keystrokes landing in the wrong window.

Up to 3 attempts are permitted. A wrong password is detected from the shell output ("Sorry, try again.") and you are re-prompted automatically. If all attempts fail or you cancel, the AI receives a structured error describing what happened and suggesting `daemoneye install-sudoers` where appropriate.

The password is never written to disk, stored in a log file, or transmitted to the AI. The in-memory credential is held in a `zeroize::Zeroizing<String>` that overwrites the allocation on drop.

### `sudoers.d` integration

`daemoneye install-sudoers <script>` writes a NOPASSWD drop-in to `/etc/sudoers.d/daemoneye-<name>` that pins the exact absolute path of the approved script — no wildcards, no `ALL`. Privilege escalation requires both an `auto_approve_scripts` entry in the runbook and a matching sudoers rule; either alone is insufficient.

### Agent config protection

Agent configs cannot be written by AI tools without user approval (the same gate as `edit_file`). An agent cannot modify its own config or another agent's config.

---

## Command audit log

Every command the AI proposes — whether approved, denied, or timed out — is recorded as a JSON object in a dated segment under `~/.daemoneye/var/log/events/` (`events-YYYYMMDD.jsonl`, UTC). AI cost records are written to the same segment after each completed turn:

```
{"cmd":"ps aux --sort=-%mem","event":"command","exit":0,"mode":"shell","out":"USER PID ...","pane":"s0","session":"abc123","shell":"s0","status":"approved","ts":"2026-05-16T10:22:58Z","wait":true,"pid":41237}
{"cmd":"sudo rm -rf /tmp/old","event":"command","mode":"foreground","out":"","pane":"","session":"abc123","shell":null,"status":"denied","ts":"2026-05-16T10:22:59Z","wait":true,"pid":41237}
{"event":"ai_cost","ts":"2026-05-16T10:23:01Z","agent_name":"chat","provider":"anthropic","cost":{"total_cost_usd":0.0847},...}
```

`wait` is `true` for a waited command and `false` for a `wait: false` one. `shell` is the PTY shell id for a run and `null` for an approval decision, which ran nothing. `exit` is the command's real exit code, present only on a run. `mode` and `pane` are kept for existing readers; new readers should use `wait` and `shell`. `status` is `approved` / `denied` / `timeout` / `refused`, or a path-specific failure. `out` is the first 200 characters of output.

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
