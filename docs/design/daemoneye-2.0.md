# DaemonEye 2.0 — Standalone Agent Manager & Tool Broker

**Status:** plan of record, drafted 2026-09-03; **PE decisions recorded
2026-09-03** (§ 8). `m19-sandbox-completion` (M18 + M19) was fast-forwarded
into `master` and **`v1.0.0` tagged at `029ab1a`** the same day. No 2.0
milestone is scoped or dispatched yet. Written against `master` @ `9c1287e` (v0.9.9, 78,668 lines of Rust)
with the `m19-sandbox-completion` branch (131 commits ahead) read alongside it.

**One-line summary.** Cut tmux out of the daemon entirely. The daemon becomes
an agent manager and tool broker that owns its own PTY-backed shells — local
shells running as the daemon's user, and SSH shells to remote hosts launched
from those local PTYs. The user talks to it through `daemoneye chat` from any
terminal. Every agent shell is logged, live-tailable and reviewable, by the
user (slash commands) and by agents (tools). The run/turn model, governor,
structured turn log, briefings, cancellation and telemetry that rexyMCP proved
out over 45 milestones come across as first-class daemon machinery.

---

## 1. Why

### 1.1 What tmux bought us, and what it costs

tmux gave 1.x four things for free: a terminal emulator, persistence of shells
across daemon restarts, a place for the user to *watch* a job, and a way to
inject commands into the user's own shell. It costs:

- **3,791 lines in `src/tmux/`** plus the deepest coupling in the tree —
  `executor/foreground.rs` (1,399 lines) is a heuristic completion-detector
  over `pane_pid` polling, output-stability polling for remote panes, a
  `pane-title-changed` hook per command, `monitor-silence`, and an opt-in
  `DE_EXIT` shell hook that falls back to *optimistic exit 0* when absent.
  M12 and M14 each found real defects here that 1,200+ green tests could not.
- **tmux concepts leak all the way into the model.** `%N` pane ids,
  `idx:K`, `[SESSION TOPOLOGY]`, `[PANE MAP]`, `[FOREGROUND TARGET]`,
  `target_pane`, `retry_in_pane`, `de-bg-*` window names, and six of the 36
  tools (`list_panes`, `read_pane`, `find_in_panes`, `watch_pane`,
  `tmux_control`, `close_background_window`) exist only because the substrate
  is tmux.
- **Remote hosts are not a concept.** "Remote" today means
  `pane_current_command == "ssh"` (`utils/host.rs:11`) — no host, user or port
  is ever known, so remote completion is a stability poll and remote file ops
  scrape `capture_pane`.
- **Nine tmux hooks** installed at startup (`daemon/mod.rs:214`, `:557-700`)
  are a live regression surface — the `#{q:session_name}` quoting bug of
  2026-08-17 silently unset four of them in production.
- The user is *required* to run tmux, and the chat client execs
  `tmux attach-session` when started outside it.

### 1.2 What we keep

Everything that is not tmux is good and stays. The survey confirmed these
modules have zero tmux references and port as-is: `config/`, `memory/` +
the FTS5 index, `scheduler.rs`, `runbook.rs`, `scripts.rs`, `session_store.rs`,
`search.rs`, `cost.rs`, `header.rs`, `sys_context.rs`, `agents/` (config,
`ToolPolicy`, mailbox), `daemon/{instance,ready,digest,situational,briefing,
auto_name,memory_prompt,cancel,stats}.rs`, `daemon/context/` (epochs, recall),
`daemon/utils/` (event log, rotation, shell escaping, sudo detection),
`ai/backends/`, `ai/filter.rs`, `ai/tools/{schema,args}.rs`, `webhook/`,
`daemon/policy.rs` (`ExecutionPolicy::wrap_remote` is already the declarative
remote model 2.0 needs). Roughly 55,000 of 78,668 lines are untouched by the
substrate change.

### 1.3 Principles carried forward

- **Trust spectrum, not autonomy switch.** Supervised → session-scoped →
  scheduled → ghost. Every level keeps its gate. No global bypass.
- **Daemon host owns all artifacts; remotes are execution targets only**
  (`architecture.md` § 2.4). Unchanged.
- **Masking on every AI egress path; audit on every action.** Unchanged, and
  extended to shell logs.
- **Measure before speccing.** Every PTY/SSH/terminal-emulator fact in a phase
  doc must have been executed on scrappy first. M18 disproved three design
  claims that way; a PTY layer has more such traps than a container layer.

---

## 2. Target architecture

```
                       ┌──────────────────────────────────────────────┐
  daemoneye chat ◄────►│ daemon (native, runs as the user)            │
  (any terminal)  IPC  │                                              │
                       │  Run manager ── Tool broker ── Shell engine  │
  daemoneye top   JSONL│   runs/turns    approval gate   PTY shells   │
  (read-only TUI) ◄────│   governor      ToolPolicy      vt100 screen │
                       │   briefings     GhostPolicy     shell logs   │
  webhook :9393 ──────►│   cancel        sandbox disp.   marker proto │
  scheduler            │   telemetry     budgets                      │
  MCP (stdio) ◄───────►│                                              │
                       └──────┬───────────────┬───────────────────────┘
                              │ local PTY     │ local PTY running ssh
                              ▼               ▼
                        host shells      remote shells (web01, db01 …)
                        (uid 1000)       (as the user's ssh identity)
                              │
                              ▼ optional (M18/M19)
                        sandboxed container exec
```

Three new subsystems, one rewritten, one deleted:

