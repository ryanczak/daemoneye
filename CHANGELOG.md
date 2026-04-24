# Changelog

All notable changes to DaemonEye are documented here.

## [Unreleased]

### Added
- **Named session persistence** — conversation history can now be saved, loaded, and resumed across daemon restarts
  - `/session save [name]` — saves current conversation to `~/.daemoneye/var/sessions/<name>/`
  - `/session tag [name]` — alias for `/session save`
  - `/session load <name>` — resumes a saved session (replaces current history)
  - `/session list` — lists all saved sessions with turn counts and descriptions
  - `/session rename <old> <new>` — renames a saved session
  - `/session delete <name>` — removes a saved session
  - `/session diff [name]` — shows a summary of what changed since last save
  - `daemoneye session import <id> --name <name>` — imports an orphaned ephemeral session log (no daemon required)
  - **Auto-naming**: after `auto_name_turn_threshold` turns (default 10) the AI suggests a short name for the session
  - **`session_origin` tagging**: runbooks, scripts, and memories created during a named session are tagged with `session_origin: "<name>"` in their frontmatter; retroactively backfilled on first save for artifacts created before the session was named
  - New `[sessions]` `config.toml` section: `auto_name_enabled`, `auto_name_turn_threshold`, `load_recent_turns`
- **Configurable tool-call limits** — all previously hardcoded limits are now tunable via `[limits]` in `~/.daemoneye/etc/config.toml`. Defaults are identical to the old constants so no config changes are required for existing users.
  - `per_tool_batch` (default `100`) — max consecutive calls of one non-approval tool per AI turn
  - `total_tool_calls_per_turn` (default `0` = unlimited) — hard cap across all non-approval tools per turn
  - `tool_result_chars` (default `16000`) — max chars of output fed to the AI per tool result
  - `max_history` (default `80`) — message-history ceiling before digest compaction fires
  - `max_turns` (default `0` = unlimited) — max AI turns per interactive chat session
  - `max_tool_calls_per_session` (default `0` = unlimited) — cumulative non-approval tool calls per session
  - `[limits.per_tool]` — per-tool overrides of `per_tool_batch`
  - Setting any value to `0` removes that limit entirely
- **`/limits` slash command** — shows active limits and live session counters (turn count, session tool calls used, history length) without leaving the chat pane
- **`/limits reset`** — zeroes the per-session tool call counter without ending the session
- **`[LIMITS]` in `daemoneye status`** — limits summary now appears alongside uptime, session, and model info
- **Config validation** at daemon startup warns when an approval-gated tool appears in `[limits.per_tool]` (the entry has no effect) or when `max_history = 0` and `digest.narrative_enabled = false` (potential unbounded context growth)

## [0.9.1]

### Added
- **Structured memory frontmatter** — memory entries now support `summary`, `relates_to`, `created`, `updated`, and `expires` fields alongside the existing `tags` field; `build_frontmatter()` serialises all fields consistently
- **`update_memory` AI tool** — partial field updates without a full read+rewrite cycle; only the supplied fields are changed, all others are preserved; `updated` timestamp is set automatically; `created` timestamp is set once on first write and never overwritten
- **`list_memories` output** now shows `[knowledge] key — summary` when a summary is available
- **`## Available Knowledge` manifest** entries now rendered as `key — summary [tags]` / `key — summary` / `key [tags]` / `key` depending on available metadata
- **Contextual auto-search** (`auto_search_context`) now matches on summary words (≥4 chars) in addition to names and tags, and follows `relates_to` links from matched knowledge memories to include related entries up to the 3-item cap — no direct keyword hit required for related entries to surface
- **Related knowledge hints** (`related_knowledge_hints`) now match on memory summary words in addition to names and tags
- SRE prompt updated with **Memory Frontmatter** guidance: tag synonym examples (`[postgres, postgresql, pg, database]`), summary writing conventions, `relates_to` cross-linking, `expires` for time-bounded facts, and `update_memory` call patterns

---

## [0.9.0]

