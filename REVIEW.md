# DaemonEye Codebase Review

Generated: 2026-03-13

This document captures findings from a comprehensive security, architecture, and functional review of the DaemonEye codebase. Issues are grouped by category and ordered by severity within each group.

---

## Security Issues

### CRITICAL

#### S1. Unauthenticated Unix Socket IPC ✓ Implemented
- **Files**: `src/daemon/mod.rs` (socket bind), `src/daemon/server.rs` (accept loop)
- **Issue**: The Unix domain socket at `/tmp/daemoneye.sock` accepts connections from any local process. Any user on the host can execute commands, read files, write scripts, and access memory via the AI.
- **Fix applied**: Socket moved to `~/.daemoneye/daemoneye.sock` via `config::default_socket_path()`. The user's home directory is not world-writable, so other local users cannot create files there or connect to the socket. Note: same-user process isolation still requires authentication (S1 partially addressed — cross-user threat eliminated; same-user threat remains).

#### S2. Unauthenticated Webhook Endpoint — Won't Fix (by design)
- **File**: `src/webhook.rs` line ~311
- **Issue**: When `webhook.secret` is empty (the default), all POST requests are accepted.
- **Decision**: Empty secret is intentionally supported for compatibility (local alertmanager setups, simple self-hosted configs). Combined with S13 (default bind to `127.0.0.1`), network-exposed risk is substantially reduced. Documentation should note that `secret` should be set when the endpoint is exposed on a non-loopback interface.

---

### HIGH

#### S3. Socket Symlink TOCTOU (pairs with S1) ✓ Implemented
- **File**: `src/daemon/mod.rs` lines ~411-419
- **Issue**: Startup does `if path.exists() { remove_file(path) }`. `Path::exists()` follows symlinks. If `/tmp/daemoneye.sock` is a symlink to another file, `remove_file()` deletes the target.
- **Fix applied**: Startup now uses `socket_path.symlink_metadata()` which does not follow symlinks. The `NotFound` error is ignored; any other stat error is propagated. Socket moved out of `/tmp` (S1 fix) eliminates the attack surface entirely.

#### S4. Path Traversal via Symlinks in `read_file` / `edit_file` ✓ Implemented
- **File**: `src/daemon/executor.rs` (local-path branches of ReadFile and EditFile handlers)
- **Issue**: Path validation rejects `..` but does not resolve symlinks. A symlink at `/home/user/innocent` → `/etc/shadow` bypasses the check.
- **Fix applied**: Both local-path branches now call `std::fs::canonicalize()` on the supplied path. For `read_file`, a failed canonicalize falls back to the original path (read returns a natural "not found" error). For `edit_file`, a failed canonicalize returns an early error. The tmp file for `edit_file` is now placed alongside the canonical file (via `PathBuf::with_extension("de_tmp")`) rather than alongside the symlink.

#### S5. Sudo Credentials Stored as Plaintext `String` ✓ Implemented
- **File**: `src/daemon/executor.rs` (background credential handling)
- **Issue**: Sudo passwords are stored in `String` which is not zeroed on drop. Heap memory retains the credential after use.
- **Fix applied**: Added `zeroize = "1"` to `Cargo.toml`. The extracted credential is now wrapped in `zeroize::Zeroizing<String>`, which overwrites heap memory on drop. The raw JSON line (`cred_line`) containing the password is explicitly zeroized with `Zeroize::zeroize()` immediately after parsing.

#### S6. Gemini Tool Response Name Hardcoded ✓ Implemented
- **File**: `src/ai/backends/gemini.rs` lines ~63-71
- **Issue**: All tool responses sent to Gemini used `"name": "run_terminal_command"` regardless of which tool was actually called. Any non-terminal-command tool on Gemini got misattributed; Gemini rejects the mismatched function response.
- **Root Cause**: `ToolResult` struct carried only `tool_call_id` and `content`, not `tool_name`.
- **Fix applied**: Added `tool_name: String` to `ToolResult` in `src/ai/types.rs`. All four construction sites in `server.rs` now populate the field from `call.tool_name()`. `gemini.rs` `convert_messages` uses `tr.tool_name` for both `functionResponse.name` fields. New test `gemini_convert_tool_results_uses_correct_function_name` verifies the mapping.

---

### MEDIUM

