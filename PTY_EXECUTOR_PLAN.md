# PTY-Based Background Executor — Implementation Plan

## 1. Motivation

The current background executor (`tokio::process::Command` with piped stdio) cannot handle commands that require a pseudo-terminal:

- `sudo` commands that prompt for a password (when the agent holds session-level approval but no NOPASSWD rule is set)
- `su` and `newgrp` — always require a terminal for credential entry
- `ssh` — host-key confirmation prompts, interactive login (if password-auth is used)
- `gpg`, `age`, `openssl` — passphrase prompts
- Any CLI that uses `isatty(3)` to detect interactive mode and refuses to run without a TTY

Without a PTY, these commands either hang indefinitely or fail with "pseudo-terminal will not be allocated". The daemon kills them after the 30-second timeout, and the AI receives an unhelpful error.

A PTY-based executor solves this by:

1. Allocating a PTY master/slave pair for each background command.
2. Running the subprocess with the PTY slave as its controlling terminal.
3. Monitoring the master side output for prompt patterns (password prompts, host-key confirmations, yes/no questions).
4. Emitting structured IPC events (`CredentialPrompt`, `ConfirmationPrompt`) when a prompt is detected.
5. Injecting the user's response back into the PTY master.

This is the foundation for autonomous runbook execution: once the daemon can handle interactive prompts programmatically, long-lived automated workflows can run without blocking on user presence.

---

## 2. Design Principles

- **Daemon-only**: The PTY executor is a private implementation detail of `daemon.rs`. The IPC protocol (`Request`/`Response`) changes minimally; clients that do not implement the new response variants continue to work with the existing approval flow.
- **Drop-in for background mode**: The PTY path replaces the `tokio::process::Command` block for `background=true` commands. The `background=false` (foreground / tmux send-keys) path is unchanged.
- **Opt-in per command class**: An AI system-prompt flag will be added to indicate whether a command should use the PTY path. For now, all background commands use PTY; the old pipe-based path is preserved as a fallback for non-interactive commands where PTY overhead is undesirable.
- **No sudo password sent over IPC for PTY commands**: When the PTY executor detects a sudo password prompt, it emits `Response::CredentialPrompt` and waits for `Request::CredentialResponse`. The client reads the password with echo disabled and sends it back — identical to the existing `SudoPrompt`/`SudoPassword` flow, reusing the same security model.
- **Resource limits preserved**: The `pre_exec` hook for `RLIMIT_AS` and `RLIMIT_NOFILE` is retained on the child.

---

## 3. Architecture

```
daemon.rs (handle_client)
    │
    │  background=true
    ▼
pty_exec::run_pty_command(cmd, tx, rx, timeout)
    │
    ├─ nix::pty::openpty()  → master_fd + slave_fd
    ├─ fork/exec child with slave as stdin/stdout/stderr + setsid + TIOCSCTTY
    ├─ close slave_fd in parent
    │
    ├─── PtyReader task: reads master_fd → output buffer
    │        │
    │        └─ PromptDetector::check(&buf) → Option<PromptEvent>
    │               │
    │               ├─ CredentialPrompt  → send Response::CredentialPrompt
    │               │                      await Request::CredentialResponse
    │               │                      write response to master_fd
    │               │
    │               └─ ConfirmationPrompt → send Response::ConfirmationPrompt
    │                                       await Request::ConfirmationResponse
    │                                       write "yes\n" or "no\n" to master_fd
    │
    ├─── Timeout task: tokio::time::sleep(timeout) → kill child
    │
    └─── Wait task: child.wait() → ExitStatus
         │
         └─ return (combined_output, ExitStatus) to handle_client
```

---

## 4. New IPC Variants

Two new `Response` variants and two new `Request` variants are added. Existing variants are unchanged.

### `ipc.rs` additions