### Added
- **Ghost Shell architecture convergence** — unified ghost shell execution across webhooks and the scheduler
- `ActionOn::Ghost { runbook }` — scheduled jobs can now invoke a ghost shell instead of a raw command
- `spawn_ghost_shell` AI tool — the foreground AI can delegate a task to an autonomous background ghost
- `schedule_command` gains `ghost_runbook` and `cron` parameters
- `ScheduleKind::Cron` — cron expression scheduling with 5→6 field normalisation via `parse_cron()`
- `daemoneye install-sudoers <script>` — writes a NOPASSWD sudoers rule for pre-vetted scripts
- Concurrency cap: `ghost.max_concurrent_ghosts` in `config.toml` (default 3; 0 = disabled)
- Per-runbook `max_ghost_turns` frontmatter field; daemon-level `ghost.max_ghost_turns` ceiling
- `evaluate_watchdog_response()` shared helper used by both webhooks and scheduled watchdog jobs
- Ghost lifecycle events (`[Ghost Shell Started]`, `[Ghost Shell Completed]`, `[Ghost Shell Failed]`, `[Ghost Shell Skipped]`) injected into all active sessions and written to `events.jsonl`
- `daemoneye status` shows active/launched/completed/failed ghost shell counts and per-type redaction counters

### Fixed
- Ghost shell scheduling deadlock
- `run_with_sudo` policy — sudo is only prepended for scripts explicitly listed in `auto_approve_scripts`
- Ghost script paths resolved to absolute `~/.daemoneye/scripts/` before execution
- Watchdog analysis returning an empty response when the model emits a tool call instead of text
- Ghost session role bug and channel lifecycle issues

### Changed
- `server.rs` split into focused sub-modules: `ghost.rs`, `scheduled.rs`
- `executor.rs` split into focused sub-modules

---

## [0.8.0]

### Added
- **`daemoneye status`** — uptime, PID, provider/model, session count, schedule count, circuit breaker state, and redaction counters (`F1`)
- **Circuit breaker** on AI API calls — opens after 5 consecutive failures, enters half-open after a 60 s cooldown (`F5`)
- **Supervised tasks** — `supervise()` wrapper with exponential backoff (1→30 s cap, resets after 60 s stable) restarts the cache poller, scheduler, session cleanup, and webhook server on crash (`A1`)
- **Catch-up brief** (`N15`) — on re-attach after ≥ 30 s away, the daemon sends a `[Catch-up]` system message summarising background completions, webhook alerts, watchdog results, watch-pane outcomes, and ghost shell events that occurred during detachment
- **Cross-session context** (`N16`) — when ≥ 2 tmux sessions exist, an `[OTHER SESSIONS]` block is appended to the AI context listing each other session's window count, last-activity age, and attached/detached status
- `client-attached` / `client-detached` global tmux hooks — track detach time and history watermark for catch-up brief generation
- `after-new-session` global hook (`N14`) — automatically installs per-session monitoring hooks in any new tmux session without manual reconfiguration
- `[CLIENT VIEWPORT] WxH` context block derived from `client-resized` per-session hook (`N7/N8`)
- `monitor-silence 2` secondary completion signal for local-pane foreground commands (`N9`)
- `retry_in_pane` parameter on `run_terminal_command` (background mode) — re-runs a command in an existing background window via `respawn-pane -k` (`N11`)
- Pane start command (`#{pane_start_command}`) shown in context when it differs from the current command (`N5`)
- `pane_last_activity` annotation on background and session panes: `[active Xs ago]`, `[idle Nm]`, `[idle NhNm]` (`N4`)

### Fixed
- Startup errors that prevented the daemon from initialising are now logged before exit
- Pipe-pane debug message appearing on every Ask turn

---

## [0.7.0]

### Added
- **Dynamic session adoption** — daemon no longer creates a fallback `daemoneye` tmux session; it joins the user's session when `daemoneye chat` connects
- **Pane preference persistence** — target pane resolved client-side and saved to `~/.daemoneye/pane_prefs.json` (keyed by tmux session); eliminates mid-conversation pane-picker prompts
- `target_pane` resolved before connecting and set as `default_target_pane` in `SessionEntry`
- `pane-focus-in` per-session hook (`N1`) — active-pane cache updated instantly on focus change
- `session-window-changed` per-session hook (`N2`) — window topology refreshed immediately on window switch
- `load-buffer` / `save-buffer` approach for local-pane `read_file` — no scrollback cap (`N12`)
- `pipe-pane` log capture (`R1`) — gives the AI access to full output history beyond the tmux scrollback buffer; ANSI colours converted to `[ERROR:]` / `[WARN:]` / `[OK:]` semantic markers
- Copy-mode detection: `| copy mode` shown in `[ACTIVE PANE]` header (`R4`)
- Scroll-position awareness: `| scrolled N lines up` shown in `[ACTIVE PANE]` header (`R3`)
- Zoom guard in `open_chat_pane`: uses `new-window` instead of `split-window -h` when the pane is zoomed (`R7`)
- Synchronized-pane guard: `send_keys` is rejected for panes marked `[synchronized]` (`R6`)
- Pane context reclassified as `[VISIBLE PANE]`, `[BACKGROUND PANE]`, or `[SESSION PANE]`
- `[SESSION TOPOLOGY]` context block listing all windows with pane count and active/zoomed state
- `[SESSION ENVIRONMENT]` context block from a curated allowlist of tmux environment variables
- Window-relative pane index `idx:N in 'window'` shown in all context blocks and the tool-call approval prompt

