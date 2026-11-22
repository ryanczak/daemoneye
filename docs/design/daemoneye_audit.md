# DaemonEye Codebase Audit Report

**Scope:** `daemoneye` v2.10.0 (`HEAD e46e946e`, Rust 2024 edition) — 305 `.rs` files under `src/`, 175,564 total lines across `src/` + `tests/` (~79.8k production LOC).  
**Baseline Verification:**
- `cargo fmt --check`: **PASS**
- `cargo clippy --all-targets -- -D warnings`: **PASS**
- `cargo test --test '*'`: **PASS** (105 passed, 3 ignored, 0 failed across 7 integration/invariant test binaries)
- `cargo test --lib`: **1 FAILURE** out of 3,054 unit tests (3,043 passed, 10 ignored) — [`shell::host::tests::host_sends_lagged_when_subscriber_falls_behind`](file:///home/matt/src/daemoneye/src/shell/host.rs#L760-L826) failed due to a test-scheduling race (only reads 3 frames after 21 concurrent pushes, exiting before the post-lag chunk if 2 chunks drain before lag occurs).

---

## Executive Scorecard

| Dimension | Rating | Summary |
|---|---|---|
| **Safety & Security** | **Needs Urgent Fixes** | Solid `SO_PEERCRED` IPC auth, minimal `unsafe`, and masking discipline, but **3 Critical** vulnerabilities exist: tmux `#(...)` format injection via unauthenticated localhost webhooks, Ghost sandbox escape + silent remote-host approval bypass via `open_shell`/`send_input`, and Ghost root escalation via user-writable `NOPASSWD` scripts + `sudo` lexer bypasses. |
| **Functionality & Correctness** | **Needs Urgent Fixes** | Multi-turn Ghost Shells are functionally broken across all 3 LLM providers (combined `tool_calls`+`tool_results` message drops `tool_calls`, and the resulting HTTP 400 is silently swallowed as `Complete`). PTY command timeouts are bypassed on continuous output; Anthropic cache accounting zeroes uncached `input_tokens`; transcript viewer panics on whitespace-only lines. |
| **Performance & Concurrency** | **Moderate Risk** | Significant blocking I/O and subprocess churn on Tokio worker threads (`tmux` 2s poller spawning OS threads + 10ms sleeps, non-WAL SQLite `busy_timeout=0` fsyncs on every `log_event`), `ScheduleStore::save` concurrent `.tmp` file race, `O(N)` full-file scans in `read_line_at_offset`, and 2s `sbx exec` polling per idle sandbox shell. |
| **Architecture & Coupling** | **Moderate Debt** | Strong local hygiene (zero prod file/fn exemptions left), clean `src/ai/` boundary, and sound `SessionStore` non-reentrancy guard. Macro boundaries have eroded across ~38 milestones: **20 bidirectional production module cycles**, ~28 mutable process globals bypassing `DaemonState`, 5 divergent ghost spawners, leaky IPC streams inside `executor`, and heavy doc drift. |
| **Code Quality & Testing** | **Strong w/ Targeted Gaps** | Zero current lock-poison `.unwrap()` violations and ~3,170 tests. Gaps include a false-invariant `.expect()` on empty `[models]`, unchecked UTF-8 byte slicing, **two source-gate blind spots** (`text.split("#[cfg(test)]")` skipping ~30 files), 17 broken no-op `with_home` tests causing `HOME` env UB, and 22–24-parameter positional functions from mechanical splitting. |

> [!CAUTION]
> **Top 5 P0 Issues Requiring Immediate Action**
> 1. **Tmux `#(shell-command)` RCE via webhook `display-message`** ([process.rs](file:///home/matt/src/daemoneye/src/webhook/process.rs#L242-L257))
> 2. **Ghost sandbox escape (`open_shell`) + silent remote-SSH approval bypass + socket path traversal (`send_input`)** ([dispatch_open_shell.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_open_shell.rs#L22-L59), [dispatch_send_input.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_send_input.rs#L45-L136))
> 3. **Ghost multi-turn tool loop broken on all 3 AI providers + silent error swallowing** ([turn_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/ghost/turn_helpers.rs#L452-L483), [turn.rs](file:///home/matt/src/daemoneye/src/daemon/ghost/turn.rs#L84-L91))
> 4. **Ghost root escalation via user-writable `NOPASSWD` scripts + `sudo` detection/basename bypasses** ([scripts.rs](file:///home/matt/src/daemoneye/src/scripts.rs#L149-L283), [policy.rs](file:///home/matt/src/daemoneye/src/daemon/policy.rs#L111-L139), [sudo.rs](file:///home/matt/src/daemoneye/src/daemon/utils/sudo.rs#L48-L162))
> 5. **PTY command timeout bypass + unbounded RAM on continuous output** ([client_read.rs](file:///home/matt/src/daemoneye/src/shell/client_read.rs#L191-L291))

---

## 1. Safety & Security

### Critical

1. **Tmux Format-String & `#(shell-command)` RCE via Webhook Alerts** — [process.rs](file:///home/matt/src/daemoneye/src/webhook/process.rs#L242-L257), [notice_sinks.rs](file:///home/matt/src/daemoneye/src/daemon/notice_sinks.rs#L185-L190)
   - `notify_chat_panes` runs `Command::new("tmux").args(["display-message", "-d", "8000", "-t", pane, msg])` without `-l`, `--`, or `#` escaping. `msg` embeds untrusted webhook `alert_name` / `summary`.
   - `tmux display-message` without `-l` evaluates `FORMATS`, executing `#(cmd)` via `/bin/sh` whenever any chat session has an active `chat_pane` — even with `auto_analyze = false` and Ghost Shells disabled, against the default unauthenticated `127.0.0.1:9393` webhook listener.
   - **Fix:** Strip control characters/newlines, escape `#` → `##`, and pass `--` (and `-l` where supported).

2. **Ghost Sandbox Escape (`open_shell`) + Silent Remote-Host Approval Bypass & Socket Traversal (`send_input`)** — [dispatch_open_shell.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_open_shell.rs#L22-L59), [dispatch_send_input.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_send_input.rs#L45-L136), [approval.rs](file:///home/matt/src/daemoneye/src/cli/commands/approval.rs#L124-L146)
   - **Sandbox escape:** Unlike [dispatch_foreground.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_foreground.rs#L32-L37), `open_shell` never checks `config.sandbox.enabled` or `startup_error()`, allowing a sandboxed ghost to open an unsandboxed local host PTY registered under `Owner::Chat` (bypassing ghost shell caps and teardown).
   - **Remote approval bypass:** `open_shell(host: "prod")` requires no approval. Then `dispatch_send_input` builds `ApprovalRequest` with `host: None` hardcoded ([dispatch_send_input.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_send_input.rs#L116)), causing the CLI to classify it as a local regular command (`approvals.commands = true` by default) and auto-approve arbitrary keystrokes into remote SSH hosts with zero prompt. Spelling `sudo` via `keys` or quotes also bypasses `command_has_sudo`.
   - **Socket path traversal:** When `ShellId::parse(shell)` fails at line 54, execution continues and passes the raw unvalidated `shell` string to `socket_path(shell)` (`shells_dir().join(format!("{shell}.sock"))`), connecting to arbitrary `.sock` paths.
   - **Fix:** Gate `open_shell` on `sandbox.enabled`/`startup_error()` for ghosts and use `Owner::Ghost`; fail closed on `ShellId::parse` in `send_input`; resolve the shell's `HostId` from `ShellRegistry` into `ApprovalRequest.host`; require explicit approval for `send_input`.

3. **Ghost Shell Root Escalation & `sudo` Policy/Lexer Bypasses** — [scripts.rs](file:///home/matt/src/daemoneye/src/scripts.rs#L149-L283), [policy.rs](file:///home/matt/src/daemoneye/src/daemon/policy.rs#L111-L139), [sudo.rs](file:///home/matt/src/daemoneye/src/daemon/utils/sudo.rs#L48-L162)
   - `install_sudoers` grants `NOPASSWD` to user-writable `~/.daemoneye/scripts/<name>` with no `sha256:` digest. Because `GhostPolicy::is_safe` permits all non-sudo commands, any unsandboxed ghost can overwrite an approved script and invoke `sudo <name>` as root.
   - `GhostPolicy::is_safe` approves any `sudo /arbitrary/path/<approved-basename>` by basename alone, and `resolve_command` leaves absolute paths unchanged.
   - `command_has_sudo()` is bypassed by `s\udo id`, `s'u'do id`, `"su""do" id`, leading redirections (`</dev/null sudo id`), globs (`/usr/bin/sud?`), and alternative elevation/wrappers (`su`, `doas`, `pkexec`, `run0`, `strace sudo`, `flock ... sudo`), defeating both Ghost Policy and interactive `sudo` prompts.
   - `GhostPolicy::wrap_remote()` ([policy.rs](file:///home/matt/src/daemoneye/src/daemon/policy.rs#L286-L297)) skips wrapping whenever `cmd` starts with `"ssh "`, allowing remote-fenced ghosts to SSH to arbitrary hosts, and interpolates `target` unquoted without `--`.
   - **Fix:** Pin sudoers rules with `sha256:` digests or install root-owned copies; require canonical paths inside `scripts_dir()` in `is_safe()`; harden `command_has_sudo()`; remove the `starts_with("ssh ")` escape and quote `target` with `--`.

### High

4. **Unbounded Memory DoS on IPC & Shell-Host Reads + Pipelined Framing Bug** — [server/mod.rs](file:///home/matt/src/daemoneye/src/daemon/server/mod.rs#L131-L151), [shell/host.rs](file:///home/matt/src/daemoneye/src/shell/host.rs#L109-L193)
   - `MAX_IPC_MESSAGE_BYTES` (1 MiB) is checked *after* unbounded `reader.read_line(&mut line)` finishes; subsequent approval/credential reads and `shell::host` have no cap at all, and accept loops have no concurrency bound.
   - [shell/host.rs](file:///home/matt/src/daemoneye/src/shell/host.rs#L181) slices `&line[..end]` instead of `&line[offset..end]` in its multi-frame loop, breaking pipelined JSON frames.
   - **Fix:** Shared `read_bounded_line` using `AsyncReadExt::take(max + 1)`; fix `&line[offset..end]`; cap concurrent connections with a `Semaphore`.

5. **Daemon API Keys Inherited by Every PTY Shell & Child Process** — [pty.rs](file:///home/matt/src/daemoneye/src/shell/pty.rs#L247-L260), [launch.rs](file:///home/matt/src/daemoneye/src/shell/launch.rs#L197-L220), [notice_sinks.rs](file:///home/matt/src/daemoneye/src/daemon/notice_sinks.rs#L206-L213)
   - `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` are never stripped (`env_remove`) when spawning `shell-host`, PTY shells, or notification shell hooks, letting any command/ghost read and exfiltrate daemon API keys.
   - **Fix:** Strip provider secret env vars in `PtyShell::spawn`, `spawn_detached`, and `deliver_command`.

6. **Remote `edit_file` (Including `sudo = true`) Predictable `.de_tmp` Symlink Overwrite & World-Readable Window** — [write.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/write.rs#L269-L307), [ops.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/ops.rs#L423-L435)
   - Remote Python/Perl payloads open predictable `p + '.de_tmp'` without `O_CREAT | O_EXCL | O_NOFOLLOW` (allowing symlink overwrites as root under `sudo = true`) and create the file with default umask (`0644`) before `chmod`ing, briefly exposing `0600` file contents. Local edits also use predictable `.with_extension("de_tmp")`.
   - **Fix:** Use `tempfile.NamedTemporaryFile` / `File::Temp::tempfile` remotely (`0600`, `O_EXCL`) and `create_new(true).mode(0o600)` with a UUID suffix locally.

7. **`file_ops` Path-Guard Symlink & Missing-Parent Bypasses** — [file_ops/mod.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/mod.rs#L60-L108)
   - `is_blocked_secret_path` checks the uncanonicalized path, so `/tmp/k -> ~/.ssh/id_ed25519` or `/tmp/e -> /app/.env` bypasses both `read_file` and `edit_file` blocks.
   - `resolve_path_for_guard` only canonicalizes `p` or `p.parent()`; `/tmp/de_link/newdir/evil.sh` (2+ missing components under a symlink to `~/.daemoneye`) falls back to lexical path and bypasses `in_daemoneye_dir()`.
   - **Fix:** Walk `ancestors()` to the deepest existing canonical directory and re-join trailing components; apply `is_blocked_secret_path` to the resolved canonical path.

8. **Path Traversal via Unvalidated `memory_namespace`, `session_id`, and `prompt`** — [memory.rs](file:///home/matt/src/daemoneye/src/memory.rs#L247-L259), [session.rs](file:///home/matt/src/daemoneye/src/daemon/session.rs#L174-L237), [load.rs](file:///home/matt/src/daemoneye/src/config/load.rs#L348-L355), [session_store.rs](file:///home/matt/src/daemoneye/src/session_store.rs#L264-L289)
   - `memory_dir_for_namespace` joins `namespace` directly (an absolute path or `../../scripts` escapes `~/.daemoneye/agents/`). `session_id` and `prompt` names are joined into paths without strict character validation.
   - **Fix:** Enforce `[A-Za-z0-9_-]` validation at IPC ingress and inside path builders.

9. **Ghost Delegation (`spawn_ghost_shell`) Bypasses Caller Policy & Lacks Interactive Approval** — [knowledge/ghost.rs](file:///home/matt/src/daemoneye/src/daemon/executor/knowledge/ghost.rs#L60-L132)
   - `caller_policy` host fencing only triggers when `host: Some(...)`; passing `host: None` inherits another runbook's `ssh_target`, `auto_approve_scripts`, and agent `tool_policy`. Interactive chat can also spawn autonomous ghosts without approval.
   - **Fix:** Always intersect child hosts/scripts/tool_policy with `caller_policy`, and gate interactive `spawn_ghost_shell` behind approval.

### Medium & Low Security / `unsafe` Audit

- **Webhook server hardening** ([webhook/server.rs](file:///home/matt/src/daemoneye/src/webhook/server.rs#L60-L144)): non-constant-time Bearer secret check (`==`); Axum `Json(body)` parses up to 2 MiB *before* auth/rate-limit; single POST alert array fans out N `tokio::spawn` tasks; `rate_limit` map never evicts; `state.config` snapshot ignores `/reload` secret rotation; raw alert payloads injected into watchdog/ghost/chat prompts without untrusted-data framing.
- **CLI flag injection:** `HostId::parse` permits leading `-` and SSH argv builders omit `--` ([host.rs](file:///home/matt/src/daemoneye/src/host.rs#L28-L52)); remote `read_file` runs `grep -E '{}'` without `-e` ([read.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/read.rs#L143)); `infocmp -x` lacks `--` ([tmux/session.rs](file:///home/matt/src/daemoneye/src/tmux/session.rs#L343)).
- **Ungated schedule deletion:** `cancel_schedule` / `delete_schedule` have no ghost guard and no user approval ([dispatch_schedule.rs](file:///home/matt/src/daemoneye/src/daemon/executor/dispatch_schedule.rs#L54-L60)).
- **Masking & credential bugs:**
  - [filter.rs](file:///home/matt/src/daemoneye/src/ai/filter.rs#L119-L121) + [L206-L210](file:///home/matt/src/daemoneye/src/ai/filter.rs#L206-L210): JSON secret rule replacement `r#""$1": "<REDACTED>""#` is returned from a `replace_all` closure, which does **not** expand `$1` — emitting literal `"$1": "<REDACTED>"` in production (hidden because unit tests used `rep.as_str()` directly).
  - `credential_round_trip` copies `Zeroizing` credential into a plain `Vec<u8>` ([shell_exec_auth.rs](file:///home/matt/src/daemoneye/src/daemon/executor/shell_exec_auth.rs#L72-L75)); `Request`, `ModelEntry`, `WebhookConfig` derive `Debug` with plaintext secrets; 7 prompt response handlers ignore `resp_id == prompt_id`.
- **Production `unsafe` review:** `SO_PEERCRED` ([server/mod.rs](file:///home/matt/src/daemoneye/src/daemon/server/mod.rs#L41-L92)), `ioctl`, `tcgetattr`/`tcsetattr`, and `kill(getpid(), SIGTERM)` are sound. Three low-severity defects found:
  - [ready.rs](file:///home/matt/src/daemoneye/src/daemon/ready.rs#L25): `libc::pipe()` lacks `O_CLOEXEC` (use `pipe2`), so startup children inherit the readiness write fd.
  - [main.rs](file:///home/matt/src/daemoneye/src/main.rs#L393-L399), [startup.rs](file:///home/matt/src/daemoneye/src/daemon/startup.rs#L117-L132), [log_rotation.rs](file:///home/matt/src/daemoneye/src/daemon/utils/log_rotation.rs#L71-L81): `dup2` + unconditional `close`/drop closes fd 0/1/2 if standard fds were closed before launch.
  - [shell_label.rs](file:///home/matt/src/daemoneye/src/daemon/executor/shell_label.rs#L45-L67): fixed 16 KiB `getpwuid_r` buffer without `ERANGE` retry.

---

## 2. Functionality & Correctness

### Critical

1. **Multi-Turn Ghost Shells Broken Across All 3 AI Backends** — [turn_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/ghost/turn_helpers.rs#L452-L483), [anthropic.rs](file:///home/matt/src/daemoneye/src/ai/backends/anthropic.rs#L267-L280), [openai.rs](file:///home/matt/src/daemoneye/src/ai/backends/openai.rs#L229-L238), [gemini.rs](file:///home/matt/src/daemoneye/src/ai/backends/gemini.rs#L300-L317)
   - `persist_ghost_turn_message` writes both `tool_calls: Some(...)` and `tool_results: Some(...)` onto a **single** `Message`.
   - Every backend checks `if let Some(trs) = m.tool_results { ... } else if let Some(tcs) = m.tool_calls { ... }`, completely dropping `tool_calls` (and assistant text) and sending orphan `tool_results` on turn 2 — triggering HTTP 400 on Anthropic, OpenAI, and Gemini.
   - **Fix:** Emit two messages (`assistant` with `tool_calls`, then `user` with `tool_results`), matching [conversation_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/stream/conversation_helpers.rs#L604-L610).

2. **Ghost Shell Silently Swallows All `AiClient::chat` Errors & Reports `Complete`** — [turn.rs](file:///home/matt/src/daemoneye/src/daemon/ghost/turn.rs#L84-L91), [turn_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/ghost/turn_helpers.rs#L280)
   - When `client_clone.chat().await` returns `Err(e)`, the spawned task logs and drops `ai_tx` without sending `AiEvent::Error`. `collect_ghost_stream_turn` sees channel EOF (`None`), returns `Ok(("", vec![]))`, and marks the run `Completed` / `RunState::Complete` — also masking bug #1 above!
   - **Fix:** Forward `Err(e)` as `AiEvent::Error`, require `AiEvent::Done` for clean stream completion, and wrap the spawned task in `ChatTaskGuard` with `cancel.cancelled()` in `select!`.

3. **PTY Command Timeout Completely Bypassed on Continuous Output** — [client_read.rs](file:///home/matt/src/daemoneye/src/shell/client_read.rs#L191-L291)
   - `deadline` is only checked in the `Err(_)` branch of `tokio::time::timeout_at(slice_end, reader.read_until(...))`. Because `timeout_at` polls the inner future first, a command continuously emitting bytes (`yes`, `find /`, `tail -f`) keeps `read_until` immediately ready past `deadline`, never triggering `handle_read_timeout` or Ctrl-C while growing `buf` until OOM.
   - **Fix:** Check `Instant::now() >= slice_end` (and `>= deadline`) at the top of the loop before awaiting `read_until`, and bound `buf`.

4. **Anthropic Usage Parser Double-Subtracts Cache Tokens** — [anthropic.rs](file:///home/matt/src/daemoneye/src/ai/backends/anthropic.rs#L17-L35)
   - Anthropic's `message_start.message.usage.input_tokens` is *already* strictly uncached input tokens. Subtracting `cache_read` and `cache_write` saturates `input_tokens` to `0` on almost every cached turn, skewing cost reporting, daily budget caps, and `last_prompt_tokens` compaction triggers.
   - **Fix:** Use `input_tokens: total_input` directly without subtracting cache counts.

### High

5. **Transcript Viewer `wrap_words` Panics on Whitespace-Only Lines & Width-1 Wide Chars** — [viewer_layout.rs](file:///home/matt/src/daemoneye/src/cli/viewer_layout.rs#L219-L222), [L258-L266](file:///home/matt/src/daemoneye/src/cli/viewer_layout.rs#L258-L266)
   - On `"   "`, `rest.len() == 0` while `trailing_len == 3`, panicking at `rest.len() - trailing_len` before reaching the whitespace-only check on line 228. Also `space_remaining -= w` underflows on a 2-col CJK/emoji char at `width == 1`.

6. **Webhook Deduplication Suppresses `Resolved` Alerts, Stranding Active Alerts for 6 Hours** — [webhook/process.rs](file:///home/matt/src/daemoneye/src/webhook/process.rs#L52-L104)
   - `state.dedup` keys on `fingerprint` alone (without `status`). A `Resolved` alert arriving within 300s of `Firing` is discarded as a duplicate, leaving the alert stuck active until the 6-hour TTL.

7. **CircuitBreaker Failures: Never Trips on Mid-Stream Errors, Trips Globally on 4xx, Unbounded Half-Open Probes** — [ai/mod.rs](file:///home/matt/src/daemoneye/src/ai/mod.rs#L31-L89), [L371](file:///home/matt/src/daemoneye/src/ai/mod.rs#L371)
   - `send_with_retry` calls `record_success()` on HTTP 200 headers *before* SSE streaming, resetting `consecutive_failures` to 0 every turn so mid-stream errors can never reach threshold 5. Meanwhile, 4xx client errors trip the process-global breaker across all providers, and half-open `allow()` has no `probe_in_flight` gate.

8. **Consecutive `role: "user"` Messages Wedge Anthropic Sessions (HTTP 400)** — [conversation_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/stream/conversation_helpers.rs#L104-L126), [L589-L599](file:///home/matt/src/daemoneye/src/daemon/stream/conversation_helpers.rs#L589-L599), [endings.rs](file:///home/matt/src/daemoneye/src/daemon/stream/endings.rs#L112-L132), [anthropic.rs](file:///home/matt/src/daemoneye/src/ai/backends/anthropic.rs#L264-L318)
   - Cancel with empty response, empty final reply, `user_message_redirect` (which also drops results of earlier tools already run in the batch), `end_client_gone`, and ghost `push_user_note` all produce consecutive `user` messages without coalescing in `AnthropicClient::convert_messages`.

9. **Named Session Load Truncates Mid-Tool-Exchange** — [session_store.rs](file:///home/matt/src/daemoneye/src/session_store.rs#L273-L289)
   - `load_session_messages` runs `messages.drain(..len - max_count)` without `next_clean_turn_start` / `repair_tail_head` (unlike `read_session_file`), producing orphan `tool_results` on resume.

10. **Background Compaction Bugs (0s First Epoch, Wrong Model Window, `"before turn 0"`)** — [background.rs](file:///home/matt/src/daemoneye/src/daemon/context/background.rs#L95), [L135-L139](file:///home/matt/src/daemoneye/src/daemon/context/background.rs#L135-L139), [L204](file:///home/matt/src/daemoneye/src/daemon/context/background.rs#L204)
    - Uses `Utc::now()` instead of `started_at` for epoch 1 `span_start` (recording 0 commands/files/tokens/alerts), ignores the session's `/model` context window, and hardcodes `tail_first_turn = 0`.

11. **`AsyncStdin::read_line` Corrupts Non-ASCII UTF-8 as Latin-1** — [tty.rs](file:///home/matt/src/daemoneye/src/cli/input/tty.rs#L73-L81) (`b as char` per byte).

12. **SSE Pre-First-Token Retry Bugs** — [ai/backends/mod.rs](file:///home/matt/src/daemoneye/src/ai/backends/mod.rs#L193-L213), [gemini.rs](file:///home/matt/src/daemoneye/src/ai/backends/gemini.rs#L236-L240)
    - Transport errors fall through into `stall_retries` (5 retries total, last 2 with zero backoff); non-retriable provider errors retry twice; Gemini malformed function calls emit 3 duplicate `AiEvent::Error` frames.

### Medium & Low Correctness Bugs

- **DST midnight skip resets daily spend to `$0.00`:** `local_day_bounds` falls back to `(now, now)` on `LocalResult::None` ([spend.rs](file:///home/matt/src/daemoneye/src/daemon/spend.rs#L93-L109)).
- **`turn_inbox` missed on cold reload & leaked on early error exits** ([ask_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/server/ask_helpers.rs#L24-L44), [conversation.rs](file:///home/matt/src/daemoneye/src/daemon/stream/conversation.rs#L44-L115)).
- **Chapter context block prints epoch sequence IDs as `(turns lo–hi)`** + unchecked `e.turn_end - e.turn_start` ([epochs.rs](file:///home/matt/src/daemoneye/src/daemon/context/epochs.rs#L388), [L431-L436](file:///home/matt/src/daemoneye/src/daemon/context/epochs.rs#L431-L436)).
- **`recall_context` stemming fallback returns full untruncated message** + Unicode case-folding panic in `build_excerpt` ([recall.rs](file:///home/matt/src/daemoneye/src/daemon/context/recall.rs#L210-L256)).
- **OSC 7501 `decode_field` rejects unpadded base64** (`STANDARD` rejects missing `=` padding; [program_status.rs](file:///home/matt/src/daemoneye/src/shell/program_status.rs#L179-L184)).
- **Scheduler bugs:** `mark_done` resurrects `Cancelled` recurring jobs; `interval_secs == 0` loops forever under the write lock ([scheduler.rs](file:///home/matt/src/daemoneye/src/scheduler.rs#L89-L107), [L426-L484](file:///home/matt/src/daemoneye/src/scheduler.rs#L426-L484)).
- **Chat sandbox idle reaper TOCTOU & dropped retries** ([chat_sandbox.rs](file:///home/matt/src/daemoneye/src/daemon/executor/chat_sandbox.rs#L89-L114)); **PTY/sandbox shell leak on `await_ready`/`spawn_command` error** ([shell_exec.rs](file:///home/matt/src/daemoneye/src/daemon/executor/shell_exec.rs#L332-L339), [bg_shell.rs](file:///home/matt/src/daemoneye/src/daemon/executor/bg_shell.rs#L99-L145), [L243](file:///home/matt/src/daemoneye/src/daemon/executor/bg_shell.rs#L243)).
- **`save_session` wipes existing `tags` and `parent`** ([session_store.rs](file:///home/matt/src/daemoneye/src/session_store.rs#L217-L229)); **`index_memory_file` indexes already-expired memories** ([memory/index/mod.rs](file:///home/matt/src/daemoneye/src/memory/index/mod.rs#L640-L673)).
- **CLI/TUI bugs:** multi-span overflow in `wrap_line` ([render.rs](file:///home/matt/src/daemoneye/src/cli/render.rs#L144)); char-count instead of display-width wrapping + whitespace desync in `InputLine` ([editor.rs](file:///home/matt/src/daemoneye/src/cli/input/editor.rs#L190-L368)); unchecked `u16` SGR mouse math & dropped `[` in bracketed paste ([tty.rs](file:///home/matt/src/daemoneye/src/cli/input/tty.rs#L315-L394)); `/clear` & `/new` don't update status bar, transcript, or `held_presence` ([chat_loop.rs](file:///home/matt/src/daemoneye/src/cli/commands/chat_loop.rs#L149-L159)); `top_tui_loop` lacks panic restore and orders `enable_raw_mode()?` before guard creation ([top_tui_loop.rs](file:///home/matt/src/daemoneye/src/cli/commands/top_tui_loop.rs#L44-L58)).
- **Low:** `"error": null` aborts OpenAI/Gemini proxy streams ([openai.rs](file:///home/matt/src/daemoneye/src/ai/backends/openai.rs#L126), [gemini.rs](file:///home/matt/src/daemoneye/src/ai/backends/gemini.rs#L187)); `finish_command` drops counters after 6-slot eviction ([stats.rs](file:///home/matt/src/daemoneye/src/daemon/stats.rs#L215-L257)); modern OpenAI models (`gpt-5`, `gpt-4.1`, `o1`/`o3`/`o4`) default to 32k context window ([models.rs](file:///home/matt/src/daemoneye/src/config/models.rs#L190-L203)).

---

## 3. Performance & Concurrency

### High Impact

1. **Synchronous `tmux` Subprocess & OS-Thread Storm on Tokio Workers** — [startup_tasks.rs](file:///home/matt/src/daemoneye/src/daemon/startup_tasks.rs#L20-L34), [cache.rs](file:///home/matt/src/daemoneye/src/tmux/cache.rs#L194-L239), [process.rs](file:///home/matt/src/daemoneye/src/process.rs#L65-L133), [host.rs](file:///home/matt/src/daemoneye/src/daemon/utils/host.rs#L11-L27)
   - Every 2s, `SessionCache::refresh` runs synchronously on a Tokio worker, spawning $4 + N$ serial `tmux` subprocesses (capturing every home pane even when unchanged). Each call via `bounded_output` spawns 2 OS threads and polls with `std::thread::sleep(10ms)`.
   - On the prompt hot path, `get_labeled_context_scoped` and `get_pane_remote_host` run synchronous `tmux` calls (the latter with **no timeout at all**) directly on Tokio workers.
   - **Fix:** Move `refresh()` to `spawn_blocking` (or async `tokio::process::Command`), skip `capture_pane` when pane metadata is unchanged, and serve `get_pane_remote_host` from `SessionCache`.

2. **Non-WAL SQLite (`busy_timeout=0`, `synchronous=FULL`) + Re-Entrant Mutex Contention on Hot Paths** — [memory/index/mod.rs](file:///home/matt/src/daemoneye/src/memory/index/mod.rs#L122-L172), [L350-L401](file:///home/matt/src/daemoneye/src/memory/index/mod.rs#L350-L401), [event_log.rs](file:///home/matt/src/daemoneye/src/daemon/utils/event_log.rs#L10-L99)
   - SQLite opens with `DELETE` journal, `synchronous=FULL`, and `busy_timeout=0`. Whenever `INDEX_CONN.try_lock()` contends, `open_index()` opens a transient second connection that executes `PRAGMA user_version = 2` and immediately hits `SQLITE_BUSY` — dropping index updates silently.
   - `open_and_reconcile_if_empty` holds `INDEX_CONN` across `reconcile_corpus()` and `open_index()`, guaranteeing self-contention every time; if `reconcile_corpus` fails, it permanently inserts into `INDEX_RECONCILED_EMPTY`, disabling retries forever.
   - `log_event()` runs synchronously on Tokio workers (double-masking payloads + SQLite fsyncs). `append_archive_message` also still has the pre-append `metadata().len()` offset race ([session.rs](file:///home/matt/src/daemoneye/src/daemon/session.rs#L297-L300)).
   - **Fix:** Enable `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;`, drop `conn` before `reconcile_corpus`, and move `log_event`/FTS5 writes to a dedicated background writer channel.

3. **Long-Held Global Mutexes Across Disk Scans, Syscalls, & Fragile Lock Ordering** — [chat_presence.rs](file:///home/matt/src/daemoneye/src/daemon/server/chat_presence.rs#L26-L38), [bus/mod.rs](file:///home/matt/src/daemoneye/src/bus/mod.rs#L213-L248), [spend.rs](file:///home/matt/src/daemoneye/src/daemon/spend.rs#L128-L163), [registry.rs](file:///home/matt/src/daemoneye/src/shell/registry.rs#L710-L733)
   - `attach_brief_for` holds `SessionStore` across two full JSONL event-log scans (`build_catchup_brief`).
   - `Bus::publish` holds `publish_lock` + `subscribers` across synchronous `log_event`, `inject_into_sessions` file writes, and SQLite commits — establishing cross-lock chains (`publish_lock -> subscribers -> SessionStore` and `AlertRegistry -> PresenceRegistry`, opposite `SessionStore -> PresenceRegistry` in `cleanup_pass`).
   - `spend::LEDGER`, `webhook::dedup`, `ShellRegistry`, and `SessionStore` (in `handle_set_pane`) are all held across disk/flock I/O.

4. **Concurrent File Corruption Race in `ScheduleStore::save`** — [scheduler.rs](file:///home/matt/src/daemoneye/src/scheduler.rs#L313-L321)
   - Acquires a shared `RwLock::read()` and writes/renames a single fixed `.tmp` path without serialization or `sync_all()`, allowing concurrent callers to corrupt `schedules.json` or lose updates.

5. **Unbounded Buffers & $O(N)$ / $O(N^2)$ Scans on Hot Paths** — [memory/index/mod.rs](file:///home/matt/src/daemoneye/src/memory/index/mod.rs#L324-L340), [pty.rs](file:///home/matt/src/daemoneye/src/shell/pty.rs#L348-L381), [server/shells.rs](file:///home/matt/src/daemoneye/src/daemon/server/shells.rs#L527-L600), [ops.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/ops.rs#L315-L324)
   - `read_line_at_offset` scans linearly from byte 0 instead of `seek(SeekFrom::Start(offset))` — up to 100 full-file scans per `recall_context` on a Tokio worker.
   - `PtyShell::run` rescans `buf` from byte 0 on every 4 KiB chunk ($O(N^2)$).
   - `read_shell_log` reads the entire `.cast` file into RAM before applying its 4 MiB cap.
   - `ops.rs` / `ops_copy.rs` / `ops_delete.rs` call `read_to_string` on Tokio workers with no `is_file()` check or byte cap (unlike `read.rs`).

### Medium & Low Performance Issues

- **`ShellScreen::scrollback_rows(n)`** iterates all 10,000 scrollback rows under `PtyBackend.screen` mutex before slicing the last `n` ([screen.rs](file:///home/matt/src/daemoneye/src/shell/screen.rs#L191-L219)).
- **`program_notices_loop` spawns `sbx exec` every 2s per idle sandbox shell** because `SbxBackend::status()` always probes the VM even though OSC 7501 programs live in the local `PtyBackend` ([program_notices.rs](file:///home/matt/src/daemoneye/src/daemon/program_notices.rs#L148-L167), [sbx_backend.rs](file:///home/matt/src/daemoneye/src/shell/sbx_backend.rs#L85-L137)).
- **Blocking disk/FTS5 I/O on Tokio workers** across prompt assembly ([prompt.rs](file:///home/matt/src/daemoneye/src/daemon/prompt.rs#L73-L255)), memory tools, `handle_status`, `handle_shell_list` (parses `.meta.json` twice per shell), and `ask_helpers.rs` (reads session `.jsonl` twice; calls `read_epochs` 3 times including unused `let _chain`).
- **3x full `Vec<Message>` clones per turn** in `persist_final_turn_history` even when compaction isn't needed ([conversation_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/stream/conversation_helpers.rs#L176-L181)).
- **Resource leaks & unbounded queues:** unbounded presence `mpsc` + unreachable `discarded.clear()` ([chat_presence.rs](file:///home/matt/src/daemoneye/src/daemon/server/chat_presence.rs#L141-L160)); permanent OS reaper thread per shell ([launch.rs](file:///home/matt/src/daemoneye/src/shell/launch.rs#L228-L248)); `Arc<PtyBackend>` cycle with pump thread ([backend.rs](file:///home/matt/src/daemoneye/src/shell/backend.rs#L125-L178)); `std::thread::sleep` + `kill(1)` subprocess under `PtyBackend.shell` mutex ([pty.rs](file:///home/matt/src/daemoneye/src/shell/pty.rs#L437-L560)); dead `Bus` subscribers with rare filters never pruned ([bus/mod.rs](file:///home/matt/src/daemoneye/src/bus/mod.rs#L219-L248)).
- **Minor hot-loop wins:** missing indexes on `turns_map(session_id)` and `events_map(segment)`; 18+ sequential regex passes in `mask_sensitive` (use `RegexSet` pre-filter); backwards short-circuit `part_counts_as_token(v) && !*first_token_seen` in [gemini.rs](file:///home/matt/src/daemoneye/src/ai/backends/gemini.rs#L198); `O(L^2)` `drain` in `SseBuffer::next_data` ([ai/mod.rs](file:///home/matt/src/daemoneye/src/ai/mod.rs#L245-L255)); uncached `/proc/sys/kernel/hostname` read per TUI frame ([host.rs](file:///home/matt/src/daemoneye/src/daemon/utils/host.rs#L2-L7)).

---

## 4. Architecture & Coupling

1. **20 Bidirectional Production Module Cycles:**
   - `daemon` acts as both top-level orchestrator and shared utility bag imported by `shell`, `bus`, `memory`, `runbook`, `scripts`, `scheduler`, `cost`, `agents`, `tmux`, `search`, `webhook`, `cli`, and `config`.
   - Upward inversions include `daemon -> cli` ([ops.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/ops.rs#L447), [shell_screen.rs](file:///home/matt/src/daemoneye/src/daemon/executor/knowledge/shell_screen.rs#L446)), `config -> cli/ai/agents/host`, `shell -> daemon`, `bus -> daemon`, and `process -> tmux::ansi`. `src/ai/` is the cleanest isolated layer.
2. **Half-Migrated `DaemonState` vs. ~28 Mutable Global Statics:**
   - [DaemonState](file:///home/matt/src/daemoneye/src/daemon/state.rs#L21-L32) itself is clean (10 fields), but **254 production call sites** inside `daemon/` and `webhook/` bypass it to read global singletons (`HANDLE`, `REGISTRY`, `BUS`, `ACTIVE_ALERTS`, `GLOBAL_METRICS`, `CHAT_PRESENCE`), while `run/{active,cancel,pause}` and `spend::LEDGER` live outside `DaemonState` entirely.
   - `spawn_webhook_task` ([startup_tasks.rs](file:///home/matt/src/daemoneye/src/daemon/startup_tasks.rs#L82-L102)) wraps a single-use `Option<TcpListener>` in `supervise()`, causing a 30s infinite restart spin-loop if `webhook::serve` ever exits.
3. **Divergent Execution Loops & Leaky Executor Abstraction:**
   - Ghost spawning is duplicated across **5 sites** with inconsistent lifecycle events and a TOCTOU race on `max_concurrent_ghosts`.
   - `execute_tool_call<W, R>` is coupled to raw IPC sockets, forcing ghosts to pass `tokio::io::sink()` / `tokio::io::empty()` ([turn_helpers.rs](file:///home/matt/src/daemoneye/src/daemon/ghost/turn_helpers.rs#L369-L370)).
   - `PendingCall` repeats `id` and `thought_signature` on all 45 variants (plus dead `Background._credential`), and `route.rs` -> `dispatch_*.rs` double-matches with runtime `bail!` fallbacks.
4. **IPC Protocol Debt:**
   - Unversioned wire enums; dead/asymmetric variants (`PaneSelectPrompt`/`PaneSelectResponse` have no daemon side; `ResetSessionToolCount` has no CLI sender; `ScheduleList`/`ScriptList`/`RunbookList` are emitted by daemon and discarded by CLI; `Ask.model` is ignored as `_ask_model`); `handle_resume_run` reports errors via `SystemMsg` + `Ok` instead of `Response::Error`.
5. **Material Documentation Drift (`AGENTS.md`, `CLAUDE.md`, `docs/architecture.md`):**
   - Wrong socket path (`~/.daemoneye/run/daemoneye.sock` vs actual `~/.daemoneye/var/run/daemoneye.sock`), removed globals (`BG_DONE_TX`, `FG_HOOK_COUNTER`, `BUFFER_COUNTER`), removed `notify` subcommand, outdated `read_file`/`edit_file` `~/.daemoneye/` rules, stale "tmux is default / M11 active" text, and broken "`src/tmux/` is the only place calling `tmux`" invariant (7 external files call `Command::new("tmux")`, 4 unbounded).

---

## 5. Code Quality, Maintainability & Testing

### Production Panic Hazards & Error Swallowing
- **False-invariant `.expect()` on empty `[models]`** ([load.rs](file:///home/matt/src/daemoneye/src/config/load.rs#L202-L225)): `parse_config_text()` never checks `models.is_empty()`, so an empty `[models]` table panics `resolve_model()`.
- **Unchecked UTF-8 byte-slice panics:** [manifest.rs](file:///home/matt/src/daemoneye/src/manifest.rs#L387), [scripts.rs](file:///home/matt/src/daemoneye/src/scripts.rs#L119-L120), [sys_context.rs](file:///home/matt/src/daemoneye/src/sys_context.rs#L94-L95), [scheduler.rs](file:///home/matt/src/daemoneye/src/scheduler.rs#L337) (`&id[..8]`), [executor/schedule.rs](file:///home/matt/src/daemoneye/src/daemon/executor/schedule.rs#L197) — all should use `crate::util::truncate_chars()`.
- **Silent persistence error drops:** `let _ = self.save();` in [scheduler.rs](file:///home/matt/src/daemoneye/src/scheduler.rs#L420) (`420, 483, 509, 552`); `meta.toml` write ignored in `rename_session` ([session_store.rs](file:///home/matt/src/daemoneye/src/session_store.rs#L335-L336)); partial log rotation leaves `daemon.log` missing without reattaching fds ([log_rotation.rs](file:///home/matt/src/daemoneye/src/daemon/utils/log_rotation.rs#L41-L48)); non-atomic silent write in [pane_prefs.rs](file:///home/matt/src/daemoneye/src/pane_prefs.rs#L51-L56).

### Safety-Gate Blind Spots
- Both [tests/lock_invariant.rs](file:///home/matt/src/daemoneye/tests/lock_invariant.rs#L19) and [tests/tmux_read_only.rs](file:///home/matt/src/daemoneye/tests/tmux_read_only.rs#L185) split files on the first `#[cfg(test)]` string — **blinding both gates to all production code below top-of-file `#[cfg(test)]` items in ~30 files** (including `ghost/turn.rs`, `event_log.rs`, `notice_sinks.rs`, `shell/registry.rs`, `shell/pty.rs`).
- `lock_invariant.rs` also misses `.read()`, `.write()`, `.try_lock()`, `.try_read()`, and `.try_write()` (though a full scan confirms **zero** current production violations). Both gates should adopt `tests/hygiene.rs`'s `syn` AST traversal.

### Maintainability & Dependency Hygiene
- **Positional argument explosion:** 38 production functions suppress `clippy::too_many_arguments`, peaking at **24 params** (18 `usize`) and **22 params** (15 `usize`) in [status_sections.rs](file:///home/matt/src/daemoneye/src/cli/status_sections.rs#L6-L30) / [L206-L232](file:///home/matt/src/daemoneye/src/cli/status_sections.rs#L206-L232) — fixable by boxing `DaemonStatus` as a struct.
- **Dependencies (`Cargo.toml`):** `base64 = "0.23"` contradicts its own comment (`0.22` to avoid duplicate) and pulls both `0.22` + `0.23`; single-use `nix = "0.31.3"` (only for `Flock` in [instance.rs](file:///home/matt/src/daemoneye/src/daemon/instance.rs#L11-L12)) pulls 3 `nix` versions into `Cargo.lock` and can be replaced with `std::fs::File::try_lock()` (stable since Rust 1.89). Dead `#[allow(dead_code)] fn search_dir` remains in [search.rs](file:///home/matt/src/daemoneye/src/search.rs#L575).

### Test Suite Hazards & Coverage Blind Spots
- **17 broken no-op `with_home(&tmp, || {});` tests** in [read_tests.rs](file:///home/matt/src/daemoneye/src/daemon/executor/file_ops/read_tests.rs#L49) drop `TestHomeGuard` before awaiting `run_read_file()`, creating concurrent `getenv`/`setenv` UB against parallel tests.
- **`TestHomeGuard` (`std::sync::MutexGuard`) held across `.await` on a `multi_thread` Tokio runtime** in [launch.rs](file:///home/matt/src/daemoneye/src/shell/launch.rs#L413-L445) (non-owner unlock UB + leaked thread-local `HOLDS_HOME_GUARD`).
- **Destructive env cleanup** permanently removes `$SHELL` ([shell_exec_tests.rs](file:///home/matt/src/daemoneye/src/daemon/executor/shell_exec_tests.rs#L177-L194), [session_tests.rs](file:///home/matt/src/daemoneye/src/shell/session_tests.rs#L484-L543)) and `$TMUX`/`$TMUX_PANE` ([isolation.rs](file:///home/matt/src/daemoneye/tests/isolation.rs#L321-L326)).
- **Flaky unit test:** [`host_sends_lagged_when_subscriber_falls_behind`](file:///home/matt/src/daemoneye/src/shell/host.rs#L760-L826) fails under load (reproduced on our full test run).
- **Zero direct unit tests** on core orchestration modules: `cli/commands/stream_ask.rs` + `stream_prompts.rs` (1,402 LOC), `daemon/stream/conversation.rs` + `conversation_helpers.rs` (817 LOC), `daemon/server/ask.rs` + `ask_helpers.rs` (676 LOC), `daemon/server/handlers.rs` (632 LOC), `daemon/executor/route.rs` (257 LOC), and `tmux/{pane,window}.rs` (302 LOC).

---

## 6. Prioritized Remediation Roadmap

### P0 — Immediate (Security, Data Integrity, Broken Core Flows)
1. Escape `#` + add `-l`/`--` in `notify_chat_panes` ([process.rs](file:///home/matt/src/daemoneye/src/webhook/process.rs#L252)).
2. Fix `open_shell` ghost sandbox check + `Owner::Ghost`, and `send_input` `ShellId::parse` fail-closed + `HostId` propagation + approval gate.
3. Harden Ghost sudoers (`sha256:` or root-owned script copy), `GhostPolicy::is_safe` path validation, `wrap_remote` `ssh` bypass, and `command_has_sudo`.
4. Fix Ghost two-message `tool_calls`/`tool_results` persistence and error propagation (`AiEvent::Error` + `saw_done` + `ChatTaskGuard` + `cancel`).
5. Fix PTY `read_to_marker` deadline check before `read_until` + output buffer cap, and fix `shell/host.rs:181` `&line[offset..end]`.
6. Fix Anthropic `parse_anthropic_start_usage` cache double-subtraction.
7. Fix production panics: `wrap_words` whitespace-only underflow, empty `[models]` validation, UTF-8 byte-slicing (`truncate_chars`), and `build_excerpt` clamping.
8. Fix `ScheduleStore::save` concurrent `.tmp` race and SQLite `WAL` + `busy_timeout=5000` + `open_and_reconcile_if_empty` self-contention.
9. Strip API keys from spawned shell/command environments and fix `file_ops` canonical symlink guards + atomic `O_EXCL` `0600` temp files.

### P1 — Next Sprint (Reliability, Concurrency, Gate Hardening)
1. Fix `CircuitBreaker` (record success post-stream, don't trip on 4xx, scope per provider/endpoint, single half-open probe) and `drive_sse_stream` retry classification.
2. Coalesce consecutive same-role messages in `AnthropicClient::convert_messages` and fix `load_session_messages` turn-boundary repair.
3. Fix webhook dedup key `(fingerprint, status)`, pre-auth middleware + constant-time Bearer check, dynamic `config::current()`, and ghost capacity reservation.
4. Fix background compaction (`started_at`, active model window, `tail_first_turn`), `read_line_at_offset` `seek()`, `scrollback_rows(n)` bound, and `program_notices` VM-probe avoidance.
5. Offload `SessionCache::refresh`, `log_event`/FTS5 writes, and `build_catchup_brief` off Tokio workers and out of held global locks.
6. Fix `filter.rs` `$1` capture replacement, `AsyncStdin` UTF-8 decoding, `OSC 7501` unpadded base64, and CLI/TUI bugs (`wrap_line`, `InputLine`, `/clear`, `top_tui_loop`).
7. Upgrade `tests/lock_invariant.rs` and `tests/tmux_read_only.rs` to `syn` AST traversal; fix the 17 `read_tests.rs` `with_home` races, `launch.rs` multi-thread `TestHomeGuard`, and `host_sends_lagged` test flake.

### P2 — Structural Architecture & Maintainability
1. Finish `DaemonState` consolidation and retire duplicate global singletons from daemon/webhook paths.
2. Extract a low-level domain/storage layer (`run` records, `event_log`, `PromptKind`, `InstanceLock`, `sbx` argv) to break the 20 module cycles; remove `daemon -> cli` and `config -> cli` inversions.
3. Decouple `executor` from raw IPC streams via a `ToolInteraction` trait and bounded framing helper; unify the 5 ghost spawners.
4. Box `DaemonStatus` to eliminate 22/24-parameter functions; clean up dead IPC variants and `PendingCall` boilerplate; drop `nix` for `File::try_lock()`; re-sync `AGENTS.md`, `CLAUDE.md`, and `docs/architecture.md`.