#### S7. `thought_signature` Dropped in Anthropic/OpenAI Backends ✓ Implemented
- **Files**: `src/ai/backends/anthropic.rs`
- **Issue**: Anthropic's extended thinking returns `thinking` content blocks with a `signature` field required for multi-turn round-trips. The backend was dropping them silently.
- **Fix applied**: Anthropic backend now tracks `thinking` blocks during SSE streaming (`thinking_delta` / `signature_delta` events), encodes the full block as `{"thinking": "...", "signature": "..."}` JSON in `pending_thought_sig`, and passes it to `dispatch_tool_event`. `convert_messages` now emits the full `thinking` content block before each `tool_use` block when a signature is present. OpenAI has no equivalent `thought_signature` concept; `None` remains correct for that backend.

#### S8. Anthropic Stream End Missing Tool Flush ✓ Implemented
- **File**: `src/ai/backends/anthropic.rs` lines ~104-165
- **Issue**: Unlike OpenAI, the Anthropic backend has no final flush for buffered tool arguments after the SSE stream ends. If the stream ends before `content_block_stop`, the entire tool call is silently lost.
- **Fix applied**: Final flush block added after the stream loop (mirrors the OpenAI pattern).

#### S9. ReDoS in Grep Patterns ✓ Implemented
- **File**: `src/daemon/executor.rs` (three sites: watch_pane ~1092, read_file ~1303, grep filter ~1825)
- **Issue**: User-supplied search patterns go directly to `regex::Regex::new()`. Pathological patterns cause exponential backtracking, hanging the daemon.
- **Fix applied**: All three sites now use `regex::RegexBuilder::new(pat).size_limit(1 << 20).build()`. Error paths return a user-visible error string rather than panicking.

#### S10. Gemini Malformed-Call Parser Too Permissive ✓ Implemented
- **File**: `src/ai/backends/gemini.rs` lines ~19-44 (`parse_malformed_gemini_call`)
- **Issue**: The fallback regex matched `command = '...'` anywhere in text, including model commentary outside the call.
- **Fix applied**: Parser now extracts the substring between the parentheses of `run_terminal_command(...)` using a paren-depth counter, then applies `CMD_RE` / `BG_RE` only to that call body. Commentary text outside the call cannot match. `log::warn!` emitted on every fallback invocation. Two new tests verify the isolation (commentary-only, and mixed commentary + real call).

#### S11. Terminal Escape Code Injection via `pane_id` ✓ Implemented
- **File**: `src/daemon/server.rs` (notify handlers)
- **Issue**: `pane_id` received from external hook IPC was used without validation. A crafted `daemoneye notify activity` call with an ANSI-escaped or shell-injecting pane_id could pollute the cache or broadcast channels.
- **Fix applied**: `is_valid_pane_id()` helper added in `server.rs` — accepts only `%` followed by one or more ASCII digits (the tmux pane ID format). Applied to `NotifyActivity`, `NotifyComplete`, and `NotifyFocus` handlers; invalid IDs are logged at WARN and dropped. Six tests cover valid IDs, no-digits, no-prefix, non-digit chars, ANSI injection, and shell injection.

#### S12. Single-Quote Escaping Gap in Hook Commands ✓ Implemented
- **File**: `src/daemon/utils.rs` (`shell_escape_arg`)
- **Issue**: The function doesn't handle single quotes within single-quoted strings. A session name containing `'` breaks the quoting context in tmux hook commands.
- **Fix applied**: `'` is replaced with `'\''` (end-single-quote, backslash-escaped `'` outside quotes, begin-single-quote), which tmux's `cmd_string_parse` collapses to a literal `'`. The `\` replacement is applied first so the injected `\` from the `'` escaping isn't doubled. Two new tests cover single and multiple single-quotes.

---

### LOW

#### S13. Webhook Binds 0.0.0.0 by Default ✓ Implemented
- **File**: `src/webhook.rs` line ~605, `src/config.rs`
- **Issue**: Webhook listener binds all interfaces, exposing it to the network on internet-facing servers.
- **Fix applied**: `WebhookConfig.bind_addr` added with `#[serde(default = "default_webhook_bind")]` defaulting to `"127.0.0.1"`. `start()` parses the field before moving `config` into state.

#### S14. No IPC Message Size Limits ✓ Implemented
- **File**: `src/daemon/server.rs` (read loop)
- **Issue**: No maximum on incoming JSON message size. A malicious client can send an arbitrarily large payload to exhaust memory.
- **Fix applied**: `MAX_IPC_MESSAGE_BYTES = 1 << 20` (1 MiB) constant added. After `read_line`, `line.len()` is checked before `serde_json::from_str()`; oversized messages are rejected with `Response::Error` and the connection is closed.

