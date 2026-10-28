# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```sh
cargo build                        # debug build
cargo build --release              # release build (binary at target/release/daemoneye)
cargo clippy --all-targets -- -D warnings  # deny all warnings (CI gate)
cargo test                         # run all tests
cargo test <test_name>             # run a single test by name
cargo test -- --nocapture          # run tests with stdout visible
```

The project compiles cleanly — `cargo clippy --all-targets -- -D warnings` exits zero. Tests pass (unit + integration in `tests/integration.rs`) + 1 ignored.

`tests/hygiene.rs` parses every `.rs` file with `syn` and holds the M31 caps: at most 800 production lines per file (lines outside test-only items), at most 2000 test lines per file (a test file, or a file's inline tests), and at most 150 lines per function, signature to closing brace; `src/ai/types/pending.rs` and `src/ai/tools/args.rs` are exempt from both. Today's offenders are listed in `tests/hygiene_allowlist.txt`, whose counts may never grow; a change that brings an offender within its cap deletes its line, and the gate fails with "remove it" until it does.

## Integration Tests

`tests/integration.rs` covers the persistence and IPC protocol layers without a running daemon or tmux session: IPC round-trips (Ask request, ToolCallResponse, SessionInfo), schedule store persistence, session JSONL write/read, session index persistence, event log format validation, event log append/read, and config parsing (minimal config, ghost config). All tests run in CI alongside the unit test suite.

## Architecture Overview

DaemonEye is a Rust daemon that runs an AI assistant beside your terminal. It forks into the background, binds a Unix domain socket (`~/.daemoneye/var/run/daemoneye.sock`, resolved via `config::default_socket_path()`), and communicates with CLI clients via newline-delimited JSON. Commands the AI runs execute in daemon-owned PTY shells; tmux, when present, is an optional read-only observer (pane summaries for the user's session), never an execution substrate.

### Request/Response lifecycle

1. User runs `daemoneye chat` or `daemoneye ask` — the CLI client connects to the socket (the chat client holds a long-lived presence connection) and sends a `Request::Ask`. If the client re-attached (`chat --resume <id>`) after ≥ 30 s away and new event messages arrived (background completions, webhook alerts, watchdog results), the daemon sends a `Response::SystemMsg` catch-up brief immediately after `Response::SessionInfo` and before the first AI token (N15).
2. The daemon assembles the system prompt + context snapshot — the user's tmux pane summaries when the client runs inside tmux (read-only: capture and list only), otherwise the daemon's own PTY shells — applies the masking filter (`ai/filter.rs`), and streams tokens from the configured LLM.
3. When the AI emits a tool call the daemon sends `Response::ToolCallPrompt` back to the client. The client prompts the user: `[Y]es / [A]pprove session / [N]o / or type a message to redirect`. The client returns `Request::ToolCallResponse`.
   - **Y / A / N**: standard approve/session-approve/deny flow.
   - **Typed message**: `approved: false` with `user_message: Some(text)`. The daemon aborts the entire pending tool chain (omitting it from history), injects the text as a plain user turn, and re-enters the AI loop so the model can course-correct without seeing a synthetic tool error.
4. An approved `run_terminal_command` runs in a PTY shell (`executor/shell_exec.rs`): by default this session's shell; with `wait: false` a dedicated PTY shell (or the session's sbx sandbox when `[sandbox] enabled`) whose outcome arrives as a `[Background Task Completed]` message; with `host?` a shell on the configured `[hosts.<name>]` host, in its sandbox when one is configured there. A call with no PTY route is refused with the reason. Real exit codes come from the PTY marker protocol; output is masked and ANSI-annotated.
5. The daemon sends `Response::ToolResult` with captured output, the LLM continues, and the loop repeats until the LLM produces a final answer.

### Key files

| Path | Role |
|---|---|
| `src/main.rs` | CLI entry point; forks daemon, routes subcommands |
| `src/ipc.rs` | `Request` / `Response` enums — the full wire protocol |
| `src/ipc_types.rs` | The payload structs the wire enums carry (`ContainerInfo`, `PaneInfo`, `ShellInfo`, `GhostConfig`, `SessionSummary`, …) — a child module of `src/ipc.rs`, glob re-exported from it |
| `src/daemon/instance.rs` | `InstanceLock` — flock-based single-instance enforcement + PID payload |
| `src/daemon/startup.rs` | `run_daemon()`, the daemon's startup and accept loop; with `tmux_wanted()` / `startup_session()`, the tmux-session choice |
| `src/daemon/mod.rs` | Window-prefix constants + D6 predicates (`is_daemon_window`, `is_ghost_window`, `is_targetable_pane`) |
| `src/daemon/ready.rs` | Fork readiness handshake — child reports `READY` / `ERR <msg>` to the parent over a pipe |
| `src/daemon/server/` | IPC server, split by concern: `mod.rs` client dispatch (`handle_client`), `ask.rs` the `handle_ask` orchestrator, `handlers.rs` the simple request handlers (`handle_ping`, `handle_shutdown`, `handle_refresh`, `handle_set_model`), `sessions.rs` the named-session handlers (`handle_save_session`, `handle_load_session`, `handle_list_saved_sessions`, `handle_delete_saved_session`, `handle_rename_saved_session`), `shells.rs` the shell IPC relay (`handle_shell_list`, `handle_shell_input`, `handle_shell_resize`), `catchup.rs` the catch-up brief (`build_catchup_brief`, `is_valid_pane_id`) |
| `src/daemon/auto_name.rs` | Session auto-naming (`suggest_session_name`, `diff_sessions_summary`) |
| `src/daemon/prompt.rs` | Prompt assembly via `PromptCtx` (`build_first_turn_prompt`, `build_subsequent_turn_prompt`) |
| `src/daemon/stream.rs` | AI event streaming loop (`run_conversation_loop`); tool execution; response persistence |
| `src/daemon/executor/` | Tool call dispatch; approval gate (`ToolCallOutcome`); background/foreground execution coordination; `ArtifactCtx` for session-origin stamping |
| `src/daemon/executor/sbx.rs` | Docker Sandboxes (sbx), pure: sandbox naming (`de-ghost-*` / `de-chat-*`), `network_plan`, every `sbx` argv builder, the `ls --json` / `policy log --json` parsers, preflight classification, and the in-VM signal script (`VM_SIGNAL_SCRIPT`, `VmOp`) |
| `src/daemon/executor/sbx_lifecycle.rs` | sbx side effects through an injected `SbxRunner`: `preflight`, `create_sandbox`, `teardown_sandbox` (rm, falling back to stop), `sweep_daemon_sandboxes`, `collect_sbx_status`, `read_egress` |
| `src/daemon/executor/ghost_sandbox.rs` | A ghost run's sandbox: `ensure_sandbox` (fail-closed, created once per run), `run_ghost_in_sandbox`, `stage_script`, `forget_and_teardown`, `is_created` |
| `src/daemon/executor/chat_sandbox.rs` | A chat session's sandbox for `wait: false`: `ensure_chat_sandbox`, the idle reaper (`reap_idle_chat_sandboxes`, 600 s grace) |
| `src/shell/sbx_backend.rs` | `SbxBackend`: a shell host whose child is `sbx exec`, routing STOP/CONT/close into the VM and sending INT/TERM as a PTY `^C` |
| `src/daemon/session.rs` | Detects daemon hostname and whether the user's pane is local/SSH/mosh |
| `src/daemon/digest.rs` | Session digest: structured compaction of conversation history at 30 messages; scans events.jsonl + filesystem for artifacts |
| `src/daemon/context/epoch_extract.rs` | `extract_memories_from_epoch()`, best-effort memory extraction from a finished epoch; a child module of `context/epochs.rs`, re-exported from it |
| `src/daemon/ghost.rs` | `GhostManager::start_session()` — autonomous remediation runs headless in the daemon; briefing generation on clean exit |
| `src/daemon/briefing.rs` | G4 briefing generation: `generate_and_save_briefing()`, `read_briefing()`, `clear_briefing()`; AI summarization + masking + file I/O |
| `src/daemon/policy.rs` | `GhostPolicy` — runtime enforcement of `auto_approve_scripts` / `auto_approve_read_only` for ghost shells |
| `src/daemon/utils/` | Daemon helpers, one file per concern: `event_log.rs` `log_event()` + dated-segment readers + `sweep_event_segments()`, `log_rotation.rs` `rotate_log_file()` + `reattach_log_fds()`, `warnings.rs` `retention_warnings()`, `shell.rs` escaping + `is_interactive_command()`/`interactive_destination()`, `sudo.rs` `command_has_sudo()` + fingerprint-prompt detection, `output.rs` `normalize_output()`, `response.rs` IPC response senders + `fire_notification()`, `host.rs` `daemon_hostname()`, `keepalive.rs` the turn-wide keepalive contract (`maybe_keepalive`/`keepalive` helpers). `mod.rs` also holds `sweep_session_archives()`, `sweep_pane_logs()`, `sweep_agent_mailboxes()`, `sweep_shell_logs()` |
| `src/ai/types/` | `pending.rs` the `PendingCall` enum (one variant per AI tool), `events.rs` `AiEvent`, `wire.rs` the provider wire types (`Message`, `ToolCall`, `ToolResult`, `TokenBreakdown`) |
| `src/ai/mod.rs` | `AiClient` trait; `dispatch_tool_event()` |
| `src/ai/tools/` | Tool definitions for all three providers (Anthropic / OpenAI / Gemini): `defs.rs` the flat `TOOLS` table, `schema.rs` `ToolDef`/`ParamDef` + `render_gemini()`, `args.rs` serde defaults for tool arguments, `dispatch.rs` `dispatch_tool_event()` |
| `src/shell/pty.rs` | PTY child + marker protocol: `PtyShell`, `wrap_command` (split `DE_''BEG` literal so the echoed command never matches its own marker), `parse_outcome` (real exit code), `strip_markers` |
| `src/shell/backend.rs` | `PtyBackend` — `ShellBackend` impl tying a PTY to its cast log + screen; `screen_contents()` |
| `src/shell/screen.rs` | `ShellScreen` — the vt100 grid (`feed`, `contents`, `annotated()`, `summary()`), scrollback from `[shells] scrollback_lines` |
| `src/shell/log.rs` | asciicast v2 writer (`CastWriter`) + the `.meta.json` command index (`MetaIndex`) + `read_command_output()` |
| `src/shell/proto.rs` | Host wire frames: `ShellRequest`/`ShellResponse`/`ShellSignal`/`ShellState`, newline-delimited JSON |
| `src/shell/host.rs` / `launch.rs` | The detached `daemoneye shell-host` process: `bind`/`serve` over `var/run/shells/<id>.sock` (peer-uid checked), `spawn_detached`, `await_ready`; per-id `InstanceLock` at `<id>.lock` |
| `src/shell/client.rs` | Daemon-side socket client: `run_command()` (subscribe + input + marker-terminated chunks), `send_signal()`, `status()` |
| `src/shell/registry.rs` | `ShellRegistry` — `ShellId`/`Owner`/`ShellRecord` (id grammar, per-owner caps), `spawn_for`, startup `adopt_at_startup`, `sweep_stale`, `host_is_alive`, `reap_exited` |
| `src/shell/session.rs` | `shell_for_owner()` — reuse the owner's live shell or create one (spawn injected for tests); `login_shell()` |
| `src/daemon/executor/shell_exec.rs` | `run_terminal_command` on the PTY substrate when `[execution] backend = "pty"`: routes through `shell_for_owner`, real exit codes, masked + ANSI-annotated output |
| `src/daemon/executor/shell_exec_auth.rs` | Credentials and connecting for `run_terminal_command`: the sudo retry bookkeeping, the credential prompt round trip, the cold-host connect step (`drive_connect`, `ensure_connected`) and `run_sudo_command`; a child module of `src/daemon/executor/shell_exec.rs`, which re-exports `ensure_connected` |
| `src/daemon/executor/dispatch_schedule.rs`, `dispatch_artifacts.rs`, `dispatch_memory.rs`, `dispatch_shells.rs`, `dispatch_runs.rs`, `dispatch_files.rs`, `dispatch_context.rs`, `dispatch_open_shell.rs`, `dispatch_send_input.rs`, `dispatch_meta.rs` | `execute_tool_call`'s arms by tool family, child modules of `src/daemon/executor/mod.rs`: each `dispatch(call, …)` matches its family's `PendingCall` variants — the schedule tools; scripts, runbooks and agents; memory and recall; the shell readers and observers; the run-log tools; `read_file` and `edit_file`; `get_shell_context`; `open_shell`; `send_input`; `load_tools`, invalid arguments and `spawn_ghost_shell`. |
| `src/daemon/executor/dispatch_foreground.rs` | `run_terminal_command`'s foreground arm in stages: `dispatch` (the ghost and host gates, the approval prompt, the refusal fallback), `run_approved` (the owner and the post-approval host lookup, then the local run), `run_on_named_host` (the permissive lookup, the connect and the run over ssh) and `finish_run` (the result text, stats, `ToolResult` frame and shell log line, shared by both hosts); `ApprovedRun` carries the call's fields between them |
| `src/daemon/executor/dispatch_background.rs` | `run_terminal_command`'s background arm (`wait: false`) in two stages: `dispatch` (the chat gate, the approval prompt, then the ghost route through `foreground::run_background`) and `start_approved` (the chat sandbox or a plain dedicated shell, the dispatch reply and the completion watcher, or the start-error reply) |
| `src/daemon/executor/knowledge/shell_read.rs`, `shell_watch.rs` | Shell tool families split out of `knowledge/shell.rs` (child modules, glob re-exported): `shell_read.rs` the recorded-output readers (`read_shell`, `list_shell_logs`, `read_shell_log`), `shell_watch.rs` `find_in_shells` and the blocking observer `watch_shell` |
| `src/daemon/executor/knowledge/shell_input.rs`, `shell_screen.rs`, `shell_life.rs` | The rest of the shell tool families, child modules of `knowledge/shell.rs` (which keeps `list_shells` and the context blocks) and glob re-exported: `shell_input.rs` `send_input`, `shell_screen.rs` `read_shell_screen` and the screen waits, `shell_life.rs` `open_shell` / `close_shell` / `close_owner_shells` and their test seams |
| `src/daemon/executor/file_ops/` | `edit_file` and `read_file`: `write.rs` dispatches by operation, `ops.rs` runs `edit` and `create`, its child modules `ops_delete.rs` and `ops_copy.rs` run `delete` and `copy` (re-exported from `ops`), `read.rs` the paginated read |
| `src/ai/filter.rs` | Regex-based sensitive-data masking; `init_masking()` at daemon start |
| `src/tmux/mod.rs` | All `tmux` subprocess calls (one function per operation) |
| `src/process.rs` | Process helpers, tmux-agnostic: `annotate_output()`, `off_runtime()`, `TMUX_TIMEOUT`, `bounded_output()` / `bounded_output_with()` — run a `Command` with a timeout and kill its process group on expiry |
| `src/shell/status.rs` | `PaneStatus` classification (M12 D2): `classify()` pure function, `is_shell_prompt()`, `summarize()` — `<status> — <last line>` pane summaries |
| `src/tmux/cache.rs` | Background 2 s poll; `SessionCache`, `PaneState`, `get_labeled_context()` |
| `src/tmux/session.rs` | Session-level tmux helpers: `other_sessions_context()`, `format_other_sessions()`, `client_dimensions()`, `session_environment()`, `list_sessions()`, `session_exists()` |
| `src/util.rs` | `UnpoisonExt` trait — `unwrap_or_log()` extension on `LockResult` that logs ERROR on poison recovery |
| `src/config/` | Config, FHS paths and first-run seeding; the public surface is re-exported from `mod.rs`. `types.rs` `Config` + the remaining per-section structs + `resolve_api_key()`, `sandbox.rs` `[sandbox]` + `[execution]`, `shells.rs` `[shells]` + `[limits]`, `models.rs` `[hosts.*]`, `[models.*]` + pricing, `[ai]` and `PromptDef`, `load.rs` the path constructors (`config_dir()`, `var_log_dir()`, …), `seeds.rs` `SRE_PROMPT_TOML` + asset seeding, `path_audit.rs` the M6 path-audit gate, `lifecycle.rs` the M6 artifact-lifecycle policy table |
| `src/scheduler.rs` | `ScheduleStore` (atomic JSON persistence); `ActionOn` enum (`Alert`/`Script`/`Ghost`); `ScheduleKind` (`Once`/`Every`/`Cron`); `parse_cron()` helper |
| `src/scripts.rs` | Script management in `~/.daemoneye/scripts/` (chmod 700, path-traversal validation); `install_sudoers()` |
| `src/runbook.rs` | TOML runbook loader; `watchdog_system_prompt()` for AI watchdog analysis |
| `src/session_store.rs` | Named session persistence: `save/load/list/delete/rename_session()`; `ArtifactRef`; `backfill_session_origin()`; `build_resumed_banner()` |
| `src/cli/commands/slash_session.rs` | The `/session`, `/runs`, `/run` and `/stop` slash commands (`cmd_session`, `cmd_runs`, `cmd_run`, `cmd_stop`); a child module of `src/cli/commands/slash.rs` |
| `src/cli/commands/chat_loop.rs` | The ratatui chat loop: `run_chat_ratatui` (one chat session, turn by turn: input, slash commands, the ask stream, the viewers) and `read_input_line_inner_ratatui` (one line of input); a child module of `src/cli/commands/chat.rs` |
| `src/cli/commands/stream_ask.rs`, `stream_prompts.rs` | The chat turn's client side, child modules of `src/cli/commands/stream.rs` (the context structs and the stream helpers): `stream_ask.rs` `ask_with_session_ratatui` (re-exported), `stream_prompts.rs` the approval primitives and the ratatui prompt functions |
| `src/cli/commands/top_tui_draw.rs` | Drawing `daemoneye top`: `draw` / `draw_at` (header, four panels or the open detail, footer), the panel windowing (`visible_range`, `runs_window`) and `draw_panel`; a child module of `src/cli/commands/top_tui.rs`, re-exported from it |
| `src/cli/commands/top_load.rs`, `top_text.rs`, `top_detail.rs` | `daemoneye top` split by concern, child modules of `src/cli/commands/top.rs` (the snapshot types, the Activity rows and `run_top`) and glob re-exported from it: `top_load.rs` the loaders and `fingerprint`, `top_text.rs` the panel texts and `plan_layout`, `top_detail.rs` the run detail view |
| `src/cli/viewer_layout.rs` | Transcript layout for the viewer: `layout_blocks` / `layout_blocks_with` (blocks to `ViewRow`s), `wrap_words` (word wrap), `clamp_scroll` and `row_at` (scroll clamping + mouse hit-testing); a child module of `src/cli/viewer.rs`, re-exported from it |
| `src/cli/render_ratatui_draw.rs` | `RatatuiRenderer<B>`'s drawing and committing methods (`draw`, `draw_spinner`, `draw_prompt`, the approval / credential / picker panels, `commit`, `commit_panel`, the repaint history); a child module of `src/cli/render_ratatui.rs` |
| `src/cli/tail_view.rs` | The tail and log viewers: `open_tail` / `open_log` (open a model from the first frame), `run_tail_viewer` / `run_log_viewer` (the alt-screen commands), `tail_loop` (the event loop) and `render_tail` (the grid + status line); a child module of `src/cli/tail.rs`, re-exported from it |
| `src/memory.rs` | Memory module: `MemoryCategory` (note `Incident.dir_name()` is `incidents`, plural, while `canonical_name()` is `incident`), `MemoryInfo`, and CRUD — `add_memory` / `update_memory` / `delete_memory` / `read_memory` / `list_memories` / `list_memories_with_tags`. `memory_dir_for_namespace()` resolves the two-location layout: `memory/<category>/` for the `global` namespace, `agents/<ns>/memory/<category>/` otherwise. Masking (`mask_sensitive`) and the `SESSION_MEMORY_CAP` size cap apply in `load_session_memory_block()`, **not** in the mutators — the mutators do no locking, capping or masking. Frontmatter fields are `tags`, `summary`, `relates_to`, `created`, `updated`, `expires` |
| `src/memory/index.rs` | SQLite FTS5 memory index at `var/index/memory.db`. `open_index()` / `ensure_schema()` create it — a `SCHEMA_VERSION` mismatch drops and recreates, since the index is derived and a rebuild is always safe. `index_memory_file()` / `remove_from_index()` are called **best-effort** from the `src/memory.rs` mutators: an index failure logs a warning and never fails the caller. `reconcile_index()` rebuilds from the files on disk and runs automatically when the index is empty, which is what indexes the memories a fresh install seeds; to force a rebuild when the index is populated but stale, run `daemoneye reindex` (single-transaction, safe while the daemon is running, idempotent). `fts5_search()` returns BM25-ranked `(namespace, key, score)`, best first; `build_match_expr()` quotes each user term and joins with `OR`, because the caller passes a whole user turn and a phrase match would return nothing. The grep scan in `src/search.rs` backs the `search_repository` tool, not memory recall. |
| `src/memory/index/search.rs`, `reconcile.rs` | The index split by concern, both child modules of `src/memory/index/mod.rs` (the schema and the write path) and glob re-exported from it: `search.rs` the BM25 queries (`fts5_search`, `fts5_search_in_category`, `search_turns`, `search_artifacts`, `search_events`, `search_epochs`), `reconcile.rs` the rebuild from disk (`Corpus`, `reconcile_corpus`, `reconcile_index`) |
| `src/memory/review.rs` | Memory review scoring: `effective_confidence()` |
| `src/memory/tags.rs` | G5 SessionTags derivation: tag inference from cwd, command, hostname, recent keywords |
| `src/daemon/memory_prompt.rs` | G5 tiered memory prompt: stable ambient block + dynamic turn-relevant block |
| `src/header.rs` | Inline header parser/renderer for all artifact types; `inject_yaml_session_origin()` / `inject_comment_session_origin()` |
| `src/sys_context.rs` | One-shot host audit (OS, uptime, memory, processes, shell history); `OnceLock` |
| `src/cli/` | Terminal rendering, readline input, session-level approval state, chat/ask commands; `/panes` inspector (`render_pane_inspector`) |
| `src/cli/doctor.rs` | `daemoneye doctor` — one line per readiness check (config, api key, shell, daemon, sandbox, hosts, tmux); exit 1 only on a FAIL; tmux is optional and never fails it |
| `src/agents/mod.rs` | Named agents: `AgentConfig`, CRUD, validation, `apply_agent_to_ghost_config()` |
| `src/agents/policy.rs` | `ToolPolicy` — agent-level allow/deny tool lists, `permits()`, `format_tool_restriction_block()` |
| `src/agents/mailbox.rs` | G5 mailbox: `MailboxResult`, `write_mailbox()`, `read_mailbox()` for agent-to-agent delegation |

### Global statics in daemon

- `DAEMON_START`: `OnceLock<Instant>` — recorded at daemon startup; used by `daemon_uptime_secs()` for `daemoneye status`.

### Session context format

```
[SESSION TOPOLOGY] N windows — name (ID: @K, J panes, active/zoomed), …
[SESSION ENVIRONMENT] KEY=value, …
[CLIENT VIEWPORT] WxH
[ACTIVE PANE %N | idx:K in 'window' | cwd: /path | scrolled N lines up | copy mode]
[BACKGROUND PANE %N (idx:K in 'window') — cmd — /cwd (title) [synchronized] [dead: N] [active Xs ago]]: summary
[VISIBLE PANE %N (idx:K in 'window') — cmd — /cwd (title)]: summary
[SESSION PANE %N (idx:K in 'window') — cmd — /cwd (title)]: summary
[FOREIGN SESSION PANE %N (idx:K in 'window' | session:name) — cmd — /cwd status:Status]
[OTHER SESSIONS] name (N windows, active Xm ago, attached/detached), …
```

`idx:K` is the 0-based window-relative pane index — the number the user sees with `ctrl+a q`. Used by the AI to communicate pane targets in human-readable terms and displayed in the tool-call approval prompt so users can visually confirm the target before approving.

`[OTHER SESSIONS]` — appended by `other_sessions_context()` (`tmux/session.rs`) when two or more tmux sessions exist. Omitted in single-session setups and when there is no terminal context. Generated from `tmux list-sessions`; pure formatting extracted into `format_other_sessions()` for testability (N16).

`[Catch-up]` — a `Response::SystemMsg` sent before the first AI token on the turn after a tmux client re-attaches following ≥ 30 s of detachment. Generated by `build_catchup_brief()` (`daemon/server.rs`) which scans messages added since `messages_at_detach` for event prefixes (`[Background Task Completed`, `[Webhook Alert]`, `[Watchdog]`, `[Watch Pane`, `[Ghost Shell Started]`, `[Ghost Shell Completed]`, `[Ghost Shell Failed]`). `SessionEntry.last_detach` / `messages_at_detach` are set by `NotifyClientDetached`; cleared by `NotifyClientAttached` or after brief generation (N15).

### Adding a new AI tool (checklist)

1. `src/ai/types/pending.rs`: add `PendingCall::ToolName { ... }` variant + `to_tool_call()` arm + `id()` arm + `tool_name()` arm.
2. `src/ai/types/pending.rs`: add a `summary()` arm for the new variant (used for `ToolStarted` display). Add a `should_emit_tool_feedback()` arm: return `true` for silent (non-approval-gated) tools so the executor emits `ToolStarted`/`ToolFinished`; return `false` (via the catch-all `_ => false`) for approval-gated tools that already have richer UI.
3. `src/ai/types/events.rs`: add `AiEvent::ToolName { ... }` variant.
4. `src/ai/tools/defs.rs`: add a `ToolDef` entry to the `TOOLS` slice (all three backends share it via `render_gemini(TOOLS)`). **`deferred_group` is required**: `None` makes the tool core — sent with every request and prose-documented in `sre.toml`; `Some("group")` makes it deferred — omitted from the default render and pulled in by `load_tools`. Default to `None` unless the tool is rarely used and its schema is large. **`category` is also required** — one of `Read | Write | Search | Run | Meta` — and `Category::mutates_state()` reads it to answer "did this call change anything". The field has no default, so the compiler catches an omitted one.
5. `src/ai/tools/dispatch.rs`: add the dispatch arm in `dispatch_tool_event()`.
6. `src/ai/backends/gemini.rs`: no separate entry needed — Gemini tool definitions are auto-generated from `TOOLS` via `render_gemini(TOOLS)`.
7. `src/daemon/stream.rs`: add `AiEvent::ToolName` arm in the streaming match.
8. `src/daemon/executor/mod.rs`: add `PendingCall::ToolName` arm in `execute_tool_call()`. Agent tools (create/read/list/delete agent) dispatch to `executor/knowledge.rs` alongside runbook/memory tools.
9. `src/config/seeds.rs` (`SRE_PROMPT_TOML` / `assets/prompts/sre.toml`): document the new tool.
10. `CLAUDE.md`: add a row to the Current AI tools table below with the right `Loaded` value, **and bump the `N tools: C core + D deferred` line above it**. Both are enforced — `tests/doc_truth.rs` cross-references the table against `TOOLS` and will name the missing tool and the expected counts.
11. `README.md` § "AI tools": add the tool to the **Core** table or to its group's row in the **Deferred** table, and update the three numbers in the prose above them. Enforced by the same test file — the README's tables went three tools stale before this gate existed, which is why it does.
12. If the tool prompts the user before executing (its executor arm sends a `Response::*Prompt` and blocks on the reply), add it to `APPROVAL_GATED_TOOLS` in `src/ai/tools/defs.rs` **and** mark it `**⚠**` in the README — `tests/doc_truth.rs` holds the two in sync. **That entry also changes runtime behaviour:** the same list is what exempts a tool from the per-turn, per-session and per-tool budget caps in `daemon::stream`, and what `LimitsConfig::validate` warns against in `[limits.per_tool]`. Add a tool that does not really prompt and you hand it an uncapped call budget.

### Current AI tools

**43 tools: 29 core + 14 deferred.** `Loaded` mirrors `ToolDef.deferred_group` in
`src/ai/tools/defs.rs` — `core` means `None` (rendered on every request); a group
name means the tool is omitted until `load_tools` pulls the group in. **Note the
asymmetry**: for scripts, runbooks and memory the *write* side is core while the
*read* side is deferred, so a row like "script CRUD" would be wrong.

| Tool | Loaded | Description |
|---|---|---|
| `run_terminal_command` | core | Runs a command in a PTY shell. `wait?` selects whether to wait — omitted or `true` waits; `wait: false` is what `background: true` was and returns immediately with the shell id, follow it with `watch_shell` / `read_shell`. `shell?` targets a specific PTY shell by id or `"new"` (default: this session's lowest-numbered open shell, another session's shell is refused); `host?` runs it on a configured `[hosts.<name>]` host — omitted or `"local"` means the daemon host, a shell id naming a shell on a different host is refused (never relocated); `timeout_secs?` bounds the wait, default 45 s, cap 3600; `interactive?` launches a program that does not exit (a TUI such as `vim`, `top` or `claude`, or a REPL) — the command is typed in, the call waits only for the screen to settle (up to 5 s), and nothing waits for the program (`ssh`/`mosh` launch this way automatically) |
| `edit_file` | core | File operations on the daemon host (or on a configured `[hosts.<name>]` host with `host?` — omitted or `"local"` means the daemon host; the operation runs in a shell there after the same approval prompt; `sudo?` runs an `operation="edit"` as root on that host, with the password asked in the chat and no session approval): `operation="edit"` (atomic string replacement, requires `old_string`/`new_string`), `operation="create"` (new file from `content`), `operation="delete"` (remove file), `operation="copy"` (duplicate `path` to `dest_path`). All require user approval with colored unified diff. Atomic writes via `.de_tmp` → rename. **Blocked from `~/.daemoneye/`**. IPC: `EditFilePrompt` / `EditFileResponse`. |
| `read_file` | core | Paginated file read with optional grep filter; masks sensitive data; `host?` reads on a configured `[hosts.<name>]` host through a shell there — omitted or `"local"` means the daemon host; path `canonicalize()`d to resolve symlinks; **blocked only from `etc/config.toml` and `etc/prompts/sre.toml`** (API credential files) |
| `search_repository` | core | Grep across runbooks / scripts / memory / events |
| `get_shell_context` | core | Fresh terminal snapshot on demand; optional `scope` param: `"shells"` (open shells + default only, no capture), `"session"` (default, user's session), `"all"` (adds foreign-session pane metadata) |
| `list_shells` | core | List the PTY shells the daemon is running: id (`s3`), owner, age, flags (`dedicated` / `exited`) and what held the shell when last observed (`running <cmd>`, `running <cmd> (TUI)`, `paused <cmd>`); optional `scope`: `"mine"` (default, this session's shells) or `"all"` (every live shell). The id is what the shell tools take as their target |
| `read_shell` | core | Read a shell's recorded output — its per-command log, not the live screen. `shell` is the id from `list_shells`; optional `since_command` (command index to start from; an index past the recorded commands is an error), `filter` (regex, invalid = error), `lines` (tail clamp, default 200, max 2000). Output is masked |
| `read_shell_screen` | core | Read a shell's **live screen** as of the call, masked — the viewport, lossy, not the record (`read_shell` is the record). Header names the `Foreground` (`shell prompt` / `running <cmd>` / `running <cmd> (TUI)` / `paused <cmd>`), size, cursor and alt screen, and the call stores it as the record's `last_foreground`. Inverse-video runs show as `[SEL: …]`. Optional `lines` (tail of the visible rows), `scrollback` (rows that scrolled off, in their own section, not counted by `lines`), `pattern` (regex, invalid = error), `annotated` (default false; adds `[OK/WARN/ERROR: …]`), and the wait parameters `wait_for` (regex the rendered screen must show before returning), `settle_ms` (block until two reads this far apart agree — after typing into Claude Code this returns the finished answer; a screen that never settles — `top -d 0.2`, a clock, a spinner, anything redrawing faster than the settle window, while plain `top` redraws every 3 s and can settle between redraws — is a result, not an error) and `timeout_secs` (default 30, cap 300; a wait that runs out shows the screen as it now stands). Not approval-gated; no owner check, as `read_shell` |
| `find_in_shells` | core | One regex search across every in-scope shell's recorded output — answers "which shell printed the error?" in a single call. Returns shell id, 1-indexed line number and the matching line with ±1 line of context; masked, capped at 50 matches across all shells, searches the last 2000 lines of each recording; unreadable shells are skipped and named. `scope: "mine"` (default) or `"all"` |
| `open_shell` | core | Start a new PTY shell — on the daemon host by default, or on a configured host with `host` — and return its id. `rows` / `cols` size its terminal for the program you are about to run (defaults from `[shells]`, clamped to rows 10–200 and cols 40–500); the result names the size it actually opened at. The result also names the reuse caveat: commands reuse this session's lowest-numbered open shell, so the new shell may not be the one the next command lands in |
| `close_shell` | core | End one of this session's PTY shells by id: hangs the shell up (SIGHUP, which also ends a command running in it) and reports whether the host actually went away, refusing another session's or a ghost job's shell |
| `watch_shell` | core | Block on a live PTY shell's new output: with a pattern, until it matches (returns the matching line with context); without one, until the shell's running command finishes — or report nothing-running / paused / exited / gone / timeout |
| `send_input` | core | Approval-gated write of text and named keys into a live PTY shell's input — type a command at a shell's prompt, answer a program's stdin prompt, press `keys: ["Escape"]` / `["C-c"]` / `["BTab"]`, or send a multi-line `text` as one bracketed paste; the panel naming what the shell is doing (a shell at its prompt will run the text as a command). No exit code, no wait, no output: read the effect with `read_shell`, `read_shell_screen` or `watch_shell`. Every byte lands in the shell's cast log as an `"i"` event, which no read tool surfaces. A ghost shell may not use it unless the agent `ToolPolicy` **explicitly** allows it, and a ghost types only into a shell at its prompt unless its runbook sets `allow_tui_input: true` |
| `recall_context` | core | Retrieve archived turns compacted out of live context — by BM25 query (ranked), by turn range, or both. Optional `scope: "all"` searches every session; cross-session hits are labeled. The answer to an `[elided: …]` placeholder or a too-coarse epoch summary. |
| `load_tools` | core | Pull one or more deferred groups into the active tool set for the rest of the session. `groups` is an array of group names. |
| `write_script` | core | Create/update a script in `~/.daemoneye/scripts/` (chmod 700); approval-gated with a diff |
| `delete_script` | core | Delete a script; approval-gated |
| `read_script` | **scripts** | Read a script's content |
| `list_scripts` | **scripts** | List scripts with sizes |
| `write_runbook` | core | Create/update a runbook in `~/.daemoneye/runbooks/`; approval-gated with a diff |
| `delete_runbook` | core | Delete a runbook; approval-gated |
| `read_runbook` | **runbooks** | Read a runbook's full content |
| `list_runbooks` | **runbooks** | List runbooks with their tags |
| `run_log_tail` | **runs** | Read the last N records of one run's turn log (limit clamped inside the reader, default 50, cap 200 — the header says when a request was clamped) |
| `run_log_search` | **runs** | Search one run's turn log for records containing a substring, case-insensitively (same clamp and header) |
| `get_turn` | **runs** | Read every record one turn logged — the one uncapped escape hatch, because the caller has already narrowed to a single turn |
| `add_memory` | core | Store a persistent memory entry under `<category>/<key>.md` |
| `read_memory` | core | Read one entry by key + category |
| `update_memory` | core | Patch individual frontmatter/body fields in place; omitted fields are preserved, `updated` is stamped automatically. Preferred over read+delete+add. |
| `list_memories` | core | List keys, optionally filtered by category |
| `delete_memory` | **memory** | Remove an entry |
| `schedule_command` | core | One-shot, interval, or cron job — command, script, or ghost shell (`ghost_runbook`) |
| `list_schedules` | core | List jobs with status and next fire time |
| `cancel_schedule` | core | Cancel a job by UUID (kept in the store) |
| `delete_schedule` | core | Permanently delete a job by UUID |
| `spawn_ghost_shell` | core | Delegate a task to an autonomous background Ghost Shell that follows a named runbook; `host?` runs the specialist on a configured host or "local", fenced by both the runbook's and your own `hosts:` lists |
| `await_agent_result` | core | Block on a spawned agent's mailbox for the `job_id` returned by `spawn_ghost_shell`, until the result lands or the timeout expires |
| `create_agent` | **agents** | Create/update `~/.daemoneye/agents/<name>/config.toml`; approval-gated |
| `read_agent` | **agents** | Read a named agent's full config |
| `list_agents` | **agents** | List agents with descriptions and models |
| `delete_agent` | **agents** | Delete an agent; approval-gated, warns if runbooks reference it |
| `list_shell_logs` | **history** | List the PTY shell recordings on disk — live and finished sessions alike, newest first. Each row carries the recording's **log id** (`s3-1788470000`, shell id + start timestamp), its age, its command count and the first command. Recordings without a readable index show `commands: unknown`, never 0. Optional `since` (unix timestamp, keeps recordings at or after it) and `limit` (default 50, cap 500 — the result says when clamped) |
| `read_shell_log` | **history** | Read one shell **recording** by its log id — the id `list_shell_logs` prints, not the bare shell id (which is reused across daemon restarts and refused). Reads finished sessions as well as live ones. `command_index` reads one command (with its **exit code** in the header; an index past the highest recorded is an error naming the highest), `filter` is a regex (invalid = error), `lines` clamps the tail (default 200, max 2000). Output is masked |

## Important Invariants

- Every `events.jsonl` record carries `ts`, `event`, and `pid`; `log_event` stamps
  `pid` itself, so call sites must not pass one. Key **order** in the serialized
  line is `serde_json`'s (alphabetical — this crate does not enable
  `preserve_order`), so never rely on field position when parsing a record.
- `main()` is synchronous so `libc::fork()` can be called before the tokio runtime starts. Never move the fork inside an async context.
- Exactly one daemon may run per `$HOME`, enforced by an exclusive `flock` on
  `~/.daemoneye/var/run/daemoneye.pid` acquired in `run_daemon` before any
  startup side effect (`src/daemon/instance.rs`). The kernel releases it on
  process death, so there is no stale-lock recovery path. The PID written into
  the file is diagnostic payload only — never branch on it. Holding the lock is
  what authorizes unlinking a socket at `default_socket_path()`.
- All mutex lock sites use `.unwrap_or_log()` (the `UnpoisonExt` trait from `src/util.rs`) to recover from poisoned locks — do not change these to `.unwrap()`. The trait logs an ERROR before returning the inner value so poison events are visible in `daemon.log`.
- Legacy `de-*` tmux windows (e.g. from a daemon older than 2.0) are still
  filtered out of context by `is_daemon_window` (`src/daemon/mod.rs`), and
  `daemoneye schedule windows` lists the leftovers. The daemon creates no
  windows of its own.
- Liveness probes (`daemon_liveness()`) are reports, never authorizations. No
  code may unlink a socket or remove a file based on a `DaemonLiveness` variant
  — instance ownership is decided only by the `InstanceLock` flock.
- The webhook listener binds eagerly in `run_daemon` and a bind failure is fatal.
  It is a duplicate-instance signal, not a transient condition to retry.
- `daemoneye daemon` (without `--console`) does not report success until the
  forked child has bound its socket. The parent relays the child's outcome over
  the readiness pipe (`src/daemon/ready.rs`) and exits non-zero if the child
  failed or died. The parent must drop its copy of the write end before reading,
  or a child that dies silently hangs it.

### Ghost Shell conventions

- **Detection signal**: `watchdog_system_prompt()` (`runbook.rs`) asks the watchdog model to emit `GHOST_TRIGGER: YES` or `GHOST_TRIGGER: NO` as the final line of its response. `parse_ghost_trigger()` in `webhook/process.rs` parses this (case-insensitive, last matching line wins) with fallback to legacy `ALERT` keyword check. `evaluate_watchdog_response()` (same file) is the shared helper used by both webhooks and scheduled jobs.
- **Turn loop**: `trigger_ghost_turn()` in `ghost.rs` runs the ghost AI loop. Each iteration creates a **fresh** `(ai_tx, ai_rx)` channel — the sender is moved (not cloned) into the spawned task so the channel closes when the task exits and `recv()` unblocks. A `timeout_at` guard prevents hung turns.
- **Turn budget**: `GhostConfig.max_ghost_turns` (runbook frontmatter `max_ghost_turns: N`; 0 = use daemon default of 20). Enforced in `trigger_ghost_turn`; a warning is logged if the limit is hit.
- **Policy enforcement**: `GhostPolicy` (from `GhostConfig`) is passed through `execute_tool_call()`. Ghost shells without a policy return an error rather than proceeding. `ToolPolicy` (agent-level) is also enforced — both must pass independently.
- **Concurrency cap**: `check_ghost_capacity()` (`daemon/ghost.rs`) returns false when `stats::get_ghosts_active() >= config.ghost.max_concurrent_ghosts`. Default cap is 3; set `max_concurrent_ghosts = 0` in `[ghost]` config to disable. Checked before spawning from both webhooks and scheduled jobs.
- **Lifecycle events**: `inject_ghost_event()` in `webhook/process.rs` injects `[Ghost Shell Started]` / `[Ghost Shell Completed]` / `[Ghost Shell Failed]` / `[Ghost Shell Skipped]` into all active sessions so they appear in catch-up briefs.
- **Session validation**: `session_exists()` (`tmux/session.rs`, wraps `tmux has-session -t`) is called before the AI loop to guard against stale session names.
- **Scheduled ghost jobs**: `ActionOn::Ghost { runbook }` in `scheduler.rs` routes scheduled jobs through `GhostManager::start_session()` + `trigger_ghost_turn()`. The `schedule_command` AI tool accepts a `ghost_runbook` param to schedule these jobs. Old `ActionOn::Command` entries in `schedules.json` are still loaded (backwards-compat) but deprecated.
- **Sudoers installation**: `daemoneye install-sudoers <script-name>` writes a NOPASSWD rule to `/etc/sudoers.d/daemoneye-<name>` allowing the current user to run the script without a password. Required for ghost shells and scheduled jobs that need sudo access to pre-vetted scripts.
- **GhostConfig fields**: `agent: Option<String>` (agent name), `tool_policy: Option<ToolPolicy>` (agent-level tool allow/deny), `spawn_depth: u8` (delegation depth, default 0), `parent_job_id: Option<String>` (parent ghost's job ID), `memory_namespace: Option<String>` (agent memory namespace).
- **Delegation depth**: `spawn_depth` starts at 0 for top-level ghosts. Each `spawn_ghost_shell` increments by 1. Depth 2+ is rejected with an error. Coordinator (depth 0) → specialist (depth 1) is the only allowed pattern.
- **Mailbox**: When a ghost shell exits, its final response is written to `~/.daemoneye/agents/<agent>/mailbox/<job_id>.json` via `write_mailbox_on_exit()`. The coordinator reads results via `await_agent_result(job_id, agent_name)`.

### Named session conventions

- **Storage**: `~/.daemoneye/var/sessions/<name>/meta.toml` + `messages.jsonl`; index at `var/sessions/index.json`. Distinct from `var/log/sessions/<id>.jsonl` (ephemeral per-session JSONL logs).
- **`SessionEntry` fields**: `saved_name: Option<String>` (None = unnamed), `dirty: bool` (true after any message write-back; false after save/load), `artifacts_created: Vec<ArtifactRef>`, `auto_name_suggested: bool`.
- **Auto-naming**: After `[sessions] auto_name_turn_threshold` turns (default 10) in an unnamed session, the daemon fires a one-shot `use_tools=false` LLM call and emits `Response::SystemMsg` suggesting a name. Guarded by `auto_name_suggested` flag so it fires exactly once. Disabled if `auto_name_enabled = false` or `auto_name_turn_threshold = 0`.
- **`session_origin` stamping**: When an artifact (memory, runbook, script) is created inside a named session (`saved_name.is_some()`), `ArtifactCtx` in `executor/knowledge.rs` injects `session_origin: "<name>"` into the file's frontmatter/comment-header via `header::inject_yaml_session_origin()` / `header::inject_comment_session_origin()` before the write. The artifact name is also pushed to `SessionEntry.artifacts_created`.
- **Retroactive backfill**: When an unnamed session is saved for the first time, `session_store::backfill_session_origin(&artifacts, name)` walks `artifacts_created` and stamps `session_origin` on each. Called from the `SaveSession` handler in `server.rs` when `current_saved_name` was `None`.
- **Ghost sessions are excluded**: `ArtifactCtx.is_ghost` guards `track_artifact()` — ghost sessions never write to `artifacts_created` or stamp `session_origin`.
- **Import**: `daemoneye session import <id> --name <name>` reads an orphaned `var/log/sessions/<id>.jsonl` and saves it to the named session store without a running daemon.

## Sandbox (Docker Sandboxes / sbx)

`[sandbox] enabled = true` (default off) runs agent work in **Docker
Sandboxes** microVMs via the `sbx` CLI. `runtime` is ignored — an enabled
sandbox is always sbx — and docker-only keys warn at load.

- **Fails closed.** Preflight (`sbx version`, `sbx daemon status`) gates
  every sandboxed command; a failure refuses it with a `sandbox
  unavailable:` reason and it never runs on the host.
- **Ghost runs** get their own sandbox `de-ghost-<id>` (profile = the
  ghost's agent, mountless), created on the first command, torn down on
  every exit path. Commands run in a PTY shell whose child is `sbx exec`.
- **Chat `wait: false`** runs in a per-session sandbox `de-chat-<id>`; the
  dedicated shell closes after its completion message, and the hourly
  reaper removes a sandbox no shell uses once it has idled 600 s.
- **Scripts** are staged with `sbx cp` into `/de/scripts/` on every
  invocation; the scripts directory is never mounted.
- **Network:** `network = "none"` → `--deny-network '**'`; `"proxy"`
  inherits the global sbx policy, `proxy_allow` adds, `proxy_deny` denies.
- **Egress audit:** `sbx policy log --json` is read before teardown — a
  turn-log note plus a `sandbox_egress` event.
- **Signals:** pause/resume act inside the VM; cancel is a PTY `^C`.
- **Startup** adopts sandbox shells (sidecar) and sweeps only `de-*`
  sandboxes no live record owns.
- **Not sandboxed:** foreground commands, `[hosts.*]` commands, and
  broker-native tools. **Refused:** an unresolved ghost `ssh_target` — a
  runbook target that names no configured host — in every mode.

@REXYMCP.md