| Subsystem | 1.x | 2.0 |
|---|---|---|
| **Shell engine** (`src/shell/`) | tmux panes/windows via 40 shell-outs | daemon-owned PTYs, vt100 screen model, byte-exact logs, marker-based completion |
| **Host model** (`src/host.rs`) | string-match on `pane_current_command` | `Host::Local` / `Host::Ssh(target)` registry from config + runtime discovery |
| **Run manager** (`src/daemon/run/`) | chat `SessionEntry` + ghost loop + scheduled loop, three different shapes | one `Run` model for chat turns, ghosts, scheduled jobs and delegated agents; per-run turn log; governor; briefings; cancellation |
| **Tool broker** (`src/daemon/executor/`) | exists; one arm per tool | same choke point, gains `Category`/`mutates_state`, run-scoped guards, shell-aware approvals |
| `src/tmux/`, `pane_prefs.rs`, `daemon/hook.rs`, `cli/notify.rs`, window-prefix scheme | 5,200+ lines | **deleted** |

### 2.1 Shell engine (`src/shell/`)

A **shell** is a PTY the daemon owns, running either the user's login shell or
`ssh -tt <target>`. It replaces panes, windows, `de-*` prefixes, pipe-pane logs,
`capture-pane`, `send-keys`, `remain-on-exit`, and the `DE_EXIT` latch.

```rust
pub struct ShellId(u32);                      // rendered "s7"

pub enum Transport { Local, Ssh { target: String } }

pub enum Owner { Chat(SessionId), Ghost(JobId), Scheduled(JobId), Agent { name, job } , User }

pub struct Shell {
    id: ShellId, host: HostId, transport: Transport, owner: Owner,
    label: String,                            // "cargo-build", "web01"
    created: u64, pty: PtyPair, child: Child,
    screen: vt100::Parser,                    // live screen + scrollback grid
    log: ShellLog,                            // append-only byte log + index
    subscribers: broadcast::Sender<Bytes>,    // live tail feed
    state: ShellState,                        // Idle | Running(cmd, since) | Exited(status) | Detached
    cwd_hint: Option<PathBuf>, last_activity: u64,
}
```

**Lifecycle.** `ShellRegistry` (replaces `SessionCache` + `bg_windows` +
`pane_prefs`) holds `RwLock<HashMap<ShellId, Arc<Shell>>>`. Per-owner caps
replace the 5-window cap. GC closes exited shells after `[shells]
exited_retention` and logs go through the M6 lifecycle-policy table. Startup
sweeps `var/run/shells/`.

**Command execution — the marker protocol.** Deterministic, works identically
over local and SSH PTYs, needs no shell hook on any host:

```
<cmd>; printf '\n\x1f DE_END %s %s\x1f\n' <nonce> $?
```

(fish: `set __de_ec $status`, already handled by `shell_exit_var`). The engine
reads the PTY until the nonce line arrives, returns exactly the bytes between
send and marker (masked, ANSI-annotated), and records the real exit code. No
PID polling, no stability polling, no optimistic zero. The 1.x background
wrapper (`… ; daemoneye notify complete <pane> $?`) is the same idea with an
IPC callback that is no longer needed. **Optional enhancement:** honour OSC 133
(`\e]133;A/B/C/D`) when the remote shell emits it, to detect prompts without
markers for `attach`ed interactive use.

**Interactive commands** (`ssh` with no command, `vim`, `less`, `top`,
`mysql`): the marker never arrives. Handled explicitly instead of
heuristically: `is_interactive_command()` (kept from `utils/shell.rs`) routes
`ssh user@host` to `open_shell(transport: Ssh)`; any other interactive
program runs with `wait: false` and the agent uses `watch_shell` /
`read_shell` / `send_input` to drive it. This is what 1.x's three-way branch
was approximating. **Remote `sudo` is the canonical case** (PE, 2026-09-03):
the agent runs `sudo systemctl restart nginx` in an SSH shell, the PTY shows
the password prompt, the daemon routes it to the chat `CredentialPrompt`
panel exactly as local sudo works today, and the typed secret goes to the PTY
and never to the log or the model. Non-interactive `ssh -T` exec is therefore
**not** a second transport in 2.0: all remote work, including file ops, goes
through PTY shells (§ 2.2).

**Screen vs log.** Two views of every shell, both always available:
- `screen` — a `vt100` grid (cols×rows, resizable, scrollback N lines) for
  "what would a human see" captures. Replaces `capture-pane -e` + `ansi.rs`
  annotation (`ansi.rs` and `status.rs` are kept and pointed at the grid).
- `log` — the raw PTY byte stream, append-only, at
  `var/log/shells/<shell-id>-<unix_ts>-<label>.log` with a sidecar
  `.meta.json` (owner, host, transport, run id, commands with byte offsets and
  exit codes, start/end). Byte-offset index makes "output of command N" an
  O(1) slice, and lets the M11 knowledge index ingest shell logs as a sixth
  corpus without a second copy. Written raw (0600) like session transcripts;
  masked on every read that reaches a model or a tool result.

**Recording format — PE decision: asciicast v2.** Header line + timed
`[t, "o", data]` / `[t, "i", data]` events, plus `"m"` marker events for
command boundaries (asciicast v2 already reserves `"m"` for markers). It is a
JSONL that our tools already know how to stream, tail and grep;
`asciinema play` replays it for free; and the `"i"` events give an exact
record of what the agent *typed*, which `events.jsonl` today captures only as
a 200-char summary.