```rust
// ── New Response variants ────────────────────────────────────────────────────

/// The background PTY command is waiting for a credential (password / passphrase).
/// The client MUST prompt the user with echo disabled and return a
/// `Request::CredentialResponse`.
CredentialPrompt {
    id: String,
    /// Human-readable label to display in the chat UI, e.g.
    /// "[sudo] password for alice:" or "Enter passphrase for key 'id_ed25519':"
    prompt: String,
},

/// The background PTY command is waiting for a yes/no confirmation, e.g.
/// an SSH host-key fingerprint check.
/// The client MUST display the `message` and return a
/// `Request::ConfirmationResponse`.
ConfirmationPrompt {
    id: String,
    /// The full prompt text from the subprocess, e.g.
    /// "The authenticity of host 'example.com' can't be established.\n..."
    message: String,
},

// ── New Request variants ─────────────────────────────────────────────────────

/// User-supplied credential in response to `Response::CredentialPrompt`.
/// The daemon injects `credential + "\n"` into the PTY master.
CredentialResponse { id: String, credential: String },

/// User's yes/no decision in response to `Response::ConfirmationPrompt`.
/// `accepted = true` → "yes\n" injected; `false` → "no\n".
ConfirmationResponse { id: String, accepted: bool },
```

### Serialisation contract

All four new variants use the existing newline-delimited JSON-over-Unix-socket protocol. No framing changes are required.

---

## 5. Prompt Detection

Prompt patterns are compiled once at daemon startup and stored in a `PromptDetector` struct.

### Pattern classes

| Class | Example output to match |
|---|---|
| sudo password | `[sudo] password for alice:` |
| su password | `Password:` (after `su` in command) |
| SSH host key | `Are you sure you want to continue connecting (yes/no/[fingerprint])?` |
| SSH password | `alice@host's password:` |
| GPG/OpenSSH passphrase | `Enter passphrase for key '/home/alice/.ssh/id_ed25519':` |
| generic password | `Password:` at end of output line |
| yes/no question | `Continue? [y/N]`, `Proceed? (yes/no)` |

### Detection rules

- Prompt detection is performed on each chunk of output read from the master PTY.
- A match fires only when the chunk ends with a recognised prompt suffix (not in the middle of a line), preventing false positives from output that merely mentions passwords.
- After injecting a credential, the detector's state resets so it can detect subsequent prompts (e.g., a wrong password that causes a re-prompt).
- A maximum of **3 credential attempts** per command is enforced; on the fourth failure the command is killed and the AI receives an error.

### Implementation

```rust
// src/pty_exec/prompt.rs

use regex::Regex;
use std::sync::OnceLock;

pub enum PromptEvent {
    Credential { label: String },
    Confirmation { message: String },
}

pub struct PromptDetector {
    attempt_count: u32,
}

impl PromptDetector {
    pub fn new() -> Self { Self { attempt_count: 0 } }

    /// Check the latest output chunk for an interactive prompt.
    /// Returns `Some(PromptEvent)` if one is detected, `None` otherwise.
    pub fn check(&mut self, buf: &str) -> Option<PromptEvent> { ... }
}

fn credential_patterns() -> &'static [Regex] {
    static PATS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        [
            r"(?i)\[sudo\] password for \S+:\s*$",
            r"(?i)^Password:\s*$",
            r"(?i)Enter passphrase for .*:\s*$",
            r"(?i)\S+@\S+'s password:\s*$",
            r"(?i)^Enter password:\s*$",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

fn confirmation_patterns() -> &'static [Regex] {
    static PATS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        [
            r"(?i)Are you sure you want to continue connecting \(yes/no",
            r"(?i)\(yes/no\)\s*[?:]?\s*$",
            r"(?i)\[y/N\]\s*[?:]?\s*$",
            r"(?i)Proceed\? \(yes/no\)",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}
```

---

## 6. PTY Executor Module

### File structure

```
src/
└── pty_exec/
    ├── mod.rs       # run_pty_command() — public entry point
    └── prompt.rs    # PromptDetector, PromptEvent
```

### `src/pty_exec/mod.rs`

