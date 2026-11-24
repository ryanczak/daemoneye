# DaemonEye 2.10.0 audit — triage ledger

The findings of [`daemoneye_audit.md`](daemoneye_audit.md) (2.10.0,
`HEAD e46e946e`), one row each, with the milestone phase that takes it.
Milestones M39–M43 are this audit; the MCP surface moves to M44 and
Knowledge to M45 (PE decision 2026-11-21).

**How rows change.** Scoping assigns a `Target`. When a phase is drafted,
the architect re-verifies the row against the tree (the audit's line
links go stale as earlier phases land) and either keeps it or changes
`Triage` to `reject: <why>` / `split: <phases>`. When the phase is
approved, the review writes `act: M<n> phase-NN`. This ledger stays the
one record of where each finding went.

**Verified** = checked against the 2.10.0 tree at scoping (2026-11-21).
Every other row is the audit's claim, not yet confirmed. The scoping spot
check confirmed all ten rows marked `yes`.

## Safety & security (audit § 1)

| ID | Finding | Where (symbol) | Target | Verified | Triage |
|----|---------|----------------|--------|----------|--------|
| S1 | tmux `display-message` evaluates FORMATS, so a webhook `alert_name`/`summary` containing `#(cmd)` runs a shell command | `webhook/process.rs` · `notify_chat_panes`; `notice_sinks.rs` caller | M39 01 | yes | act: M39 phase-01 (`format_literal_round_trips_through_tmux`) |
| S2a | `open_shell` never checks the sandbox gate for a ghost and registers the shell as `Owner::Chat` | `executor/dispatch_open_shell.rs` | M39 02 | yes (drafting) | act: M39 phase-02 (`ghost_open_shell_is_refused_while_the_sandbox_is_on`, `ghost_open_shell_is_owned_by_the_ghost_and_usable_by_it`, `teardown_closes_every_shell_the_ghost_opened`). Widened at drafting: every `chat_owner(session_id)` arm a ghost reaches (open, list, find, close, context, send_input) gives it `Owner::Chat`, so `teardown_ghost_resources` (`Owner::Ghost`) misses those shells. Ghost foreground is not affected: it is always refused before the owner is built |
| S2b | `send_input` builds `ApprovalRequest { host: None }`, so input into a remote shell is classified as a local command and auto-approved; `sudo` spelled through `keys`/quotes evades `command_has_sudo` | `executor/dispatch_send_input.rs` | M39 03 | yes (drafting) | drafting: the client checks sudo on the Debug-quoted panel, so a `\n` in `text` and single-character `keys` both evade it; remote input takes the per-host grant (PE, 2026-11-22) |
| S2c | `send_input` continues after `ShellId::parse` fails and joins the raw string into `socket_path` | `executor/dispatch_send_input.rs` | M39 03 | yes (drafting) | the raw id reaches `socket_path` in the screen read, before the ownership check and the prompt; the final send is already registry-checked |
| S3a | `install_sudoers` grants NOPASSWD to a user-writable script path with no digest | `scripts.rs` · `install_sudoers` | M39 04 | yes | act: M39 phase-04 (`install_plan_copies_before_the_rule`, `sudoers_rule_file_name_is_sudo_includable`) + 04b (`copy_drift_reports_current_stale_and_orphaned`, doctor `sudo_scripts_check`). Root-owned copy at `/usr/local/libexec/daemoneye/<euid>/<name>`, not `sha256:` (sudo-rs 0.2.13 has no digests; PE 2026-11-23). Live-probed: two defects unit tests missed — umask on implicit parents, dotted rule names skipped by `@includedir`. Pre-M39 dotted rules were inert; users re-run `install-sudoers` |
| S3b | `GhostPolicy::is_safe` approves `sudo /any/path/<approved-basename>` by basename | `daemon/policy.rs` · `is_safe` | M39 04 | yes (drafting) | act: M39 phase-04 — `is_safe` approves local sudo only at the exact root-owned copy path; `resolve_command` maps `sudo <name>` there |
| S3c | `command_has_sudo` misses `s\udo`, `s'u'do`, `"su""do"`, leading redirections, globs, and other elevators (`su`, `doas`, `pkexec`, `run0`, wrappers) | `daemon/utils/sudo.rs` | M39 05 | yes (drafting, 18/18) | act: M39 phase-05 (`sudo_detect_*`). Fail-closed elevation detection: dequoted words, redirections, six elevators, wider wrapper set, unresolvable command words (`$`, glob, brace) count; quoted text fails closed only as a shell `-c`/`eval` operand (PE, 2026-11-23); `$` behind a wrapper fails closed |
| S3d | `wrap_remote` skips wrapping any command starting `ssh `, and interpolates `target` unquoted without `--` | `daemon/policy.rs` · `wrap_remote` | M39 06 | yes (drafting) | act: M39 phase-06 — `wrap_remote` was dead code (no production caller since M27 phase-05; a ghost's `ssh_target` runs in a PTY shell on its resolved `[hosts]` host or is refused), so it was deleted rather than hardened (PE, 2026-11-24). Observation, not fixed: a ghost's own `ssh …` command is not fenced by its runbook's `hosts:` list — egress control, for the sandbox work |
| S4a | IPC `read_line` is unbounded (the 1 MiB cap is checked after the read); approval/credential reads and the shell host have no cap; accept loops are unbounded | `daemon/server/mod.rs`, `shell/host.rs` | M40 01 | | |
| S4b | the shell host's multi-frame loop passes `&line[..end]` instead of `&line[offset..end]`, so pipelined frames re-parse the first | `shell/host.rs` · `serve` loop | M39 10 | yes | |
| S5 | provider API keys are inherited by shell-host, PTY shells and notification command hooks | `shell/pty.rs`, `shell/launch.rs`, `notice_sinks.rs` | M40 02 | | |
| S6 | remote `edit_file` temp file is a predictable `.de_tmp` opened without `O_EXCL`/`O_NOFOLLOW` (root under `sudo = true`) and created 0644 before chmod; local temp also predictable | `file_ops/write.rs`, `file_ops/ops.rs` | M40 03 | | |
| S7 | `is_blocked_secret_path` checks the uncanonicalized path; `resolve_path_for_guard` canonicalizes only `p` or its parent | `file_ops/mod.rs` | M40 04 | | |
| S8 | `memory_namespace`, `session_id` and prompt names are joined into paths unvalidated | `memory.rs`, `daemon/session.rs`, `config/load.rs`, `session_store.rs` | M40 05 | | |
| S9 | `spawn_ghost_shell` with `host: None` skips caller-policy fencing; interactive chat spawns ghosts without approval | `executor/knowledge/ghost.rs` | M40 06 | | |
| S10 | webhook server: non-constant-time Bearer compare, body parsed before auth, per-alert `tokio::spawn` fan-out, rate-limit map never evicts, secret snapshot ignores reload, alert text enters prompts without untrusted-data framing | `webhook/server.rs` | M40 09a, 09b | | |
| S11 | argv flag injection: `HostId::parse` allows a leading `-` and ssh argv lacks `--`; remote `grep -E` without `-e`; `infocmp -x` without `--` | `host.rs`, `file_ops/read.rs`, `tmux/session.rs` | M40 08 | | |
| S12 | `cancel_schedule` / `delete_schedule` have no ghost guard and no approval | `executor/dispatch_schedule.rs` | M40 07 | | |
| S13 | JSON secret rule's `"$1"` replacement is returned from a `replace_all` closure, which does not expand captures — the literal `"$1"` is emitted | `ai/filter.rs` | M40 10 | yes | |
| S14 | `credential_round_trip` copies a `Zeroizing` credential into a plain `Vec<u8>`; `Request`, `ModelEntry`, `WebhookConfig` derive `Debug` over secrets | `executor/shell_exec_auth.rs`, `ipc.rs`, `config/` | M40 10 | | |
| S15 | seven prompt-response handlers ignore `resp_id == prompt_id` | executor prompt round trips | M40 11 | | |
| S16 | readiness `libc::pipe()` without `O_CLOEXEC` | `daemon/ready.rs` | M40 12 | | |
| S17 | `dup2` + unconditional close closes fd 0/1/2 when they were closed at launch | `main.rs`, `daemon/startup.rs`, `utils/log_rotation.rs` | M40 12 | | |
| S18 | fixed 16 KiB `getpwuid_r` buffer, no `ERANGE` retry | `executor/shell_label.rs` | M40 12 | | |

## Functionality & correctness (audit § 2)

| ID | Finding | Where (symbol) | Target | Verified | Triage |
|----|---------|----------------|--------|----------|--------|
| C1 | ghost turns persist `tool_calls` and `tool_results` on one `Message`; every backend's converter takes `tool_results` first and drops the calls, so turn 2 sends orphan results (HTTP 400) | `ghost/turn_helpers.rs` · `persist_ghost_turn_message`; `anthropic.rs`/`openai.rs`/`gemini.rs` converters | M39 07 | yes | act: M39 phase-07 (`ghost_tool_turn_persists_calls_then_results`, `ghost_two_turn_history_converts_for_every_provider`, `ghost_final_turn_persists_one_message`). A ghost tool turn now saves an assistant message with the calls, then a user message with the results; a call without a result gets an explicit one. Found in the reader audit: `write_mailbox_on_exit`'s mailbox result and run summary took `.last()`, which would have been empty after a tool turn; both now take the last assistant message |
| C2 | the ghost chat task logs `chat()` errors and drops the sender; channel EOF reads as a clean empty turn and the run completes | `ghost/turn.rs`, `ghost/turn_helpers.rs` · `collect_ghost_stream_turn` | M39 08 | yes | |
| C3 | the PTY command deadline is checked only on a read timeout, so continuous output never times out and the buffer grows unbounded | `shell/client_read.rs` | M39 09 | yes (structure) | |
| C4 | Anthropic `input_tokens` is already uncached; subtracting cache counts zeroes it | `anthropic.rs` · `parse_anthropic_start_usage` | M39 11 | yes | |
| C5 | `wrap_words` underflows on whitespace-only lines and on a 2-col char at width 1 | `cli/viewer_layout.rs` | M39 12 | | |
| C6 | webhook dedup keys on fingerprint alone, so a `resolved` within 300 s of `firing` is dropped | `webhook/process.rs` | M41 09 | | |
| C7 | circuit breaker records success on HTTP 200 headers, trips globally on 4xx, half-open has no probe gate | `ai/mod.rs` · `CircuitBreaker`, `send_with_retry` | M41 03 | | |
| C8 | consecutive `user` messages (empty cancel, empty reply, redirect, client-gone, ghost note) reach Anthropic uncoalesced; redirect drops earlier results in the batch | `stream/conversation_helpers.rs`, `stream/endings.rs`, `anthropic.rs` | M41 05 | | |
| C9 | `load_session_messages` drains without turn-boundary repair, leaving orphan tool results | `session_store.rs` | M41 06 | | |
| C10 | background compaction: epoch 1 starts at `now`, ignores the session's model window, hardcodes `tail_first_turn = 0` | `context/background.rs` | M41 07 | | |
| C11 | `AsyncStdin::read_line` decodes bytes as Latin-1 | `cli/input/tty.rs` | M41 14 | | |
| C12 | SSE retry: transport errors fall into stall retries, non-retriable errors retry, Gemini malformed calls emit three errors | `ai/backends/mod.rs`, `gemini.rs` | M41 04 | | |
| C13 | DST-skipped midnight resets daily spend | `daemon/spend.rs` · `local_day_bounds` | M41 16 | | |
| C14 | `turn_inbox` missed on cold reload, leaked on early error exits | `server/ask_helpers.rs`, `stream/conversation.rs` | M41 11 | | |
| C15 | chapter block prints epoch sequence ids as turns; unchecked `turn_end - turn_start` | `context/epochs.rs` | M41 07 | | |
| C16 | `recall_context` stemming fallback returns the full message; `build_excerpt` panics on Unicode case folding | `context/recall.rs` | M41 08 | | |
| C17 | OSC 7501 `decode_field` rejects unpadded base64 | `shell/program_status.rs` | M41 14 | | |
| C18 | `mark_done` resurrects a cancelled recurring job; `interval_secs == 0` loops under the write lock | `scheduler.rs` | M41 10 | | |
| C19 | chat sandbox idle reaper TOCTOU and dropped retries | `executor/chat_sandbox.rs` | M41 12 | | |
| C20 | PTY/sandbox shell leaks when `await_ready` / `spawn_command` fails | `executor/shell_exec.rs`, `executor/bg_shell.rs` | M41 12 | | |
| C21 | `save_session` wipes existing `tags` and `parent` | `session_store.rs` | M41 06 | | |
| C22 | `index_memory_file` indexes already-expired memories | `memory/index/mod.rs` | M41 16 | | |
| C23a | `wrap_line` overflows on multi-span lines | `cli/render.rs` | M41 15 | | |
| C23b | `InputLine` wraps by char count, not display width; whitespace desync | `cli/input/editor.rs` | M41 15 | | |
| C23c | unchecked `u16` SGR mouse math; bracketed paste drops `[` | `cli/input/tty.rs` | M41 14 | | |
| C23d | `/clear` and `/new` leave status bar, transcript and `held_presence` stale | `cli/commands/chat_loop.rs` | M41 15 | | |
| C23e | `top_tui_loop` has no panic restore and enables raw mode before the guard | `cli/commands/top_tui_loop.rs` | M41 15 | | |
| C24 | `"error": null` aborts OpenAI/Gemini proxy streams | `openai.rs`, `gemini.rs` | M41 04 | | |
| C25 | `finish_command` drops counters after six-slot eviction | `daemon/stats.rs` | M41 16 | | |
| C26 | `gpt-5`, `gpt-4.1`, `o1`/`o3`/`o4` default to a 32k context window | `config/models.rs` | M41 16 | | |

## Performance & concurrency (audit § 3)

| ID | Finding | Where (symbol) | Target | Verified | Triage |
|----|---------|----------------|--------|----------|--------|
| P1 | `SessionCache::refresh` runs 4+N tmux subprocesses synchronously on a worker every 2 s, each spawning two OS threads with 10 ms sleep polling; `get_pane_remote_host` has no timeout | `tmux/cache.rs`, `process.rs` · `bounded_output`, `daemon/utils/host.rs` | M42 03 | | |
| P2 | SQLite in DELETE journal, `synchronous=FULL`, `busy_timeout=0`; transient connections hit `SQLITE_BUSY`; `open_and_reconcile_if_empty` self-contends and a failed reconcile disables retries for good | `memory/index/mod.rs` | M42 01 | | |
| P3 | `log_event` runs synchronously on workers (double masking + fsync); `append_archive_message` pre-append offset race | `utils/event_log.rs`, `daemon/session.rs` | M42 02 | | |
| P4 | global locks held across disk scans and I/O; `Bus::publish` lock chain opposite to `cleanup_pass`'s order | `server/chat_presence.rs`, `bus/mod.rs`, `spend.rs`, `shell/registry.rs` | M42 04, 05 | | |
| P5 | `ScheduleStore::save` writes one fixed `.tmp` under a read lock, no `sync_all` | `scheduler.rs` | M41 10 | | |
| P6 | `read_line_at_offset` scans from byte 0; `PtyShell::run` rescans the buffer per chunk; `read_shell_log` reads the whole cast before capping; `ops*.rs` `read_to_string` without size cap | `memory/index/mod.rs`, `shell/pty.rs`, `server/shells.rs`, `file_ops/ops*.rs` | M42 06 | | |
| P7 | `scrollback_rows(n)` walks all scrollback under the screen mutex | `shell/screen.rs` | M42 07 | | |
| P8 | `program_notices_loop` runs `sbx exec` every 2 s per idle sandbox shell | `daemon/program_notices.rs`, `shell/sbx_backend.rs` | M42 08 | | |
| P9 | blocking disk/FTS I/O on workers in prompt assembly, memory tools, `handle_status`, `handle_shell_list`, `ask_helpers` (duplicate reads, unused `_chain`) | `daemon/prompt.rs`, `server/` | M42 09 | | |
| P10 | three full `Vec<Message>` clones per turn in `persist_final_turn_history` | `stream/conversation_helpers.rs` | M42 09 | | |
| P11 | unbounded presence mpsc, permanent reaper thread per shell, `Arc<PtyBackend>` cycle, sleep under the shell mutex, dead bus subscribers never pruned | `chat_presence.rs`, `shell/launch.rs`, `shell/backend.rs`, `shell/pty.rs`, `bus/mod.rs` | M42 10 | | |
| P12 | missing indexes, sequential masking regexes, Gemini short-circuit order, `SseBuffer` drain, uncached hostname read | various | M42 11 | | |

## Architecture & coupling (audit § 4)

| ID | Finding | Where | Target | Verified | Triage |
|----|---------|-------|--------|----------|--------|
| A1 | 20 bidirectional module cycles; `daemon` as utility bag; `daemon -> cli`, `config -> cli` inversions | crate-wide | M43 09 | | |
| A2 | ~28 mutable globals bypass `DaemonState` (254 call sites) | `daemon/`, `webhook/` | M43 08 | | |
| A3 | `spawn_webhook_task` supervises a single-use listener, spinning every 30 s if `serve` exits | `daemon/startup_tasks.rs` | M41 09 | | |
| A4a | `max_concurrent_ghosts` TOCTOU across spawn sites | `daemon/ghost.rs` · `check_ghost_capacity` | M41 13 | | |
| A4b | ghost spawning duplicated across five sites | ghost spawners | M43 07 | | |
| A5 | `execute_tool_call<W, R>` coupled to IPC streams; ghosts pass `sink()`/`empty()` | `daemon/executor/` | M43 06 | | |
| A6 | `PendingCall` repeats `id`/`thought_signature` on every variant (plus dead `_credential`); route double-match with `bail!` fallbacks | `ai/types/pending.rs`, `executor/route.rs` | M43 05 | | |
| A7 | IPC protocol debt: unversioned enums, dead/asymmetric variants, ignored `Ask.model`, `handle_resume_run` errors as `SystemMsg` | `ipc.rs`, `server/` | M43 04 | | |
| A8 | documentation drift in `AGENTS.md` and `docs/architecture.md` (socket path, removed globals, `notify`, tmux invariant) | docs | M43 01 | yes (AGENTS.md socket path) | |

## Code quality & testing (audit § 5)

| ID | Finding | Where | Target | Verified | Triage |
|----|---------|-------|--------|----------|--------|
| Q1 | an empty `[models]` table panics `resolve_model` | `config/load.rs` | M39 12 | | |
| Q2 | unchecked UTF-8 byte slicing | `manifest.rs`, `scripts.rs`, `sys_context.rs`, `scheduler.rs`, `executor/schedule.rs` | M39 12 | | |
| Q3a | `let _ = self.save()` drops scheduler persistence errors | `scheduler.rs` | M41 10 | | |
| Q3b | `rename_session` ignores the `meta.toml` write | `session_store.rs` | M41 06 | | |
| Q3c | partial log rotation leaves `daemon.log` missing; non-atomic `pane_prefs` write | `utils/log_rotation.rs`, `pane_prefs.rs` | M41 16 | | |
| Q4 | `lock_invariant.rs` and `tmux_read_only.rs` split on the first `#[cfg(test)]` and miss production code below it | `tests/` | M41 02 | | |
| Q5 | `lock_invariant.rs` misses `.read()`/`.write()`/`try_*` | `tests/lock_invariant.rs` | M41 02 | | |
| Q6 | 22- and 24-parameter status-section functions | `cli/status_sections.rs` | M43 03 | | |
| Q7 | `base64 0.23` duplicates `0.22`; single-use `nix` (three versions in the lock) replaceable by `File::try_lock` | `Cargo.toml`, `daemon/instance.rs` | M43 02 | | |
| Q8 | dead `#[allow(dead_code)] fn search_dir` | `search.rs` | M43 02 | | |
| Q9 | 17 `with_home(&tmp, \|\| {})` no-op tests drop the guard before awaiting (env UB) | `file_ops/read_tests.rs` | M41 01 | | |
| Q10 | `TestHomeGuard` held across `.await` on a multi-thread runtime | `shell/launch.rs` tests | M41 01 | | |
| Q11 | test cleanup permanently removes `$SHELL`, `$TMUX`, `$TMUX_PANE` | `shell_exec_tests.rs`, `session_tests.rs`, `tests/isolation.rs` | M41 01 | | |
| Q12 | `host_sends_lagged_when_subscriber_falls_behind` flakes under load | `shell/host.rs` tests | M39 10 | | |
| Q13 | no direct unit tests on the orchestration modules | `stream_ask.rs`, `conversation.rs`, `ask.rs`, `handlers.rs`, `route.rs`, `tmux/{pane,window}.rs` | — | | reject: a coverage count is not a defect (WORKFLOW § "Coverage claims are inadmissible without mutation proof"); tests arrive with the fixes that touch these modules |