Cost relative to a plain byte log, measured against the format spec rather
than guessed at: the writer is one `serde_json` line per PTY read (~100
lines including the header and the time base); reads that want raw bytes
concatenate the `"o"` payloads (~40 lines, shared with the viewer). Size
overhead is the JSON framing (~25 bytes per event) plus escaping of control
bytes (`\u001b` for every ESC, `\r\n` for every line end) — roughly 15–30 %
on typical shell output, more for heavy TUI redraws. Each read returns one
event, so an event is typically 1–4 KiB and the framing is noise; a 10 MB
build log becomes ~12 MB. The byte-offset meta index points at event
boundaries instead of raw offsets, which costs nothing. Retention is already
policy-governed. Verdict: low complexity, moderate size, replay and an exact
input record for free.

**Persistence across daemon restarts — PE decision: shells must survive.**
tmux gave this for free; PTY children die with the daemon. So every shell is
owned by a small detached **shell host** process, `daemoneye shell-host --id
sN`, that holds the PTY, writes the log, tracks the marker protocol and
foreground process group, and serves a Unix socket at
`var/run/shells/sN.sock` (peer-uid checked like the main socket). The daemon
is a client of that socket: it sends input/resize/signal frames and receives
output chunks and state events. On startup the daemon scans the directory,
reconnects to every live shell host, and rebuilds the registry from each
host's `.meta.json`; a socket with no live peer is swept. A wedged PTY read
therefore cannot stall the daemon, and a ghost mid-remediation survives a
daemon upgrade. This is the tmux server in miniature (~600–800 lines) and is
an **M20 exit criterion**, not a follow-on. The daemon-side `Shell` API is a
trait so tests can use an in-process implementation.

**Crates.** `portable-pty` (wezterm's, safe wrapper over openpty/forkpty/
`TIOCSCTTY` — keeps `unsafe` out of executor phases; STANDARDS bans it) and
`vt100` for the grid. Both to be measured on scrappy before any phase doc
cites a behaviour (resize, 8-bit output, `\e[?1049h` alt-screen apps,
`bracketed paste`).

### 2.2 Host model (`src/host.rs`)

```toml
[hosts.web01]
ssh = "matt@web01.lan"          # any ssh(1) destination; ~/.ssh/config honoured
tags = ["web", "prod"]
sandbox_profile = "none"        # M18 profile if commands are containerised
[hosts.db01]
ssh = "db01"
sudo_scripts = ["pg-failover"]  # remote sudoers-materialised scripts (1.x model)
```

- `Host::Local` is implicit. Every configured host is an *execution target*;
  none stores artifacts (§ 2.4 of `architecture.md` unchanged).
- SSH runs **as the user, from a local PTY**, using the user's own
  `~/.ssh/config`, keys and `SSH_AUTH_SOCK`. No key ever enters a container
  (M18 D4 holds). Host-key prompts and password prompts surface through the
  existing `CredentialPrompt` panel — the PTY makes them detectable the same
  way sudo prompts are today (`utils/sudo.rs` patterns, extended).
- The daemon sets `-o ControlMaster=auto -o ControlPath=~/.daemoneye/var/run/ssh/%C
  -o ControlPersist=10m` so the second shell to a host is instant and needs no
  re-auth. Measured, not assumed, in the host milestone.
- Runtime discovery: `open_shell(host: "ops@10.0.0.9")` on an unconfigured
  destination is allowed but **approval-gated with a distinct "new host"
  classification**; the daemon offers to persist it to `[hosts]`.
- Runbook `ssh_target` and `ExecutionPolicy::wrap_remote` map onto `HostId`
  unchanged in meaning.
- **Remote file ops go through the host's PTY shell** (PE decision). A
  `read_file`/`edit_file` with `host` borrows an idle shell on that host (or
  opens one), runs the marker-wrapped one-liner, and returns the delimited
  result. One transport, one auth path, ControlMaster shared. If every shell
  on the host is busy with an interactive program the op opens a new shell
  rather than typing into a running one.

### 2.3 Run manager (`src/daemon/run/`) — the rexyMCP run/turn model

Today a chat turn, a ghost, a scheduled job and a delegated agent are four
loops with three shapes. 2.0 has one:

```rust
pub struct Run {
    id: RunId, kind: RunKind,                 // Chat{session} | Ghost{runbook,trigger} | Scheduled{job} | Delegated{agent,parent}
    agent: AgentIdentity, model: ModelRef,
    shells: Vec<ShellId>, budget: Budget,     // turns, tool calls, tokens, wall clock, USD
    governor: GovernorState, cancel: CancelHandle,
    turn_log: TurnLog,                        // var/log/runs/<run-id>.jsonl
    state: RunState,                          // Running | Complete | HardFail(Briefing) | BudgetExceeded(Briefing) | Cancelled{stage,turns}
}
```

Ported from rexyMCP (file anchors are in the rexyMCP repo, cited so the phase
docs can quote the mechanism verbatim, which is what worked in M16):