#### S15. Unbounded SSE Leftover Buffer ✓ Implemented
- **File**: `src/ai/backends/openai.rs`
- **Issue**: The SSE leftover buffer has no size cap. A misbehaving proxy could send unlimited data without newlines.
- **Fix applied**: `MAX_LEFTOVER_BYTES = 1 MiB` constant added. After each chunk is appended to `leftover`, its length is checked; if exceeded, the stream is aborted with an error describing the overrun.

#### S16. Webhook Dedup Window Not Validated ✓ Implemented
- **File**: `src/webhook.rs` (`process_alert`)
- **Issue**: `dedup_window_secs` has no bounds. A value of 0 disables dedup; an extremely large value causes unbounded HashMap growth.
- **Fix applied**: Window is clamped to `1..=86400` at use. Dedup HashMap is capped at 10,000 entries; when the cap is reached the oldest entry (minimum timestamp) is evicted before inserting the new fingerprint.

---

## Architectural Issues

### Error Handling

#### A1. Spawned Tasks Don't Propagate Errors
- **File**: `src/daemon/mod.rs` lines ~315-441
- **Issue**: `tokio::spawn()` is used for cache monitor, scheduler, webhook, and client handlers. Task failures are logged but not escalated. Critical subsystems (scheduler, webhook) can die silently.
- **Fix**: Track `JoinHandle`s for critical tasks. Use a supervisor loop that restarts failed tasks with exponential backoff. For client handlers, ensure the connection is closed cleanly on error.

#### A2. Silent I/O Failures in Session Persistence ✓ Implemented
- **Files**: `src/daemon/session.rs` (A9 batch), `src/daemon/background.rs` line ~37
- **Issue**: File write failures (disk full, permissions) are swallowed via `.ok()` with no logging. Sessions are lost silently on daemon restart.
- **Fix applied**: `session.rs` — fixed in A9 batch (`write_session_file` and `append_session_message` now log WARN on failure). `background.rs` — `create_dir_all` failure and `capture_pane_to_file` failure now both emit `log::warn!` instead of being silently ignored.

#### A3. Timeout Indistinguishable from Explicit Denial ✓ Implemented
- **File**: `src/daemon/executor.rs` (`prompt_and_await_approval`)
- **Issue**: Approval timeout and user denial follow the same code path. If the CLI never received the prompt (crash, tmux issue), the tool is silently denied with no indication.
- **Fix applied**: In the `timed_out` branch, a `Response::SystemMsg` is sent to the client before returning the `ToolCallOutcome::Result` so the user sees a clear notification ("Approval prompt timed out after 60 s — the command was not executed.") even if their approval window closed before they could respond. The AI also receives the timeout message as the tool result so it can explain what happened.

#### A4. Retry Logic Doesn't Classify Errors
- **File**: `src/ai/mod.rs` lines ~45-80 (`send_with_retry`)
- **Issue**: 401/403 errors (permanent configuration failures) are retried with the same backoff as transient errors, wasting 6+ seconds before failing.
- **Fix**: Fail immediately on 4xx errors except 429. Only retry on 429 and 5xx.

---

### Resource Management

#### A5. No RAII Guard for tmux Hook Cleanup ✓ Implemented
- **File**: `src/daemon/executor.rs` lines ~514-690
- **Issue**: N9 alert-silence and pane-title-changed hooks are installed before the foreground wait loop but only cleaned up on the normal exit path. Early returns via `?` leave stale hooks.
- **Fix applied**: `FgHookGuard` struct added in `executor.rs` with a `Drop` impl that calls `tmux set-hook -u` for all registered hooks and `tmux set-option -u monitor-silence` if the silence option was set. The guard is created after installing the title hook; `guard.add_silence()` registers the N9 hooks. The explicit cleanup blocks are removed — the guard now drops at end of scope (or explicitly with `drop(guard)` before the capture delay, to avoid spurious re-fires during output collection).