### Fixed
- Target pane visual highlight (`select-pane -P bg=colour17`) is now applied from `send_keys` until `capture_pane` and removed on denial or after capture; focus is immediately returned to the chat pane after each style change

---

## [0.6.0]

### Added
- **Webhook alert ingestion** — HTTP endpoint on port 9393 (default) accepts alerts from Prometheus Alertmanager, Grafana, and generic JSON sources
- Alert deduplication by fingerprint, sensitive-data masking, and injection into all active AI sessions
- `tmux display-message` overlay shown in all chat panes on alert receipt
- Matching runbook triggers automatic watchdog AI analysis; `GHOST_TRIGGER: YES/NO` on the final line of the watchdog response drives autonomous ghost shell spawning
- Bearer token authentication for the webhook endpoint (optional; defaults to localhost-only binding)
- `GET /health` liveness probe endpoint
- **Ghost Shell (autonomous remediation)** — unattended AI agent spawned in a dedicated `de-incident-*` tmux window when a watchdog analysis returns `GHOST_TRIGGER: YES`
- `GhostManager::start_session()` allocates `de-gs-bg-*` / `de-gs-sj-*` / `de-gs-ir-*` tmux windows
- `GhostPolicy` — per-runbook tool approval rules enforced in `execute_tool_call()`; sudo restricted to scripts in `auto_approve_scripts`
- Runbook `ghost_config` frontmatter: `enabled`, `auto_approve_read_only`, `auto_approve_scripts`, `run_with_sudo`, `ssh_target`
- Ghost shell turn loop with fresh `(ai_tx, ai_rx)` channel per iteration and `timeout_at` guard
- Rate limiting on webhook endpoint

### Fixed
- Background window GC: errors during window kill are logged and surfaced via `SystemMsg`
- Hook registration failures at daemon startup are now logged accurately

---

## [0.5.0]

### Added
- **Knowledge system** — three-tier AI-accessible persistence:
  - *Runbooks* (`~/.daemoneye/runbooks/`) — markdown with TOML frontmatter; `write_runbook` / `read_runbook` / `delete_runbook` / `list_runbooks` tools
  - *Memory* (`~/.daemoneye/memory/{session,knowledge,incidents}/`) — `add_memory` / `read_memory` / `delete_memory` / `list_memories` tools; session memories auto-injected into first AI turn
  - *Search* — `search_repository` tool for keyword grep across runbooks, scripts, memory, and `events.jsonl`
- `read_file` AI tool — paginated daemon-host file read with optional grep filter; sensitive-data masking; path canonicalised; blocked from `~/.daemoneye/`
- `edit_file` AI tool — atomic string replacement; requires user approval; tmp file at `<canonical>.de_tmp`; blocked from `~/.daemoneye/`
- Pattern matching added to `watch_pane`
- `UnpoisonExt` trait (`src/util.rs`) — `unwrap_or_log()` on `LockResult` logs ERROR on poison recovery instead of panicking
- AI tool definitions unified into a single `TOOLS` slice shared by all three backends; Gemini definitions auto-generated via `render_gemini(TOOLS)`

### Fixed
- Memory category validation; empty values rejected in executor
- Masking filter applied to `read_memory` results
- Session memory loading switched to newest-first to prioritise recently active entries
- Interactive foreground commands (`ssh`, `mosh`, `telnet`, `screen`) now return immediately once the remote prompt appears, with `[Interactive session started]` result

---

## [0.4.0]

### Added
- **Background window persistence** — completed background windows persist for the session (cap of 5; oldest evicted); AI can run follow-up commands or `watch_pane` against the same shell via the returned `pane_id`
- `close_background_window` — AI-callable; windows still open 15 minutes after completion are auto-GC'd
- Full pane scrollback archived to `~/.daemoneye/var/log/panes/` without truncation
- **User message redirect at approval prompt** — typing any text instead of Y/N/A aborts the pending tool chain, injects the text as a plain user turn, and re-enters the AI loop for course-correction
- `ToolCallOutcome` enum in `executor.rs` — `Result` / `UserMessage` / `SpawnGhostSession`
- Token budget indicator on chat status bar: `turn N · Xk / Yk tokens · Z% remaining`; colour-coded yellow past 50 %, bold red past 75 %
- `Ctrl+C` once interrupts the running agent; twice within 60 s closes the chat
- Per-type redaction counters tracked in `ai/filter.rs`
- SSH secret patterns added to the masking filter