| Feature | rexyMCP anchor | 2.0 placement |
|---|---|---|
| Structured `RunResult` with `complete / hard_fail / budget_exceeded / cancelled` + bounded **briefing** (goal, criteria, ≤5 working files, attempts ≤200 chars, blocker, budget left) | `executor/src/phase/result.rs`, `phase/briefing.rs:19-120` | Ghost mailbox payload, catch-up brief, `Response::RunFinished`, MCP result |
| **Governor**: ten pure detectors over a `VecDeque<ToolCallSnapshot>` — identical-call (whitespace-normalised args), oscillation, `NoProgressStall` (N non-mutating calls), `LowNoveltyStall` (`normalize_target`: path minus line range, grep scope minus pattern, command minus digits), empty-completion, stuck-gate-feedback, runaway/cumulative output, verifier-persistent, backend-error; **read-only exemption** for the tight detectors; advisory mode + `NoveltySample` events for calibration | `executor/src/governor/hard_fail.rs:115-470`, `config.rs:109-181`, `mcp/src/calibrate_governor.rs` | Replaces the flat turn budget as the ghost's primary safety net; also runs on chat turns (the user sees "governor: low-novelty churn on 3 files" instead of a silent 40-call loop). Requires `Category` on `TOOLS` first. |
| **Cooperative cancellation**: checked at turn top and inside the model-call `select!`; result is a real `Cancelled{reason, stage, turns_done}`; filesystem sentinel `var/run/stop-<run>` polled at 500 ms so `daemoneye stop <run>` works from any terminal | `mcp/src/jobs.rs`, `stop_watcher.rs:20-45`, `agent/mod.rs:287,404` | Generalises `Request::Cancel` (chat-only today) to every run kind |
| **Briefing-seeded resume**, not transcript rehydration: fresh context = runbook + briefing + one directive + current state; task states folded from the turn log | `mcp/src/resume.rs:1-70` | `daemoneye run resume <id> "<directive>"` and the `await_agent_result` → re-spawn path |
| **Turn log**: one JSONL per run, `event_type`-tagged, per-record flushed, domain types serialised verbatim, `#[serde(default)]` on every later field; `run_log_search / run_log_tail / get_turn` tools with clamped limits and one uncapped escape hatch | `store/sessions/event.rs:11-160`, `mcp/src/log_query.rs`, `mcp/src/cap.rs` | Sits beside `events.jsonl` (which stays the audit log); indexed by the knowledge index |
| **Heartbeat during model calls** (`awaiting_model` progress every ~3 s) | `agent/mod.rs:400-520` | Feeds `KeepAlive` and the dashboard; distinguishes prefill from a hang |
| **Seeded tasks**: `## Spec`-style checklist parsed from the runbook's remediation steps, tracked via `update_task`, coverage-checked before a run may complete | `agent/tasks.rs` | Runbooks gain a `## Steps` section that becomes the ghost's task list; "give the agent a condition it can check, not an instruction it can agree with" |
| **Cross-run telemetry**: one `RunRecord` per run (model, params, kind, tags, gates/outcome, tool-success rate, parse-failure rate, length-finish rate, turns, tokens, wall clock, supervision verdict, `failure_class` with `is_model_attributable`) → `daemoneye scorecard` / `daemoneye profile` with `n_runs` on every cell | `store/telemetry.rs`, `mcp/src/profile.rs:20-53`, `scorecard.rs` | Extends `cost.rs`; the "which model handles which incident class" question gets data, and model choice stays the human's |

### 2.4 Tool broker changes

The executor stays the single choke point. Additions, all from rexyMCP:

- **`Category` on every `ToolDef`** (`Read | Write | Search | Run | Meta`) and a
  derived `mutates_state()` — the single source of truth for "made progress",
  which both codebases independently learned must not be a second hand-kept
  list (`APPROVAL_GATED_TOOLS` docstring; rexyMCP `router.rs:26-32`).
- **Tool failures are values, never `Err`**: `ToolResult { output, error:
  Option<String>, metadata }`; unknown tool → `error: Some(..)`.
- **`missing_args_hint`** on arg-deserialisation failure: names missing
  fields, echoes what was supplied, shows an example shape, and says "if
  content is large and calls keep truncating, make a smaller edit".
- **Work-preservation guards in run state, wired around the stateless tools**:
  read-before-edit with mtime check for `edit_file`; refuse `git checkout
  <path>` / `git restore` / `git stash` push that would discard the run's own
  edits; `RefuseInPlaceEdit` for `sed -i` / `perl -i` with a message that
  steers to `edit_file`.
- **Command classifier** (`bash_classify`: `Allow | Block | RefuseInPlaceEdit`,
  command-position regex for `shutdown|reboot|sudo|kill -9|…`) feeding the
  **blast-radius hint** (ROADMAP R9) on the approval panel.
- **Output filter at the tool boundary**: strip ANSI, collapse duplicate line
  runs to `line (xN)`, keep head 20 + tail 80, spill the full text to the
  shell log (which we already have — the recovery file is the shell log
  slice, addressed by `read_shell_log`).
- **Post-write hook**: after `write_script`, run `shellcheck`/`python -m
  py_compile` where available and feed diagnostics back — the rexyMCP lesson
  that a spec instruction cannot guarantee post-write state, only a hook can.

### 2.5 The AI tool surface (36 → 42)

Removed (6): `list_panes`, `read_pane`, `find_in_panes`, `watch_pane`,
`tmux_control`, `close_background_window`. Removed params: `target_pane`,
`retry_in_pane`, `background`.

Changed: `run_terminal_command { command, shell?: "sN"|"new", host?: HostId,
wait?: bool (default true), timeout_secs? }` — `wait:false` returns
immediately with the shell id (the old background mode). `edit_file` /
`read_file` gain `host?` (remote ops run through a shell on that host via the
existing hex-encoded python/perl one-liners, but with marker-delimited
results instead of scraping). `get_terminal_context` → `get_shell_context
{ scope: "run" | "session" | "all" }`. `spawn_ghost_shell` gains `host?`.