#### A6. Hook Lifecycle Not Tied to Session Lifecycle ✓ Implemented
- **File**: `src/daemon/mod.rs` (`install_session_hooks`), `src/ipc.rs`, `src/main.rs`, `src/cli/commands.rs`, `src/daemon/server.rs`
- **Issue**: Per-session hooks (`pane-focus-in`, `session-window-changed`, `client-resized`) are installed when a session is first seen but the daemon never cleaned up its own state when the tmux session is destroyed.
- **Fix applied**: `install_session_hooks` now also installs a `session-closed` hook (per-session, `-t session_name`). The hook command embeds the session name directly (escaped via `shell_escape_arg`) rather than relying on `#{session_name}` format expansion after the session is gone. When the hook fires, it calls `daemoneye notify session-closed NAME` via IPC. The `NotifySessionClosed` handler in `server.rs` iterates the session store, calls `cleanup_bg_windows()` on all matching entries (kills bg windows, stops pipe-pane logs), removes them from the map, and logs the cleanup. Per-session hooks registered with `-t` are automatically removed by tmux when the session is destroyed, so no explicit hook teardown is needed.

#### A7. Pipe Log Growth Unbounded ✓ Implemented
- **File**: `src/tmux/cache.rs` (`read_pipe_log`)
- **Issue**: Pipe logs grow without bound during long sessions with high output volume. The 50 KB read limit in `read_pipe_log` doesn't prevent on-disk growth.
- **Fix applied**: `PIPE_LOG_ROTATE_THRESHOLD = 10 MiB` constant added. After each `read_pipe_log` call, if `file_size > PIPE_LOG_ROTATE_THRESHOLD`, the file is truncated and the 50 KB tail we already hold is written back. tmux's `cat` process keeps the file open with `O_APPEND` so subsequent writes land at the new end cleanly. A `log::debug!` line records each rotation; `log::warn!` on write failure. The 2 s cache-poller interval means at most ~2 s of output can be lost during a rotation, which is acceptable for context-delivery purposes.

#### A8. Background Window Eviction Kills Without Grace Period ✓ Implemented
- **File**: `src/daemon/executor.rs` (bg window cap enforcement)
- **Issue**: When the 5-window cap is hit, the oldest completed window is killed immediately via `kill_job_window()`. In-flight work in that window is lost.
- **Fix applied**: Before evicting a completed-command window, `pane_current_command` is checked. If the pane is still running a non-shell process (i.e. a user re-used the pane after the tracked command finished), eviction is skipped: the entry is re-inserted and a denial message is returned, same as the all-running-windows path. If the pane is idle (shell or empty), eviction proceeds as before.

---

### State Consistency

#### A9. Session History Write is Not Transactional ✓ Implemented
- **File**: `src/daemon/session.rs` (`write_session_file`, `append_session_message`)
- **Issue**: History compaction writes to disk then updates in-memory. If the write fails (disk full), in-memory and on-disk diverge. The next restart loads the incomplete on-disk version.
- **Fix applied**: `write_session_file` now writes to `<path>.jsonl.tmp`, calls `sync_all()`, then renames atomically over the real file. On failure: WARN is logged and the tmp file is cleaned up, leaving the old on-disk file intact. `append_session_message` now logs WARN on any I/O failure instead of silently swallowing errors (A2 partial coverage).

#### A10. Lock Poisoning Recovery Is Silent ✓ Implemented
- **Files**: `src/tmux/cache.rs`, throughout codebase (44 sites across 10 files)
- **Issue**: Poisoned lock recovery silently continues with potentially inconsistent state.
- **Fix applied**: `UnpoisonExt` trait added in `src/util.rs` with `unwrap_or_log()` method that calls `log::error!()` before returning the inner value. All 44 `.unwrap_or_else(|e| e.into_inner())` call sites replaced with `.unwrap_or_log()`.

---

### Testing Gaps

#### A11. No Concurrent Client Integration Tests
- The daemon accepts concurrent connections but there are no tests for session isolation under load, lock contention, or message history consistency with parallel clients.

#### A12. No Daemon Restart Recovery Tests
- Session persistence, schedule recovery, and hook reinstallation after restart are untested.

#### A13. No Fault-Injection Tests
- Error paths (tmux unavailable, disk full, API key revoked mid-conversation) are not exercised.

---

## Functional Gaps

#### F1. No `daemoneye status` Command
- No way to inspect daemon uptime, active sessions, installed hooks, or resource usage without reading logs.