```rust
use nix::pty::{openpty, OpenptyResult};
use nix::unistd::{close, dup2, setsid};
use std::os::unix::io::{FromRawFd, RawFd};
use tokio::io::unix::AsyncFd;
use tokio::time::{timeout, Duration};

pub struct PtyResult {
    pub output: String,
    pub exit_code: i32,
}

/// Run `command` (via `sh -c`) inside a PTY.
///
/// Interactive prompts detected in the PTY output are forwarded to the IPC
/// client via `tx`; responses are read from `rx`.  The caller supplies the
/// tool-call `id` for IPC message correlation.
///
/// Returns the combined output (stdout + stderr merged by PTY) and exit code.
pub async fn run_pty_command(
    id: &str,
    command: &str,
    cmd_timeout: Duration,
    tx: &mut WriteHalf,          // IPC send half
    rx: &mut BufReader<ReadHalf>, // IPC receive half
) -> anyhow::Result<PtyResult> {
    // 1. Allocate PTY.
    let OpenptyResult { master, slave } = openpty(None, None)?;

    // 2. Spawn child.
    let pid = unsafe { spawn_child(slave, command)? };
    close(slave)?;  // parent does not need the slave end

    // 3. Wrap master in AsyncFd for non-blocking async reads/writes.
    set_nonblocking(master)?;
    let async_master = AsyncFd::new(master)?;

    // 4. Drive the PTY: read output, detect prompts, inject responses.
    let result = drive_pty(id, &async_master, pid, cmd_timeout, tx, rx).await;

    // 5. Close master (signals EOF to child if still running).
    let _ = close(master);

    result
}
```

### Child spawn helper

```rust
/// Fork and exec `sh -c command` with `slave_fd` as stdin/stdout/stderr.
/// Called from the parent; returns the child PID.
unsafe fn spawn_child(slave_fd: RawFd, command: &str) -> nix::Result<nix::unistd::Pid> {
    match nix::unistd::fork()? {
        nix::unistd::ForkResult::Parent { child } => Ok(child),
        nix::unistd::ForkResult::Child => {
            // New session so the slave PTY becomes the controlling terminal.
            setsid().unwrap();
            // Set controlling terminal.
            nix::sys::ioctl::tiocnotty(libc::STDIN_FILENO).ok();
            nix::sys::ioctl::tiocsctty(slave_fd, false).unwrap();
            // Redirect stdio.
            dup2(slave_fd, libc::STDIN_FILENO).unwrap();
            dup2(slave_fd, libc::STDOUT_FILENO).unwrap();
            dup2(slave_fd, libc::STDERR_FILENO).unwrap();
            if slave_fd > libc::STDERR_FILENO {
                close(slave_fd).unwrap();
            }
            // Resource limits (mirrors existing pipe-based executor).
            let mem = libc::rlimit { rlim_cur: 512 * 1024 * 1024, rlim_max: 512 * 1024 * 1024 };
            libc::setrlimit(libc::RLIMIT_AS, &mem);
            let fds = libc::rlimit { rlim_cur: 256, rlim_max: 256 };
            libc::setrlimit(libc::RLIMIT_NOFILE, &fds);
            // Exec.
            let sh = std::ffi::CString::new("sh").unwrap();
            let c_arg = std::ffi::CString::new(command).unwrap();
            nix::unistd::execvp(&sh, &[&sh, &std::ffi::CString::new("-c").unwrap(), &c_arg]).unwrap();
            unreachable!()
        }
    }
}
```

### PTY drive loop

```rust
async fn drive_pty(
    id: &str,
    master: &AsyncFd<RawFd>,
    pid: nix::unistd::Pid,
    cmd_timeout: Duration,
    tx: &mut WriteHalf,
    rx: &mut BufReader<ReadHalf>,
) -> anyhow::Result<PtyResult> {
    let mut detector = PromptDetector::new();
    let mut output_buf = String::new();
    let deadline = tokio::time::Instant::now() + cmd_timeout;

    loop {
        tokio::select! {
            // Read from PTY master.
            guard = master.readable() => {
                let mut guard = guard?;
                let mut chunk = [0u8; 4096];
                match unsafe { libc::read(master.as_raw_fd(), chunk.as_mut_ptr() as _, chunk.len()) } {
                    n if n > 0 => {
                        let s = String::from_utf8_lossy(&chunk[..n as usize]);
                        output_buf.push_str(&s);
                        // Check for interactive prompt.
                        match detector.check(&s) {
                            Some(PromptEvent::Credential { label }) => {
                                send_response(tx, Response::CredentialPrompt { id: id.to_string(), prompt: label }).await?;
                                let response = await_credential_response(rx).await?;
                                write_to_master(master, &format!("{}\n", response)).await?;
                            }
                            Some(PromptEvent::Confirmation { message }) => {
                                send_response(tx, Response::ConfirmationPrompt { id: id.to_string(), message }).await?;
                                let accepted = await_confirmation_response(rx).await?;
                                let answer = if accepted { "yes\n" } else { "no\n" };
                                write_to_master(master, answer).await?;
                            }
                            None => {}
                        }
                        guard.clear_ready();
                    }
                    0 | _ => {
                        // EOF from master — child has exited.
                        break;
                    }
                }
            }

            // Timeout.
            _ = tokio::time::sleep_until(deadline) => {
                let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
                return Ok(PtyResult {
                    output: format!("Command timed out after {} s and was killed", cmd_timeout.as_secs()),
                    exit_code: -1,
                });
            }
        }
    }

    // Reap child.
    let status = nix::sys::wait::waitpid(pid, None)?;
    let exit_code = match status {
        nix::sys::wait::WaitStatus::Exited(_, code) => code,
        _ => -1,
    };

    Ok(PtyResult { output: output_buf, exit_code })
}
```