Added (12: seven core shell tools, five deferred history tools):

| Tool | Loaded | Gate | Purpose |
|---|---|---|---|
| `open_shell { host?, label? }` | core | approval-gated when `host` is new or remote | Start a PTY shell; returns `sN` |
| `close_shell { shell }` | core | — | End a shell (SIGHUP, then kill after grace) |
| `list_shells { scope? }` | core | — | Own run's shells, or session's, or all (incl. other runs, ghosts, scheduled) |
| `read_shell { shell, lines?, since_command?, filter? }` | core | — | Screen or log slice; masked, annotated |
| `find_in_shells { pattern, scope? }` | core | — | Regex across live shells (replaces `find_in_panes`) |
| `watch_shell { shell, pattern?, timeout_secs }` | core | — | Block until pattern / command exit / timeout |
| `send_input { shell, text, enter?: bool }` | core | **approval-gated** | Drive an interactive program or answer a prompt; every keystroke lands in the `"i"` log |
| `list_shell_logs { host?, owner?, since?, limit? }` / `read_shell_log { id, command_index?, offset?, lines?, filter? }` | **history** (deferred) | — | Review past shell sessions — the agent-side half of the user's requirement. Backed by the `.meta.json` index, so "show me the last time anyone ran `pg_ctl` on db01" is a query, not a scan |
| `run_log_search / run_log_tail / get_turn` | **history** | — | Query past runs' turn logs (ghost post-mortems, "why did the 03:00 ghost stop") |

That is 36 − 6 + 12 = 42 tools, 28 core + 14 deferred; the `doc_truth` test
keeps CLAUDE.md and README in sync as today.

**Prompt context** replaces the pane blocks:

```
[HOSTS] local · web01 (ssh matt@web01.lan, 2 shells) · db01 (configured, idle)
[SHELLS] s3 local  owner:this-run  cwd:/home/matt/src/x  idle   last: "cargo test" → exit 0, 14s ago
         s5 web01  owner:this-run  running "tail -f /var/log/nginx/error.log" 3m
         s2 local  owner:ghost:high-disk-usage  running "du -sh /var/log/*" 40s
[DEFAULT SHELL] s3 — run_terminal_command without `shell` runs here
```

### 2.6 Chat client (`daemoneye chat`)

Runs in any terminal; no tmux detection, no `attach-session` exec, no
`TMUX_PANE`. The ratatui inline viewport, editor, approval/credential panels
and the M17 alt-screen viewer all stay. New or changed slash commands:

| Command | Behaviour |
|---|---|
| `/shells` | Table of live shells: id, host, owner, state, last command, age. Replaces `/panes`. |
| `/tail <sN>` (alias `/watch`) | **Live tail** in the alt-screen viewer: follows the shell's broadcast feed, renders through the same vt100 grid the daemon uses, `f` toggles follow, `/` searches, `Esc` returns to chat. Read-only. |
| `/agents` (alias `/attach`) | **Agent picker**, modelled on Claude Code's subagent selector: an in-viewport list of live agent shells (ghosts, background commands, delegated agents, the user's own shells) with owner, host, state and last line; `↑`/`↓` selects, `Enter` attaches, `Esc` closes. `/attach <sN>` skips the picker. |
| attached mode | Full-screen pass-through to the shell's PTY via `Request::ShellInput`, resize propagated, the vt100 grid rendered locally. Three intercepted chords (PE decision): **`Ctrl-p` pauses** the shell — `SIGSTOP` to the PTY's foreground process group and the owning run parks at its next turn boundary with a `Paused` state; `Ctrl-p` again resumes (`SIGCONT`, run continues and is told "the user paused s5 for 40 s"). **`Ctrl-c` cancels** — cooperative cancel of the owning run (§ 2.3) plus `SIGINT` to the foreground group; the run ends `Cancelled{stage: "user_attached"}`. **`Ctrl-d` detaches** back to chat. Because `Ctrl-d` is intercepted, EOF is sent to the shell with `Ctrl-d Ctrl-d` (double-tap), shown in the attached status line. Everything typed lands in the `"i"` log and in `events.jsonl` as a user-authored input event; the owning agent is told on its next turn that the user typed into its shell. |
| `/logs [host] [--owner ghost\|chat\|sched] [--since 2h]` | List past shell logs from the meta index. |
| `/log <id>` | Open a past shell log in the viewer with command-boundary navigation (`n`/`p` jump between markers) and replay (`space` plays at recorded timing, asciicast). |
| `/runs`, `/run <id>` | Live and past runs with state, budget, governor status; opens the run's turn log in the viewer. |
| `/stop <run>` | Cooperative cancel; the run ends `Cancelled{stage}` and the catch-up brief says so. |
| `/hosts` | Configured hosts, ControlMaster state, shells per host. |
| `/shell [host]` | Open a **user-owned** shell (owner `User`) that agents can see but only drive via approval; useful to pre-authenticate to a host. |

IPC additions: `Request::{ShellList, ShellTail{id, follow}, ShellInput{id, bytes},
ShellResize{id,w,h}, ShellLogList, ShellLogRead, RunList, RunGet, RunStop}` and
`Response::{ShellList, ShellChunk{id, bytes, ts}, ShellExited, ShellLogList,
ShellLogChunk, RunList, RunInfo, RunFinished{result}}`. Payload cap stays
1 MiB; tails stream in chunks.

### 2.7 Read-only dashboard: `daemoneye top`