#### F2. Graceful Shutdown (SIGTERM/SIGINT) ✓ Implemented
- **File**: `src/daemon/mod.rs` (end of `run_daemon`)
- **Issue**: On SIGTERM/SIGINT the daemon removed the socket but left global tmux hooks installed and bg windows open, so hooks would fire against a dead daemon and produce errors in tmux.
- **Fix applied**: Shutdown sequence after the accept loop now: (1) removes the socket, (2) uninstalls all four global tmux hooks (`pane-died`, `after-new-session`, `client-attached`, `client-detached`) via `set-hook -gu`, (3) iterates all active sessions and calls `cleanup_bg_windows()` on each (kills bg windows, stops pipe-pane logs), (4) logs `"Daemon stopped cleanly."`.

#### F3. No API Key Validation at Startup ✓ Implemented
- **File**: `src/daemon/mod.rs`
- **Issue**: Invalid API keys are discovered only when a user runs `ask`/`chat`, producing a cryptic 401 error. Format mismatches (e.g. using an OpenAI key with the Anthropic provider) go undetected at startup.
- **Fix applied**: After the empty-key check, the daemon now validates the key prefix for known providers: Anthropic keys must start with `sk-ant-`; OpenAI keys must start with `sk-`. A mismatch emits `log::warn!` at startup (non-fatal, to avoid breaking proxy setups with non-standard key formats). Local providers (`ollama`, `lmstudio`) are skipped.

#### F4. `ensure_dirs()` Failure Doesn't Abort Startup ✓ Implemented
- **File**: `src/main.rs` lines ~150-152
- **Issue**: If config directory creation fails, startup continues and all subsequent I/O silently fails. This should be a fatal error.
- **Fix applied**: Changed from `eprintln!` warning to `?` propagation — `main()` now returns `Err` and exits non-zero if `ensure_dirs()` fails.

#### F5. No Circuit Breaker for Flaky AI Backends
- Repeated API failures cause repeated full-backoff retries. A circuit-breaker (fail fast after N consecutive errors, reset after a cooldown) would improve the user experience during outages.

#### F6. No Session Export
- Session history is persisted but there's no CLI command to export it to a portable format for backup or sharing.

---

## Priority Implementation Order

| Priority | ID | Description | Effort |
|---|---|---|---|
| 1 | S1+S3 | ~~Move socket to `~/.daemoneye/daemoneye.sock`~~ ✓ Done | Small |
| 2 | S13 | ~~Default webhook bind to `127.0.0.1`~~ ✓ Done | Trivial |
| 3 | S6 | ~~Add `tool_name` to `ToolResult`; fix Gemini hardcoded name~~ ✓ Done | Small |
| 4 | S8 | ~~Add final tool flush in Anthropic backend~~ ✓ Done | Trivial |
| 5 | S9 | ~~Add `size_limit()` to regex builder (3 sites in executor.rs)~~ ✓ Done | Trivial |
| 6 | S10 | ~~Tighten Gemini malformed-call parser~~ ✓ Done | Small |
| 7 | S7 | ~~Forward `thought_signature` in Anthropic extended thinking~~ ✓ Done | Small |
| 8 | A4 | ~~Fail fast on 4xx in `send_with_retry`~~ ✓ Already correct | Trivial |
| 9 | A5 | ~~RAII hook guard in foreground executor~~ ✓ Done | Medium |
| 10 | A6 | ~~Session-closed hook for per-session hook cleanup~~ ✓ Done | Medium |
| 11 | S2 | ~~Require webhook secret~~ Won't Fix — empty secret allowed by design | — |
| 12 | S4 | ~~`canonicalize()` for file path validation~~ ✓ Done | Small |
| 13 | A9 | ~~Atomic session history writes (write-tmp-rename)~~ ✓ Done | Medium |
| 14 | A10 | ~~Log poisoned lock recovery (`UnpoisonExt` trait, 44 sites)~~ ✓ Done | Trivial |
| 15 | A1 | Task supervision for critical spawned tasks | Large |
| 16 | F4 | ~~Fatal error on `ensure_dirs()` failure~~ ✓ Done | Trivial |
| 17 | S5 | ~~`zeroize` crate for sudo credentials~~ ✓ Done | Small |
| 18 | F2 | ~~SIGTERM/SIGINT graceful shutdown~~ ✓ Done | Medium |
| 19 | F3 | ~~API key format validation at startup~~ ✓ Done | Small |
| 20 | S11 | ~~Validate pane_id format in notify handlers~~ ✓ Done | Small |
