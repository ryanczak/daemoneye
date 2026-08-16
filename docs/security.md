# DaemonEye security model

How DaemonEye protects itself against local attackers, what is and is not a
security boundary, and what each guard actually does.

## Trust model

DaemonEye runs as the logged-in user. Its Unix socket, CLI, and tmux hooks all
assume that user. The agent is a regular user-space process: it can run
commands, read files, and write files as you. The controls below raise the
cost of abusing that privilege; they do **not** turn it into a sandbox.

Two kinds of attacker matter:

1. **Another local user** on a multi-user box (shared workstation, corp
   laptop, CI host). They previously had full control via the socket — now
   blocked by peer identity (below).
2. **The AI/model itself being tricked** (prompt injection from captured
   terminal output). The only real defense here is human approval of
   sensitive tools; masking and path guards just slow a runaway model down.

DaemonEye is safe to use by default as a single-user tool. On genuinely
multi-user machines, treat the daemon as "one powerful account" and rely on
peer-auth + file permissions, not on the model's judgment.

---

## Guards, in order of strength

### 1. IPC peer authentication (primary boundary)

`src/daemon/server/mod.rs` reads `SO_PEERCRED` via `libc::getsockopt` on every
incoming connection **before parsing any bytes**. The connecting process's
effective UID must match the daemon's. Any other process — even one that can
create a socket at the same path — is refused.

This is what makes `Request::Shutdown`, `Request::Ask`, and the approval-gate
replies (`ToolCallResponse`, `CredentialResponse`) safe: "approved" can only
come from your own processes.

The socket itself is chmod 0700 (defense-in-depth; tmux runs `daemoneye`
notifications as the same user so nothing legitimate needs other-user access).

### 2. Filesystem lockdown

`main()` calls `libc::umask(0o077)` and `~/.daemoneye/` is created and
maintained at dirs `0700` / files `0600` (executables keep their owner-execute
bit via `0o100`) by `lockdown_permissions()` in `src/config/seeds.rs`
(`Config::ensure_dirs()`). The whole `~/.daemoneye/` tree is private to the
owner:

- `etc/config.toml` (+ `.bak`) — API keys — `0600`
- `var/log/sessions/*.jsonl` — full transcripts (masked data is stored here:
  masking applies to what is *sent to the model*, the transcripts retain raw
  text) — `0600`
- `var/log/daemon.log` — raw command strings — `0600`
- `scripts/` , `memory/` , `var/index/memory.db` (FTS5), runbooks,
  schedules — all `0700`/`0600`

Because everything is in one private tree and every file writer uses `0600`
from the umask, no separate writer-level chmod is needed. If you add a new
top-level path, create it under `var/`.

### 3. Webhook fail-closed

The HTTP listener (port 9393, opt-in via `enabled:true`) rejects a startup
configuration that would expose an unauthenticated endpoint: **non-loopback
bind + empty/absent `secret` → daemon refuses to start.** Loopback with no
secret is allowed (it's identical to the IPC trust shell) but logs a warning.

`bind_addr: "0.0.0.0"` etc. always requires a secret (ideally shared via an
HTTPS-terminating proxy in front of axum). Every request to the webhook now
beats a per-IP fixed-window rate limit (default 30 req/60 s), since each one
can trigger a ghost-shell AI burn.

### 4. Ghost trigger strictness

`evaluate_watchdog_response` acts **only** on an explicit, final
`GHOST_TRIGGER: YES` line. The old fallback that treated any response
containing an uppercase `ALERT` substring as a trigger is gone — it fired on
honest-looking false positives such as "no ALERT condition present". If a
response lacks the marker the daemon refuses to act (and logs why).

### 5. Temp files

`install_sudoers` writes to `~/.daemoneye/var/run/sudoers-<pid>.tmp` with
`O_CREAT|O_EXCL` and mode `0600`, never `/tmp` — eliminating the symlink
race and other-users' read/tamper window. The `sudo install` + `visudo -c -f`
steps are unchanged.

### 6. Shell-string hygiene

- Every subprocess invocation uses argv arrays — no `sh -c` anywhere
  (tmux, background, foreground, remote panes).
- Pane-remote shell strings single-quote interpolated values and escape `'`
  (`sq_escape`). A newline inside single quotes cannot break out (verified).
- `read_file`/`edit_file` reject control characters in paths/patterns.
- `read_file`/`edit_file` block well-known credential paths (not a boundary —
  see note below).
- tmux hook bodies single-quote `#{{session_name}}`/`#{{pane_id}}`, and
  `daemoneye notify` re-validates session names at runtime (rejects `'` and
  control characters) as defense-in-depth against hostile session names.

Sources: `src/daemon/executor/file_ops/mod.rs`, `src/daemon/executor/foreground.rs`,
`src/daemon/mod.rs` (global hooks), `src/cli/notify.rs`.

### 7. Masking (AI egress only)

`src/ai/filter.rs` replaces matching secrets before anything is sent to the
model; `src/daemon/utils/event_log.rs` masks `events.jsonl`. **Session
transcripts and daemon.log are NOT masked** — they are private (see §2) but
contain unmasked text. If you need a scrubbed export, don't rely on the log
files.

---

## What is deliberately NOT a boundary

- **`read_file` credential block** — the model can still read any file through
  `run_terminal_command` (which is why that tool is approval-gated). The
  credential blocklist only removes the *silent* easy path so keys don't end up
  in session output by accident. It is not a sandbox.
- **Path guards on read/edit** — they stop `..` traversal and control chars;
  the model can still reach anywhere it knows about via the terminal tool.
  Guarded paths are documented in the controls, not in code comments only.
- **The approval gate** — trust that "the daemon's own user approved". It is
  implemented over the socket; with SO_PEERCRED in front of it, only your
  processes can answer. It doesn't protect you from yourself.
- **Network-proxied socket** — do not expose the Unix socket; its auth is
  identity-based (peer UID), which does not survive OS isolation hops.

---

## Adding an AI tool: keep these in mind

- New read-like tools should go through the same path guards
  (`src/daemon/executor/file_ops/mod.rs`).
- New tools that can run arbitrary commands are approval-gated by default
  (`should_emit_tool_feedback()` returns `false`).
- If a tool produces shell strings for a remote pane, single-quote + `sq_escape`
  are not optional.
- See the 8-step guide in AGENTS.md.