A pure consumer of the run turn logs, shell registry and `events.jsonl`,
modelled on rexyMCP's seven-panel dashboard (`mcp/src/dashboard/`, 500 ms
poll, `DataFingerprint` stat-only change detection, auto-follows the newest
live run): **Runs** (state, turn, stage, liveness spinner, avg update
interval), **Budget** (tokens, USD today vs `[budget]` cap, tok/s), **Context**
(usage %, compaction/eviction reclaim per lever), **Activity** (filterable
transcript), **Tasks** (runbook steps checklist), **Shells** (live shells with
last line). Strictly read-only — the stop path is `/stop` or `daemoneye stop`.
Cheapest item on the list because everything it reads already exists by then.

### 2.8 DaemonEye as an MCP server (and client)

The rexyMCP experience shows the highest-leverage integration is a small,
structured MCP surface. `daemoneye mcp serve` (stdio, `rmcp`) exposes:
`ask`, `run_command` (through the broker: same approvals via the attached chat
client, or `ToolPolicy` for unattended callers), `open_shell / read_shell /
watch_shell`, `spawn_run { runbook, host, directive }` → `run_id`,
`get_run_status` (bounded long-poll), `stop_run`, `run_log_*`, `search_knowledge`.
Claude Code, Codex or rexyMCP itself can then drive DaemonEye as a fleet
operator with the full audit trail and trust spectrum intact — the plugin
architecture the ROADMAP asked for (R3) without dlopen. Design rule from
`codex-support-plan.md`: host coupling lives in an adapter layer; core Rust
changes only when an installed smoke test proves packaging cannot fix it.

Conversely, `[mcp.servers.<name>]` lets the daemon **consume** external MCP
tools (PagerDuty, Grafana, GitHub) as deferred tool groups, brokered through
the same approval gate — the alert-provider plugins of ROADMAP I8.

### 2.9 Sandbox integration (M18/M19)

The container backend is orthogonal to the substrate change and gets simpler:
a sandboxed command is a shell whose transport is `docker exec -it …` — the
`Transport` enum gains `Container { id, profile }`, the marker protocol is
unchanged, and the D3 requirement "the user can still watch the job in a
pane" is met by `/tail` rather than by a `de-bg-*` window. `Host` profiles
choose `sandbox_profile`; ghosts default to sandboxed local shells exactly as
M19 phase-03 closed. The escape hatch (M19 phase-09) becomes an approval
classification on `open_shell { transport: Local }` from a sandboxed run.

---

## 3. Security model deltas

- **Identity.** The daemon and every local shell run as the invoking user
  (uid 1000). Remote shells run as whatever `ssh` authenticates as, using
  the user's agent — DaemonEye never holds a private key. `SO_PEERCRED` on the
  socket remains the primary boundary; `/attach` and `send_input` are behind
  it, so only the user's own processes can type into an agent shell.
- **Shell logs at rest** are raw and 0600 (as session transcripts are today,
  `security.md` § 2). Every read that reaches a model or the client goes
  through `mask_sensitive`. `"i"` (input) events additionally scrub anything
  typed while the screen showed a password/passphrase prompt (detected via the
  sudo/ssh prompt patterns) — the credential never reaches the log.
- **New approval classifications** in the panel: *new host*, *remote shell*,
  *interactive input*, *escape hatch*. Session-scope approval (`A`) is per
  classification and per host, shown in the status bar as today.
- **Blast-radius hint** (R9) computed daemon-side from the command classifier
  before the prompt is sent.
- **Runbooks gain `hosts: [...]`** — a ghost may open shells only on listed
  hosts; an unlisted host is a parked escape-hatch request (M18 D6 pattern).
- **Governor is a safety control, not ergonomics**: a ghost that tripped
  `NoProgressStall` ends in `HardFail(briefing)` and the brief lands in the
  catch-up; it does not get 20 turns of `systemctl status`. On **chat** turns
  the governor is **advisory only** (PE decision): a fired detector renders a
  status-bar warning and a `SystemMsg`, never terminates the turn.
- **Pause and cancel are user-only controls**: `Ctrl-p`/`Ctrl-c` in attached
  mode arrive over the peer-authenticated socket; no tool can pause or
  cancel another run.

---

## 4. Additional features that fall out of this shape

Ranked by (value × fit) / cost; the first four are in the milestones below.

1. **Shell log replay + search as a sixth knowledge corpus** — `recall_context`
   and `search_repository` can answer "what did we run on db01 during the
   last failover" from the meta index. Postmortem generation (ROADMAP I4)
   becomes a query over the ghost's run log + shell logs, not a reconstruction.
2. **`daemoneye doctor`** (R5): ssh reachability per configured host,
   ControlMaster, PTY allocation, vt100 self-test, docker rootless, sudoers,
   webhook port, index integrity. Setup calls it post-install.
3. **Budget cap that actually opens the breaker** (`[budget] max_cost_per_day`,
   the R2 leftover) — trivial once every run carries a `Budget`.
4. **Auto-learned runbook proposals** (I3): a `complete` ghost run's task list
   + commands + hosts → `runbooks/_proposed/<alert>.md` for review. The turn
   log makes the reconstruction exact.
5. **Fleet primitives** (I1) scoped to runbooks: `spawn_run` fans one runbook
   over `hosts: [...]` with a concurrency cap and a per-host shell; results
   aggregate into one briefing. No free-form `ssh $host $cmd`.
