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

#### S2. Unauthenticated Webhook Endpoint
- **File**: `src/webhook.rs` line ~311
- **Issue**: When `webhook.secret` is empty (the default), all POST requests are accepted. An attacker on the network can inject fake alerts and trigger AI analysis/remediation.
- **Fix**: Require a non-empty secret when webhook is enabled. Either generate a default random secret at first run, or refuse to start the webhook listener if no secret is configured.

---

### HIGH

#### S3. Socket Symlink TOCTOU (pairs with S1) ✓ Implemented
- **File**: `src/daemon/mod.rs` lines ~411-419
- **Issue**: Startup does `if path.exists() { remove_file(path) }`. `Path::exists()` follows symlinks. If `/tmp/daemoneye.sock` is a symlink to another file, `remove_file()` deletes the target.
- **Fix applied**: Startup now uses `socket_path.symlink_metadata()` which does not follow symlinks. The `NotFound` error is ignored; any other stat error is propagated. Socket moved out of `/tmp` (S1 fix) eliminates the attack surface entirely.

#### S4. Path Traversal via Symlinks in `read_file` / `edit_file`
- **File**: `src/daemon/executor.rs` (file tool handlers, lines ~1338-1346)
- **Issue**: Path validation rejects `..` but does not resolve symlinks. A symlink at `/home/user/innocent` → `/etc/shadow` bypasses the check.
- **Fix**: Call `std::fs::canonicalize()` on the path before applying allow/deny rules.

#### S5. Sudo Credentials Stored as Plaintext `String`
- **File**: `src/daemon/background.rs` lines ~170-185
- **Issue**: Sudo passwords are stored in `String` which is not zeroed on drop. Heap memory retains the credential after use.
- **Fix**: Use `zeroize::Zeroizing<String>` for credential storage. Clear immediately after the sudo command completes rather than caching.

#### S6. Gemini Tool Response Name Hardcoded ✓ Implemented
- **File**: `src/ai/backends/gemini.rs` lines ~63-71
- **Issue**: All tool responses sent to Gemini used `"name": "run_terminal_command"` regardless of which tool was actually called. Any non-terminal-command tool on Gemini got misattributed; Gemini rejects the mismatched function response.
- **Root Cause**: `ToolResult` struct carried only `tool_call_id` and `content`, not `tool_name`.
- **Fix applied**: Added `tool_name: String` to `ToolResult` in `src/ai/types.rs`. All four construction sites in `server.rs` now populate the field from `call.tool_name()`. `gemini.rs` `convert_messages` uses `tr.tool_name` for both `functionResponse.name` fields. New test `gemini_convert_tool_results_uses_correct_function_name` verifies the mapping.

---

### MEDIUM

#### S7. `thought_signature` Dropped in Anthropic/OpenAI Backends
- **Files**: `src/ai/backends/anthropic.rs` line ~143, `src/ai/backends/openai.rs` lines ~132/163
- **Issue**: Both backends pass `None` for `thought_signature` to `dispatch_tool_event()`. Only Gemini correctly extracts it. Model reasoning traces are silently dropped.
- **Fix**: Extract and forward `thought_signature` in both backends, mirroring the Gemini implementation.

#### S8. Anthropic Stream End Missing Tool Flush ✓ Implemented
- **File**: `src/ai/backends/anthropic.rs` lines ~104-165
- **Issue**: Unlike OpenAI, the Anthropic backend has no final flush for buffered tool arguments after the SSE stream ends. If the stream ends before `content_block_stop`, the entire tool call is silently lost.
- **Fix applied**: Final flush block added after the stream loop (mirrors the OpenAI pattern).

#### S9. ReDoS in Grep Patterns ✓ Implemented
- **File**: `src/daemon/executor.rs` (three sites: watch_pane ~1092, read_file ~1303, grep filter ~1825)
- **Issue**: User-supplied search patterns go directly to `regex::Regex::new()`. Pathological patterns cause exponential backtracking, hanging the daemon.
- **Fix applied**: All three sites now use `regex::RegexBuilder::new(pat).size_limit(1 << 20).build()`. Error paths return a user-visible error string rather than panicking.