---

## 7. Changes to Existing Files

### `src/ipc.rs`

- Add `CredentialPrompt { id, prompt }` to `Response` enum.
- Add `ConfirmationPrompt { id, message }` to `Response` enum.
- Add `CredentialResponse { id, credential }` to `Request` enum.
- Add `ConfirmationResponse { id, accepted }` to `Request` enum.
- Add round-trip unit tests for all four new variants.

### `src/daemon.rs`

- Add `mod pty_exec;` module declaration.
- In the `background=true` branch of `handle_client`, replace the `tokio::process::Command` block with a call to `pty_exec::run_pty_command(...)`.
- The existing `SudoPrompt`/`SudoPassword` round-trip is **removed** from this branch; sudo password prompts are now handled automatically by `PromptDetector` detecting the sudo password prompt in the PTY output.
- The `inject_sudo_flags` helper (`sudo -S -p ""`) is no longer needed for background PTY commands — remove the `-S` flag injection for the PTY path (the PTY provides a real terminal, so `sudo` prompts normally).
- Keep `command_has_sudo` (unchanged) for the foreground path and for session-approval classification in the client.
- Update `log_command` call for background mode: the `output` field now comes from `PtyResult.output`.

### `src/main.rs`

- Add `mod pty_exec;` if the module is top-level, or nest it under daemon. No CLI changes are needed.

### `Cargo.toml`

- `nix = "0.31.1"` is already present with no additional features needed.
  - `nix::pty::openpty` — available in the default feature set.
  - `nix::unistd::{fork, setsid, dup2, execvp, close}` — available.
  - `nix::sys::signal::kill` — available.
  - `nix::sys::wait::waitpid` — available.
  - Verify: `nix::sys::ioctl::{tiocnotty, tiocsctty}` — may require the `ioctl` feature flag; check and add if needed.
- No other new dependencies required.

### `src/client.rs`

- Add handlers for `Response::CredentialPrompt` and `Response::ConfirmationPrompt` in `ask_with_session`.
- `CredentialPrompt`: display the prompt label, read password with echo disabled (same as the existing `SudoPrompt` handler), send `Request::CredentialResponse`.
- `ConfirmationPrompt`: display the `message`, offer `[Y]es / [N]o`, send `Request::ConfirmationResponse`.
- The existing `Response::SudoPrompt` handler is kept for the foreground path (unchanged).

---

## 8. Client-Side Changes (detail)

```rust
Response::CredentialPrompt { id, prompt } => {
    if !response_started { print!("\r\x1b[K"); response_started = true; }
    println!("  \x1b[33m⚙\x1b[0m {}", prompt);
    let password = read_password_no_echo().await?;
    send_request(&mut tx, Request::CredentialResponse { id, credential: password }).await?;
}

Response::ConfirmationPrompt { id, message } => {
    if !response_started { print!("\r\x1b[K"); response_started = true; }
    println!("  \x1b[33m⚙\x1b[0m {}", message);
    print!("  \x1b[32mProceed?\x1b[0m [\x1b[1;92mY\x1b[0m]es  [\x1b[1;91mN\x1b[0m]o \x1b[32m›\x1b[0m ");
    std::io::stdout().flush()?;
    let input = stdin.read_line().await.unwrap_or_default();
    let accepted = matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    send_request(&mut tx, Request::ConfirmationResponse { id, accepted }).await?;
}
```