6. **Second-pair-of-eyes gate** (I7): `requires_review: true` runbooks post
   the briefing-shaped proposal to a webhook and wait for an approve token —
   the run model's `Cancelled`/`Complete` states already fit.
7. **Business-hours gates** (I6) on `Budget` (after-hours turn cap).
8. **TCP egress rules for the sandbox proxy** (M19 PE note: HTTP-only is a
   deferral) — with SSH now a first-class daemon-side transport, the common
   reason a container needed raw TCP goes away.

---

## 5. Migration strategy

**Base branch — PE decision: merge M18+M19 first.** `m19-sandbox-completion`
*contains* `m18-container-sandboxing` (75 M18 commits + 56 M19 commits), and
its merge base is `master`'s current HEAD, so the merge is a fast-forward.
What it brings that 2.0 keeps: `src/daemon/executor/container.rs` (the
argv builders and decision logic, pure and tested), the root staging helper,
the proxy image/network and both lockfiles, `Request::ContainerStatus` +
the `SANDBOX` status section, the hardening flags, the proxy allowlist and
audit, and `src/cli/commands/sandbox.rs` — 5,356 lines across 18 files, of
which only the wiring in `background/run.rs` / `respawn.rs` / `ghost.rs` is
tmux-bound, and that code is deleted in M26 anyway. Phases 09 (escape hatch)
and 12 (workspace mount) are **re-homed** into M21/M22 where they become an
approval classification and a host profile option; phase 10 (live close)
runs as the M19 close-out on the merged tree. **Done 2026-09-03:** merged
fast-forward, crate bumped to 1.0.0, `v1.0.0` tagged as the last tmux release.
2.0 work proceeds on `master` behind the `[execution] backend` flag; a `v2`
branch is unnecessary while every milestone ships a bootable daemon.

**Strangler, not big bang, within the branch.** The shell engine lands
beside `src/tmux/` behind `[execution] backend = "tmux" | "pty"` so every
milestone below ships a bootable daemon and the 1,4xx-test suite keeps
running against both until parity. The tmux backend is deleted in one phase
at the end, with the `doc_truth`-style tests that pin the tool table and
CLAUDE.md flipping in the same commit.

**Tests.** The M6 isolation harness (throwaway `HOME`, private tmux server)
becomes throwaway `HOME` + a private PTY; most existing tests are unaffected
because they never touched tmux. New pure seams to test without a PTY:
marker parsing, vt100 grid → context rendering, meta index, governor
detectors, briefing rendering, log query filters/caps.

**Executor fit (rexyMCP).** PTY spawning and termios need `unsafe` or a crate
that hides it; with `portable-pty` the executor-authored phases stay
`unsafe`-free, and the two or three phases that touch raw fds are
architect-authored. Every PTY/SSH behaviour cited in a spec is measured on
scrappy first and quoted verbatim ([[spec-facts-must-be-executed]]).

---

## 6. Milestones

Numbered after M19. Each gets a README with exit criteria and a retrospective
per `WORKFLOW.md`; phase counts are estimates from the survey's line counts.

| # | Milestone | Delivers | Phases |
|---|---|---|---|
| **M20** | **Shell engine** | `src/shell/`: `portable-pty` + `vt100` measured and adopted; the **shell-host process** and its socket protocol; `Shell`/`ShellRegistry`/`ShellLog` (asciicast v2 + meta index); marker protocol with real exit codes; interactive-command routing; pause/resume/cancel signalling; `[execution] backend` flag; ANSI annotation and `PaneStatus` re-pointed at the grid. No tool wiring yet. Exit: a daemon with `backend="pty"` runs `run_terminal_command` end to end in chat with the exit code proven by a failing command, **and a shell started before `daemoneye daemon` is restarted is still running and still logging afterwards.** | 10–12 |
| **M21** | **Host model & SSH** | `[hosts]`, `Host::Ssh` shells from local PTYs, ControlMaster, prompt detection (host key, password, passphrase, **remote sudo**) through `CredentialPrompt`, new-host approval class, the re-homed M19 escape-hatch classification, runbook `hosts:`/`ssh_target` mapping, `read_file`/`edit_file` `host` param through the PTY shell with marker-delimited results. Exit: a live remote `sudo` edit on a configured host from chat, evidenced by the shell log with the password absent from it. | 7–9 |
| **M22** | **Tool surface & context** | The 36→42 tool change, `Category`/`mutates_state()` on `TOOLS`, `ToolResult`-as-value, `missing_args_hint`, the `[HOSTS]/[SHELLS]` context blocks, approval panel shows shell+host, sre.toml + seeded memories rewritten (the two tmux knowledge memories retired), `doc_truth` updated. Exit: zero `%`-pane identifiers in any prompt asset or tool description. | 8–10 |
| **M23** | **Client decoupling & shell UX** | Chat runs outside tmux; `/shells`, `/tail`, the `/agents` picker + attached mode with `Ctrl-p`/`Ctrl-c`/`Ctrl-d`, `/logs`, `/log`, `/hosts`, `/shell`; the IPC additions; viewer gains follow mode, marker navigation and replay. Exit: pick a running ghost from `/agents`, attach, pause it, resume it, detach, then `/log` replay of a finished shell — from a plain `alacritty` window with no tmux server running. | 9–11 |
| **M24** | **Run manager** | One `Run` model for chat/ghost/scheduled/delegated; per-run turn log; heartbeat; cooperative cancel + sentinel; `RunResult` + briefing; briefing-seeded resume; seeded tasks from runbook `## Steps`; `/runs`, `/run`, `/stop`; `run_log_*` tools. Ghost mailbox carries a `RunResult`. | 10–12 |
| **M25** | **Governor & guards** | Ten detectors as pure functions, advisory novelty + `NoveltySample`, `daemoneye calibrate-governor` over turn logs; read-before-edit, self-revert refusal, `RefuseInPlaceEdit`, command classifier + blast-radius hint; output filter with shell-log spill; post-write script check. | 8–10 |
| **M26** | **tmux removal & 2.0 close** | Delete `src/tmux/`, `pane_prefs.rs`, `hook.rs`, `cli/notify.rs`, window prefixes, `DE_EXIT` setup snippet, the `[execution]` flag; rewrite `architecture.md`, `PRODUCT_DEFINITION.md` (drop "tmux" from the type line), `REQUIREMENTS.md`, `security.md`, `CLAUDE.md`; `daemoneye doctor`; live-verification sweep per M14 convention; retrospective. Tag `v2.0.0`. | 5–6 |
| **M27** | **Telemetry & dashboard** | `RunRecord` store, `daemoneye scorecard/profile` with `n_runs`, `failure_class` + `is_model_attributable`, `[budget] max_cost_per_day` breaker, `daemoneye top`. | 6–8 |
| **M28** | **MCP surface** | `daemoneye mcp serve` (rmcp, stdio) with the § 2.8 tools; installed smoke test from Claude Code and from rexyMCP; `[mcp.servers]` client side as deferred tool groups. | 6–8 |
| **M29** | **Knowledge & fleet follow-ons** | Shell logs as a knowledge corpus; postmortem + proposed-runbook generation; runbook fan-out over `hosts:` with a cap; `requires_review` gate. | 6–8 |