#### S10. Gemini Malformed-Call Parser Too Permissive
- **File**: `src/ai/backends/gemini.rs` lines ~19-44 (`parse_malformed_gemini_call`)
- **Issue**: The fallback regex matches `command = '...'` anywhere in text, including model commentary. A Gemini response saying "the user might try: `command = 'rm -rf /'`" could trigger an unintended command.
- **Fix**: Require the full function call prefix (`run_terminal_command\s*\([^)]*command\s*=`) and only invoke the fallback if structured JSON parsing fails AND the text contains `run_terminal_command(`. Log all fallback matches as WARN.

#### S11. Terminal Escape Code Injection via `pane_id`
- **File**: `src/tmux/cache.rs` lines ~534, 621
- **Issue**: `pane_id` is embedded in context output strings without sanitizing ANSI escape codes. A direct invocation of `daemoneye notify activity` with a crafted pane_id can inject escape sequences into the AI's context.
- **Fix**: Validate pane_id matches `^%[0-9]+$` in notify handlers before use. Strip non-printable characters before embedding in context strings.

#### S12. Single-Quote Escaping Gap in Hook Commands
- **File**: `src/daemon/utils.rs` (`shell_escape_arg`)
- **Issue**: The function doesn't handle single quotes within single-quoted strings. A session name containing `'` breaks the quoting context in tmux hook commands.
- **Fix**: Use `replace("'", "'\\''")` for single-quote escaping.

---

### LOW

#### S13. Webhook Binds 0.0.0.0 by Default ✓ Implemented
- **File**: `src/webhook.rs` line ~605, `src/config.rs`
- **Issue**: Webhook listener binds all interfaces, exposing it to the network on internet-facing servers.
- **Fix applied**: `WebhookConfig.bind_addr` added with `#[serde(default = "default_webhook_bind")]` defaulting to `"127.0.0.1"`. `start()` parses the field before moving `config` into state.

#### S14. No IPC Message Size Limits
- **File**: `src/daemon/server.rs` (read loop)
- **Issue**: No maximum on incoming JSON message size. A malicious client can send an arbitrarily large payload to exhaust memory.
- **Fix**: Check `line.len()` before `serde_json::from_str()`, or use a `BufReader` with `take()`.

#### S15. Unbounded SSE Leftover Buffer
- **File**: `src/ai/backends/openai.rs`
- **Issue**: The SSE leftover buffer has no size cap. A misbehaving proxy could send unlimited data without newlines.
- **Fix**: Cap the leftover buffer (e.g. 1 MB) and return an error if exceeded.

#### S16. Webhook Dedup Window Not Validated
- **File**: `src/webhook.rs` lines ~362-379
- **Issue**: `dedup_window_secs` has no bounds. A value of 0 disables dedup; an extremely large value causes unbounded HashMap growth.
- **Fix**: Clamp to `1..=86400`. Cap the dedup HashMap at a reasonable size (e.g. 10,000 entries).

---

## Architectural Issues

### Error Handling

#### A1. Spawned Tasks Don't Propagate Errors
- **File**: `src/daemon/mod.rs` lines ~315-441
- **Issue**: `tokio::spawn()` is used for cache monitor, scheduler, webhook, and client handlers. Task failures are logged but not escalated. Critical subsystems (scheduler, webhook) can die silently.
- **Fix**: Track `JoinHandle`s for critical tasks. Use a supervisor loop that restarts failed tasks with exponential backoff. For client handlers, ensure the connection is closed cleanly on error.

#### A2. Silent I/O Failures in Session Persistence
- **Files**: `src/daemon/session.rs` lines ~102-125, `src/daemon/background.rs` line ~37
- **Issue**: File write failures (disk full, permissions) are swallowed via `.ok()` with no logging. Sessions are lost silently on daemon restart.
- **Fix**: Log all I/O errors at WARN. Consider surfacing persistent failure to the user via a `Response::SystemMsg`.

#### A3. Timeout Indistinguishable from Explicit Denial
- **File**: `src/daemon/executor.rs` lines ~117-141
- **Issue**: Approval timeout and user denial follow the same code path. If the CLI never received the prompt (crash, tmux issue), the tool is silently denied with no indication.
- **Fix**: Add a `Parsed::TimedOut` variant. For timeouts, inject a system message informing the user that the approval prompt was not answered.

