# DaemonEye Development Priorities

## Completed

### Daemon & Reliability
- **Daemonization** — `fork()` before tokio runtime; parent prints PID and exits; child calls
  `setsid()` and redirects stdin from `/dev/null`.
- **Foreground command completion detection** — `idle_cmd` baseline captured before
  `send_keys`; poll loop requires `pane_current_command == idle_cmd` AND two consecutive
  identical `capture_pane` snapshots before declaring done. Prevents false positives with
  subshells (e.g. `bash script.sh`).
- **Background command timeout** — 30 s `tokio::time::timeout` wraps `child.wait()`;
  on expiry `child.kill().await` is called and stdout/stderr drain tasks are aborted.
- **Background command resource limits** — `pre_exec` hook sets `RLIMIT_AS` = 512 MiB and
  `RLIMIT_NOFILE` = 256 before `exec`.
- **Mutex panic recovery** — All `sessions.lock()` and `cache.active_pane.read()` calls use
  `.unwrap_or_else(|e| e.into_inner())` to recover from poisoned locks instead of panicking.
- **Background command output trimming** — `normalize_output()` strips trailing whitespace
  per line and leading/trailing blank lines; applied uniformly to both execution paths.

### Security
- **Sensitive data masking completeness + user extension** — Built-in patterns: AWS key IDs,
  PEM private key blocks, GCP JSON `"private_key"` fields, JWTs, GitHub PATs (classic and
  fine-grained), database/broker connection URLs, password/token/API-key assignments, URL
  query-param secrets, credit cards, SSNs. 16 unit tests. `MaskingConfig` added to
  `config.rs`; `[masking] extra_patterns` in `config.toml` lets users append org-specific
  patterns — built-in patterns always run, never replaceable.

### Chat Interface
- **Syntax highlighting** — Fenced code blocks highlighted with per-language keyword sets
  (Bash, Python, Rust, JS/TS, Go, Java, SQL), comment styles, string literals, and numerics.
- **Session / turn indicator** — Dim full-width separator after each AI response showing
  turn number and prior message count.
- **System notification coloring** — `SystemMsg` responses rendered with amber ⚙ prefix.
- **AI prose tint** — AI response text rendered in bright-white for visual separation.
- **Structured tool-call output panel** — `ToolResult` shown in a dimmed bordered panel,
  capped at 10 rows with a truncation indicator. All border width calculations use
  `visual_len()` (strips ANSI, counts Unicode code points) to handle multi-byte box-drawing
  characters correctly.
- **Width-adaptive header + pane auto-resize** — Chat pane resized to 40% of window width
  (min 80 cols) on startup using `tmux resize-pane`. Header width and all separators
  derived from `query_pane_width` to avoid `TIOCGWINSZ` race conditions.
- **`/clear` in-session command** — Generates a new session ID so the next message starts
  a clean context; prints a dim separator line.

---

## Recommended Order of Operations

### ~~1. Client socket timeout~~ ✅
~~No deadline on `connect()` or `recv()`. A hung daemon silently blocks the client forever.~~
- ~~`connect()`: wrap `UnixStream::connect()` in a 5 s `tokio::time::timeout`.~~
- ~~`ask_with_session()`: wrap the streaming `recv()` in a 60 s inter-token timeout so a
  daemon that stops responding mid-stream produces a clear error rather than hanging.~~

### ~~2. Environment awareness in context~~ ✅
*(Requested by integrated AI — see AI_ASSISTANT_IMPROVEMENTS.md §4)*
Add an `environment` field to `config.toml` (values: `personal`, `development`, `staging`,
`production`). Include it in the system context injected into the first-turn prompt so the
AI can calibrate caution, blast-radius assessment, and security posture accordingly.
- `config.rs`: add `environment: String` (default `"personal"`) to `AiConfig` or a new
  `[context]` section.
- `daemon.rs` / `sys_context.rs`: prepend `Environment: <value>` to the host-context block.
- Minimal change; high signal-to-noise ratio for every response the AI gives.