The `read_password_no_echo` helper is a refactoring of the existing echo-disabled read used in the `SudoPrompt` handler — extract it into a shared private function.

---

## 9. Testing Plan

### Unit tests

1. **`PromptDetector` — credential patterns**: Feed sample sudo/su/ssh/gpg prompts; assert `Credential` event fires. Feed command output lines that mention passwords but are not prompts (e.g., log lines); assert no event fires.
2. **`PromptDetector` — confirmation patterns**: Feed SSH host-key prompt; assert `Confirmation` event. Feed a yes/no question embedded mid-line; assert no event.
3. **`PromptDetector` — attempt limit**: Simulate 3 credential injections followed by a 4th prompt; assert the detector signals exhaustion.
4. **IPC round-trips**: `CredentialPrompt`, `ConfirmationPrompt`, `CredentialResponse`, `ConfirmationResponse` — all four serde round-trips.

### Integration tests (manual)

1. **Non-interactive command** (`echo hello`): confirm output is captured correctly via PTY, no prompts detected.
2. **sudo command** (with real password prompt): confirm `CredentialPrompt` fires in chat, credential is injected, command succeeds.
3. **SSH to a new host**: confirm `ConfirmationPrompt` fires for host-key, user accepts, connection proceeds.
4. **Timeout**: run `sleep 300` with a 5-second timeout override; confirm process is killed and error is returned.
5. **Three-strikes**: enter wrong password three times; confirm daemon kills the process and returns an error to the AI.

---

## 10. Security Constraints

| Concern | Mitigation |
|---|---|
| Credential capture in output | PTY output is masked through `mask_sensitive()` before being returned to the AI. Credentials typed into the PTY by injection are not re-echoed into the output buffer (PTY echo is disabled for the duration of credential injection via `cfmakeraw` on master). |
| PTY output includes the typed credential | After injecting the credential the daemon reads and discards the echo line (if any) before resuming output accumulation. |
| Autonomous sudo without user knowledge | All background commands (PTY or pipe) go through the same `ToolCallPrompt` approval gate. Session-level approval (`[A]pprove for session`) still requires the user's explicit consent the first time. |
| Runaway processes | `RLIMIT_AS` (512 MiB) and `RLIMIT_NOFILE` (256) are set in the child before `exec`, identical to the current pipe-based executor. The timeout still applies. |
| Fork safety in async runtime | `fork()` is called before any tokio tasks are spawned for the child — the child immediately `exec`s, never returning to user-land async code. This follows the standard `fork`+`exec` pattern and avoids async-signal-safety issues. |

---

## 11. Implementation Phases

### Phase 1 — Infrastructure (no behaviour change)

- Create `src/pty_exec/mod.rs` and `src/pty_exec/prompt.rs`.
- Implement `PromptDetector` with full unit tests.
- Add IPC variants to `ipc.rs` with round-trip tests.
- `cargo test` — all existing tests pass.

### Phase 2 — PTY executor and foreground prompt detection

**Background path:**
- Implement `run_pty_command` in `pty_exec/mod.rs`.
- Replace the `tokio::process::Command` block in `daemon.rs` with a `run_pty_command` call.
- Remove `SudoPrompt`/`SudoPassword` IPC variants (replaced by `CredentialPrompt`/`CredentialResponse`).
- Remove `inject_sudo_flags` and `command_has_sudo` helpers from `daemon.rs`.
- Add `CredentialPrompt`/`ConfirmationPrompt` handlers to `client.rs`.

**Foreground path (§13):**
- Replace the bifurcated sudo/non-sudo wait loop in `daemon.rs` with a single unified loop.
- Apply `PromptDetector` to `capture-pane` snapshots on each poll iteration.
- On detection: send `Response::SystemMsg` with prompt description, switch pane focus if not already done.
- Do **not** inject credentials — user types directly in the terminal pane.
- Command timeout pauses while a prompt is active; resumes once the prompt resolves.

Manual tests: non-interactive background command, foreground sudo command, foreground SSH to new host.

### Phase 3 — Prompt handling (sudo + confirmations)

- Enable prompt detection in `drive_pty`.
- Manual test: background `sudo` command triggers password prompt in chat, succeeds.
- Manual test: background `ssh` to new host triggers confirmation, user accepts.