Ordering rationale: M20–M23 are the substrate and the user-facing
requirement; they can ship as `v2.0.0-alpha` on the flag. M24–M25 are the
rexyMCP ports and are deliberately *after* the tool surface settles so the
governor classifies the final tool set, not the transitional one. M26 is the
point of no return. M27–M29 are additive.

---

## 7. Risks and how each is retired

| Risk | Mitigation |
|---|---|
| Terminal-emulation fidelity (alt-screen apps, resize, wide glyphs) | `vt100` measured on scrappy against `vim`, `less`, `top`, `htop`, `mysql` before M20 phase-01 is drafted; the grid is used for *context*, the byte log is the record of truth, so a rendering gap never loses data |
| Daemon restart kills shells | Shell-host process (§ 2.1 option 2) in M20 or a follow-on; decision by measuring how often ghosts outlive a daemon restart in `events.jsonl` today |
| SSH prompts (host key, 2FA, password) hang a run | PTY makes them visible; prompt patterns route to `CredentialPrompt` for chat and to a parked escape-hatch for ghosts; `watch_shell` timeouts bound the wait |
| Marker forgery by hostile output (a remote prints the nonce) | Nonce is 128-bit random per command and the marker carries `\x1f` control bytes that the masking layer strips from any *displayed* output; a duplicate marker is logged as an anomaly event |
| The sudo credential flow relies on 1.x pane injection | It already works by writing to a PTY through `send_keys`; the 2.0 shell writes to the PTY directly — same panel, fewer moving parts |
| Executor cannot author `unsafe` PTY code | `portable-pty`; raw-fd phases architect-authored |
| Loss of the "just look at the tmux window" habit | `/tail` + `/attach` + `daemoneye top`; and nothing stops a user running `daemoneye chat` inside tmux — it just no longer requires it |
| Scope: nine milestones | M20–M23 alone satisfy the stated 2.0 requirement; the rest is ordered so each stopping point is a shippable daemon |

---

## 8. PE decisions (2026-09-03)

The eight open questions from the first draft, answered:

1. **Base branch** — merge `m19-sandbox-completion` (which contains M18)
   into `master` first; fast-forward. Container sandboxing for agents is in
   scope for 2.0. Phases 09/12 re-homed, 10 is the close-out (§ 5).
2. **Shell persistence** — required. Shells survive a daemon restart via the
   shell-host process; M20 exit criterion (§ 2.1).
3. **Remote transport** — PTY for everything remote, including file ops.
   Interactive remote use (sudo prompts, 2FA, `vim` over ssh) is a first-class
   case, not an edge (§ 2.1, § 2.2).
4. **Log format** — asciicast v2; complexity judged low, size overhead
   accepted (§ 2.1).
5. **Attach UX** — a Claude-Code-style picker (`/agents`, arrows + Enter);
   attached mode intercepts `Ctrl-p` pause/resume, `Ctrl-c` cancel, `Ctrl-d`
   detach (§ 2.6). Pausing parks the owning run; the agent is told.
6. **Governor on chat turns** — advisory only (§ 3).
7. **MCP server priority** — stays at M28.
8. **Naming** — `v1.0.0` tagged for the last tmux release (done). **"Ghost Shell"
   remains the term for an autonomous agent shell DaemonEye runs in the
   background**; a chat turn's shells are just shells.

## 9. Next step

M19's undrafted phases are re-homed (§ 5), so the M19 README should be
closed with that note before M20 opens. Then scope M20 with
`/rexymcp:architect` on `master`: first phase is the
measurement phase — `portable-pty` + `vt100` + a throwaway shell-host
prototype exercised on scrappy against `bash`, `fish`, `ssh -tt`, `sudo`,
`vim`, `less` and a daemon-restart — so every fact the later phase docs cite
has been executed.