### ~~3. Active pane labeling in context~~ ✅
*(Requested by integrated AI — see AI_ASSISTANT_IMPROVEMENTS.md §2)*
When the daemon assembles multi-pane context, the pane matched by `DAEMONEYE_SOURCE_PANE` is not
explicitly tagged as "active" in the text sent to the AI. Add a clear `[ACTIVE PANE]` marker
so the AI can immediately identify the user's current focus and weight its interpretation
accordingly.
- `daemon.rs`: when iterating cached pane summaries, prefix the source pane's block with
  `[ACTIVE PANE]` and others with `[BACKGROUND PANE <id>]`.

### ~~4. Structured command failure feedback~~ ✅
*(Requested by integrated AI — see AI_ASSISTANT_IMPROVEMENTS.md §3)*
Background command results are returned to the AI as raw stdout/stderr text. When a command
exits non-zero, prepend a structured header so the AI can immediately classify the failure
without parsing raw output:
```
exit 127 · command not found
--- output ---
bash: foo: command not found
```
Classification map (daemon-side, no IPC changes): exit 1 → generic failure; 126 → permission
denied; 127 → command not found; 124/timeout → timed out; 130 → interrupted. Anything else
reports the raw exit code.

### ~~5. Prompt library UI (FR-1.3)~~ ✅
Config and file loading for `~/.daemoneye/prompts/` are implemented but prompts cannot be
discovered or selected from the chat interface. Two pieces needed:
- `daemoneye prompts` subcommand that lists available prompts with descriptions.
- `/prompt <name>` in-chat command that starts a new session with the named prompt as the
  system message (combines `/clear` + system-prompt swap).

### ~~6. System context refresh on demand~~ ✅
~~`sys_context` is collected once at daemon startup via `OnceLock`. If the user changes
hostname, logged-in user, or working directory after starting the daemon the AI sees stale
context.~~ `/refresh` in-chat command re-collects OS info, memory, processes, and shell
history via the daemon (`Request::Refresh` IPC); starts a new session so the fresh context
is picked up immediately. `OnceLock` replaced with `RwLock<Option<SystemContext>>` for
mutability.

### 7. Plugin architecture (FR-1.5)
Largest remaining unimplemented requirement. Minimal first pass: executables in
`~/.daemoneye/plugins/` that receive prompt lifecycle events (prompt sent, response received,
tool call approved/rejected) over stdin/stdout as newline-delimited JSON. Unlocks community
extensions without a full API.

### 8. Proactive diagnostic context acquisition
*(Requested by integrated AI — see AI_ASSISTANT_IMPROVEMENTS.md §1)*
The AI must currently ask the user to run diagnostic commands manually; with background
execution it can already trigger them, but the prompt and tool schema don't make this pattern
explicit. After the plugin architecture lands, a "diagnostic context" plugin could pre-seed
the conversation with targeted output (`ss -tulnp`, `/etc/resolv.conf`,
`kubectl describe pod`, etc.) based on detected keywords in the initial query.
Depends on §7.

### ~~9. Message history memory~~ ✅
~~`sessions` clones the full history `Vec` on every turn.~~ Session history now persists
to `~/.daemoneye/sessions/<id>.jsonl` after each completed turn. On the next turn within the
same daemon run the in-memory `HashMap` is hit first (O(1)); after a daemon restart the
file is loaded instead, so conversation context survives restarts. `MAX_HISTORY` hoisted
to module level and applied at both load and write time.

### ~~10. OpenAI tool-call final-flush edge case~~ ✅
~~If an OpenAI stream ends with a pending tool-call accumulation buffer, the call is silently
dropped.~~ Explicit flush after the `'outer` stream loop ensures the last buffered tool call
is always emitted before `AiEvent::Done`.

### ~~11. Session ID fallback~~ ✅
~~`new_session_id()` falls back to 16 zeros if `/dev/urandom` is unreadable.~~ Now returns
immediately on a successful `/dev/urandom` read; on failure falls back to
`subsec_nanos ^ PID` mixed with a Knuth multiplicative hash of the PID — non-zero and
non-trivially non-predictable.

### ~~12. Sudo i18n~~ ✅
~~Current password-prompt detection matches English `[sudo] password for`.~~ Replaced
`capture-pane` text matching with `pane_current_command == "sudo"` — locale-independent
and eliminates the `capture_pane` call in the hot poll loop.