### Phase 4 — Autonomous runbook support

- Extend the AI system prompt with guidance on which commands are safe for fully autonomous execution (i.e., require no interactive prompts beyond those the PTY executor can handle).
- Add a `runbook` IPC request type and a `daemoneye runbook <name>` subcommand.
- The daemon executes the runbook using the PTY executor throughout, emitting `CredentialPrompt` and `ConfirmationPrompt` to the chat pane as needed.
- Session-level approval for credential classes (`[A]pprove sudo for session`) means the user can authorise an entire runbook run in one action.

---

## 13. Foreground PromptDetector Integration

### 13.1 Motivation

The existing foreground wait loop in `daemon.rs` only detects sudo password prompts by checking `pane_current_command == "sudo"`. Every other blocking interactive prompt — `su`, SSH host-key confirmations, GPG passphrases, database logins — causes the daemon to silently wait until the 30-second timeout expires. The user sees their terminal blocked with no feedback from the chat pane.

Applying `PromptDetector` to `capture-pane` output during the foreground wait loop broadens awareness to all prompt types and generates informative chat notifications for each.

### 13.2 What the detector does (and does not do) for foreground

| Aspect | Background (PTY) | Foreground (tmux pane) |
|---|---|---|
| Prompt detected via | Reading PTY master fd (streamed) | Polling `capture-pane` (snapshots) |
| On credential prompt | Send `CredentialPrompt` → inject response into PTY | Send `SystemMsg` + switch pane focus. User types directly. |
| On confirmation prompt | Send `ConfirmationPrompt` → inject `yes\n`/`no\n` into PTY | Send `SystemMsg` + switch pane focus. User types directly. |
| Credential relay | Yes — daemon controls child stdin via PTY master | **No** — injecting via `send-keys` would echo password in the pane |
| Timeout behaviour | Paused for credential awaits (user IPC response) | Command timeout paused while prompt is active in pane |

### 13.3 Unified foreground wait loop

The bifurcated `if command_has_sudo(cmd)` / `else` branches are replaced with a single loop:

```
loop (100 ms poll):
    pane_current_command     → back_to_shell flag
    capture-pane (10 lines)  → strip_ansi → PromptDetector.check()

    if prompt detected:
        if first detection of this prompt text:
            send Response::SystemMsg("password/confirmation prompt detected — respond in terminal")
            tmux select-pane (switch focus to working pane)
        pause command timeout (don't increment waited)
        reset stability counter

    if no prompt:
        clear last_prompt_text
        increment waited

    exit when (back_to_shell && stable_ticks >= 2) || waited >= 30 s
```

### 13.4 Timeout semantics

Two timers govern the unified loop:

- **`waited`** — cumulative time the loop has polled without a prompt being active. Caps at 30 seconds. Paused whenever `PromptDetector` fires (user is responding).
- **`wall_deadline`** — hard 120-second absolute limit to prevent infinite hangs (e.g. a command that repeatedly re-prompts). Unaffected by prompt activity.

### 13.5 Changes to `daemon.rs`

- Remove `command_has_sudo` — no longer needed for the foreground path (PromptDetector covers it) or the background path (PTY executor handles it natively).
- Remove `inject_sudo_flags` — the PTY provides a real terminal; `sudo -S -p ""` flag injection is not needed.
- Replace the `if command_has_sudo(cmd) { ... } else { ... }` wait block (≈ 50 lines) with the unified loop (≈ 40 lines) that calls `crate::pty_exec::strip_ansi` and `crate::pty_exec::PromptDetector`.

### 13.6 No IPC changes required

Foreground prompt detection sends only `Response::SystemMsg`, which is already implemented in both daemon and client. No new IPC variants are needed for the foreground path.

---

## 12. Out of Scope

- **Windows / macOS**: PTY implementation uses POSIX APIs; these platforms are not supported by DaemonEye.
- **GUI / web interface**: The IPC client is always `daemoneye chat`; no browser integration.
- **Multi-user / privilege separation**: The daemon runs as the owning user; no setuid or privilege escalation beyond what `sudo` itself provides.
- **PTY size negotiation**: The PTY window size is set to a nominal 220×50 (wide enough for most CLI output). Dynamic resize is not implemented in Phase 1–3.