#### A4. Retry Logic Doesn't Classify Errors
- **File**: `src/ai/mod.rs` lines ~45-80 (`send_with_retry`)
- **Issue**: 401/403 errors (permanent configuration failures) are retried with the same backoff as transient errors, wasting 6+ seconds before failing.
- **Fix**: Fail immediately on 4xx errors except 429. Only retry on 429 and 5xx.

---

### Resource Management

#### A5. No RAII Guard for tmux Hook Cleanup
- **File**: `src/daemon/executor.rs` lines ~514-690
- **Issue**: N9 alert-silence and pane-title-changed hooks are installed before the foreground wait loop but only cleaned up on the normal exit path. Early returns via `?` leave stale hooks.
- **Fix**: Wrap hook installation in a struct with a `Drop` impl that uninstalls hooks unconditionally.

#### A6. Hook Lifecycle Not Tied to Session Lifecycle
- **File**: `src/daemon/mod.rs` (`install_session_hooks`)
- **Issue**: Per-session hooks (`pane-focus-in`, `session-window-changed`, `client-resized`) are installed when a session is first seen but never removed when the session is destroyed.
- **Fix**: Install a `session-closed` hook (or use `after-kill-session`) to trigger hook cleanup for the dying session. Alternatively, audit installed hooks at startup and remove any for sessions that no longer exist.

#### A7. Pipe Log Growth Unbounded
- **File**: `src/tmux/mod.rs` (`start_pipe_pane`), `src/daemon/mod.rs` (startup cleanup)
- **Issue**: Pipe logs grow without bound during long sessions with high output volume. The 50 KB read limit in `read_pipe_log` doesn't prevent on-disk growth.
- **Fix**: Rotate pipe logs at a configurable size threshold (e.g. 10 MB). Clean up on pane close, not just daemon restart.

#### A8. Background Window Eviction Kills Without Grace Period
- **File**: `src/daemon/executor.rs` lines ~797-806
- **Issue**: When the 5-window cap is hit, the oldest completed window is killed immediately via `kill_job_window()`. In-flight work in that window is lost.
- **Fix**: Check `#{window_active}` before killing. For running windows, send SIGTERM and wait briefly before SIGKILL.

---

### State Consistency

#### A9. Session History Write is Not Transactional
- **File**: `src/daemon/server.rs` lines ~663-669
- **Issue**: History compaction writes to disk then updates in-memory. If the write fails (disk full), in-memory and on-disk diverge. The next restart loads the incomplete on-disk version.
- **Fix**: Write to a `.tmp` file, `fsync`, then rename atomically. Only update in-memory state after the rename succeeds.

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

#### F2. No Graceful Shutdown (SIGTERM/SIGINT)
- The daemon doesn't handle signals for clean shutdown (remove socket, uninstall global hooks, flush logs).

#### F3. No API Key Validation at Startup
- Invalid API keys are discovered only when a user runs `ask`/`chat`, producing a cryptic error. A startup validation call (or `--validate` flag) would surface this earlier.

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
| 6 | S10 | Tighten Gemini malformed-call parser | Small |
| 7 | S7 | Forward `thought_signature` in Anthropic/OpenAI backends | Small |
| 8 | A4 | ~~Fail fast on 4xx in `send_with_retry`~~ ✓ Already correct | Trivial |
| 9 | A5 | RAII hook guard in foreground executor | Medium |
| 10 | A6 | Session-closed hook for per-session hook cleanup | Medium |
| 11 | S2 | Require webhook secret by default | Small |
| 12 | S4 | `canonicalize()` for file path validation | Small |
| 13 | A9 | Atomic session history writes (write-tmp-rename) | Medium |
| 14 | A10 | ~~Log poisoned lock recovery (`UnpoisonExt` trait, 44 sites)~~ ✓ Done | Trivial |
| 15 | A1 | Task supervision for critical spawned tasks | Large |
| 16 | F4 | ~~Fatal error on `ensure_dirs()` failure~~ ✓ Done | Trivial |
| 17 | S5 | `zeroize` crate for sudo credentials | Small |
| 18 | F2 | SIGTERM/SIGINT graceful shutdown | Medium |
| 19 | F3 | API key validation at startup | Small |
| 20 | S11 | Validate pane_id format in notify handlers | Small |
