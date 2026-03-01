mod prompt;
pub use prompt::{PromptDetector, PromptKind};

use std::os::unix::io::AsRawFd;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::ipc::{Request, Response};

// ---------------------------------------------------------------------------
// Public output type
// ---------------------------------------------------------------------------

pub struct PtyOutput {
    pub output: String,
    pub exit_code: i32,
}

// ---------------------------------------------------------------------------
// ANSI stripping (shared with daemon.rs foreground path)
// ---------------------------------------------------------------------------

/// Strip ANSI/VT escape sequences and bare carriage returns from `s`.
///
/// Handles CSI sequences (`\x1b[...letter`) and SS3 sequences (`\x1bOletter`).
/// Used before running `PromptDetector` on PTY output and `capture-pane` snapshots.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    for nc in chars.by_ref() {
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some('O') => {
                    chars.next(); // consume 'O'
                    chars.next(); // consume the SS3 final byte
                }
                _ => {} // bare escape — drop it
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// AsyncFd wrapper for the PTY master fd
// ---------------------------------------------------------------------------

/// Non-owning wrapper so a raw fd can be registered with tokio's AsyncFd.
/// The fd is closed explicitly by the caller after `run_pty_command` returns.
struct MasterFd(libc::c_int);

impl AsRawFd for MasterFd {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run `command` (via `sh -c`) inside a PTY, forwarding interactive prompts
/// to the IPC client and injecting responses into the PTY master.
///
/// - Credential prompts → `Response::CredentialPrompt` → `Request::CredentialResponse`
/// - Confirmation prompts → `Response::ConfirmationPrompt` → `Request::ConfirmationResponse`
///
/// Returns the combined (ANSI-stripped) output and process exit code.
pub async fn run_pty_command(
    id: &str,
    command: &str,
    cmd_timeout: Duration,
    tx: &mut OwnedWriteHalf,
    rx: &mut BufReader<OwnedReadHalf>,
) -> anyhow::Result<PtyOutput> {
    // ── Allocate PTY ─────────────────────────────────────────────────────────
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let ret = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(anyhow::anyhow!(
            "openpty failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    // Set master non-blocking so AsyncFd can poll it.
    unsafe {
        let flags = libc::fcntl(master_fd, libc::F_GETFL, 0);
        libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    // ── Spawn child with slave as controlling terminal ────────────────────────
    let mut proc = tokio::process::Command::new("sh");
    proc.args(["-c", command]);

    // pre_exec runs in the child after fork but before exec.
    // We redirect stdio to the slave PTY and apply resource limits.
    unsafe {
        proc.pre_exec(move || {
            // New session so the slave can become the controlling terminal.
            libc::setsid();
            // Attach slave as controlling terminal.
            if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0i32) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Redirect stdin/stdout/stderr to slave.
            libc::dup2(slave_fd, libc::STDIN_FILENO);
            libc::dup2(slave_fd, libc::STDOUT_FILENO);
            libc::dup2(slave_fd, libc::STDERR_FILENO);
            if slave_fd > libc::STDERR_FILENO {
                libc::close(slave_fd);
            }
            // Resource limits (mirrors the old pipe-based executor).
            let mem = libc::rlimit {
                rlim_cur: 512 * 1024 * 1024,
                rlim_max: 512 * 1024 * 1024,
            };
            libc::setrlimit(libc::RLIMIT_AS, &mem);
            let fds = libc::rlimit { rlim_cur: 256, rlim_max: 256 };
            libc::setrlimit(libc::RLIMIT_NOFILE, &fds);
            Ok(())
        });
    }

    let mut child = proc.spawn()?;
    // Parent no longer needs the slave end.
    unsafe { libc::close(slave_fd) };

    // ── Drive PTY ────────────────────────────────────────────────────────────
    let result = drive_pty(id, master_fd, &mut child, cmd_timeout, tx, rx).await;
    // Close master (signals EIO to child if still running).
    unsafe { libc::close(master_fd) };
    result
}

// ---------------------------------------------------------------------------
// PTY drive loop
// ---------------------------------------------------------------------------

async fn drive_pty(
    id: &str,
    master_fd: libc::c_int,
    child: &mut tokio::process::Child,
    cmd_timeout: Duration,
    tx: &mut OwnedWriteHalf,
    rx: &mut BufReader<OwnedReadHalf>,
) -> anyhow::Result<PtyOutput> {
    let async_master = tokio::io::unix::AsyncFd::new(MasterFd(master_fd))?;
    let mut detector = PromptDetector::new();
    let mut output_buf = String::new();
    let mut last_prompt: Option<String> = None;
    let deadline = tokio::time::Instant::now() + cmd_timeout;

    let exit_code = loop {
        tokio::select! {
            guard_result = async_master.readable() => {
                let mut guard = guard_result?;
                let mut raw = [0u8; 4096];
                let n = unsafe {
                    libc::read(
                        master_fd,
                        raw.as_mut_ptr() as *mut libc::c_void,
                        raw.len(),
                    )
                };
                if n > 0 {
                    let chunk = strip_ansi(&String::from_utf8_lossy(&raw[..n as usize]));
                    output_buf.push_str(&chunk);

                    if detector.exhausted() {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        return Ok(PtyOutput {
                            output: "Too many failed credential attempts; command killed.".to_string(),
                            exit_code: -1,
                        });
                    }

                    match detector.check(&chunk) {
                        Some(event) if last_prompt.as_deref() != Some(&event.text) => {
                            // Record this prompt text so that if the same text appears
                            // again in a subsequent read (before the PTY outputs a fresh
                            // prompt) we don't trigger twice.  The deduplication key is
                            // cleared in the None arm once the output no longer contains
                            // a prompt, allowing a wrong-password re-prompt to fire again.
                            last_prompt = Some(event.text.clone());
                            match event.kind {
                                PromptKind::Credential => {
                                    detector.record_attempt();
                                    send_ipc(tx, Response::CredentialPrompt {
                                        id: id.to_string(),
                                        prompt: event.text,
                                    }).await?;
                                    let credential = await_credential(rx).await;
                                    let line = format!("{}\n", credential);
                                    unsafe {
                                        libc::write(
                                            master_fd,
                                            line.as_ptr() as *const libc::c_void,
                                            line.len(),
                                        );
                                    }
                                }
                                PromptKind::Confirmation => {
                                    send_ipc(tx, Response::ConfirmationPrompt {
                                        id: id.to_string(),
                                        message: event.text,
                                    }).await?;
                                    let accepted = await_confirmation(rx).await;
                                    let answer = if accepted { "yes\n" } else { "no\n" };
                                    unsafe {
                                        libc::write(
                                            master_fd,
                                            answer.as_ptr() as *const libc::c_void,
                                            answer.len(),
                                        );
                                    }
                                }
                            }
                        }
                        Some(_) => {} // same prompt text — already being handled
                        None => {
                            // Prompt gone from output; clear so a fresh re-prompt is handled.
                            last_prompt = None;
                        }
                    }
                    // Leave guard ready so we drain all available data before blocking.
                } else {
                    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                    if n == 0 || errno == libc::EIO {
                        break child.wait().await.ok()
                            .and_then(|s| s.code())
                            .unwrap_or(0);
                    } else if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                        guard.clear_ready();
                    } else {
                        // Other error — treat as EOF.
                        break child.wait().await.ok()
                            .and_then(|s| s.code())
                            .unwrap_or(-1);
                    }
                }
            }

            _ = tokio::time::sleep_until(deadline) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Ok(PtyOutput {
                    output: format!(
                        "Command timed out after {} s and was killed",
                        cmd_timeout.as_secs()
                    ),
                    exit_code: -1,
                });
            }
        }
    };

    let output = normalize_output(&output_buf);
    Ok(PtyOutput { output, exit_code })
}

// ---------------------------------------------------------------------------
// IPC helpers
// ---------------------------------------------------------------------------

async fn send_ipc(tx: &mut OwnedWriteHalf, response: Response) -> anyhow::Result<()> {
    let mut json = serde_json::to_string(&response)?;
    json.push('\n');
    tx.write_all(json.as_bytes()).await?;
    Ok(())
}

/// Wait up to 120 s for a `CredentialResponse` from the client.
async fn await_credential(rx: &mut BufReader<OwnedReadHalf>) -> String {
    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(120), rx.read_line(&mut line)).await {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::CredentialResponse { credential, .. }) => credential,
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// Wait up to 120 s for a `ConfirmationResponse` from the client.
async fn await_confirmation(rx: &mut BufReader<OwnedReadHalf>) -> bool {
    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(120), rx.read_line(&mut line)).await {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ConfirmationResponse { accepted, .. }) => accepted,
            _ => false,
        },
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Output normalisation
// ---------------------------------------------------------------------------

/// Trim leading/trailing blank lines from PTY output.
/// PTY sessions typically emit extra newlines around prompts and on exit.
fn normalize_output(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    if start >= end {
        return String::new();
    }
    lines[start..end].join("\n")
}

// ---------------------------------------------------------------------------
// strip_ansi tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\x1b[32mhello\x1b[0m"), "hello");
    }

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\x1b[1;91mred bold\x1b[0m"), "red bold");
    }

    #[test]
    fn strip_ansi_removes_carriage_returns() {
        assert_eq!(strip_ansi("foo\r\nbar"), "foo\nbar");
    }

    #[test]
    fn strip_ansi_leaves_plain_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_removes_ss3() {
        // SS3 sequences: \x1bOA (cursor up) etc.
        assert_eq!(strip_ansi("\x1bOAtext"), "text");
    }
}