### Fixed
- `FG_HOOK_COUNTER` provides unique `alert-activity[N]` hook slots for concurrent foreground watchers
- `BUFFER_COUNTER` provides unique tmux buffer names (`de-rb-N`) for concurrent `read_file` calls

---

## [0.3.0]

### Added
- **Multi-provider support** — Anthropic Claude, OpenAI, Google Gemini, Ollama, LM Studio
- **`watch_pane` AI tool** — blocks until a regex pattern matches pane output, the watched command exits, or a timeout fires; hook-based SIGUSR1 notification
- **`list_panes` AI tool** — enumerate all panes in the session with ID, window-relative index, window name, command, cwd, and title
- `get_terminal_context` AI tool — fresh on-demand tmux snapshot without a full AI turn
- `schedule_command`, `list_schedules`, `cancel_schedule`, `delete_schedule` AI tools
- `write_script`, `read_script`, `list_scripts`, `delete_script` AI tools; scripts stored at `~/.daemoneye/scripts/` with mode 0700
- `daemoneye status` subcommand
- `daemoneye setup` prints systemd service file and recommended tmux keybinding
- Hook-based foreground completion via `pane-title-changed` (replaces 20 ms poll)
- `DAEMON_START` global for uptime reporting
- Multi-line chat input — input box grows dynamically up to 5 rows; collapses on submit
- `user@host` shown in chat input border
- Input history navigation (↑/↓), cursor movement (←/→, Home/End, Ctrl+A/E), kill shortcuts (Ctrl+K/U)

### Fixed
- Gemini malformed tool-call recovery — robust regex handles any argument order and both quote styles
- Non-blocking flag on pane picker input cleared during `sync_read_line()`
- Pane titles escaped to prevent shell injection

### Changed
- Codebase modularised into `src/daemon/`, `src/ai/`, `src/cli/`, `src/tmux/`

---

## [0.2.0]

### Added
- **Scheduler** — `ScheduleStore` with `Once`, `Every`, and per-minute tick; jobs run in dedicated `de-sj-*` tmux windows left in place on failure
- **Watchdog** — AI-powered runbook analysis triggered by scheduled jobs; configurable notification hook (`[notifications] on_alert`)
- **Scripts directory** — `~/.daemoneye/scripts/`; script writes and deletes are approval-gated
- Background execution window prefix constants (`de-bg-*`, `de-sj-*`)
- `remain-on-exit on` for background windows — output preserved for inspection after failure
- Structured event log (`events.jsonl`) — every executed command, AI turn, and lifecycle event appended as a single JSON object
- `pane-died` global hook — notifies daemon when a background pane exits; triggers output capture and `[Background Task Completed]` history injection
- Sudo password prompt in the chat interface (echo disabled) for background commands requiring `sudo`
- `daemoneye ask <query>` single-turn subcommand
- `daemoneye logs` — streams `daemon.log` to the terminal

### Fixed
- Background window creation: use `session:` target to avoid index 0 collision
- Gemini Thought round-trip preserved across streaming

---

## [0.1.0]

Initial release.

### Added
- Background daemon with `fork(2)` / `setsid` lifecycle; PID file at `~/.daemoneye/var/run/daemoneye.pid`
- Unix domain socket IPC at `~/.daemoneye/var/run/daemoneye.sock`; newline-delimited JSON wire protocol
- `daemoneye chat` — interactive multi-turn chat session with full conversation history
- `daemoneye daemon` — start the daemon; `--console` logs to stdout; `--log-file` overrides log path
- AI streaming via server-sent events from Anthropic Claude
- `run_terminal_command` AI tool — foreground (injected into user pane via `send-keys`) and background (dedicated `de-bg-*` tmux window) execution modes
- tmux `capture-pane` context snapshot on first turn: active pane contents, non-active pane summaries, session environment
- Sensitive-data masking (`ai/filter.rs`) — AWS keys, PEM keys, GCP service accounts, JWT tokens, GitHub PATs, database URLs, credit card numbers, SSNs; user-defined patterns via config
- API key resolution from environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`)
- `~/.daemoneye/etc/config.toml` configuration with `[ai]`, `[notifications]`, `[webhook]`, and `[ghost]` sections
- `daemoneye daemon --console` for interactive troubleshooting
