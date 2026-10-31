//! A live shell behind phase-05a's `ShellBackend`: one pump task feeds the
//! subscriber broadcast, the cast log and the screen from the same PTY bytes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::shell::host::ShellBackend;
use crate::shell::log::CastWriter;
use crate::shell::proto::{ShellResponse, ShellSignal, ShellState};
use crate::shell::pty::PtyShell;
use crate::shell::screen::ShellScreen;
use crate::util::UnpoisonExt;

/// Map a `ShellSignal` to the `kill(1)` signal name that `PtyShell`'s
/// `signal_foreground` hands to the kernel.
fn signal_name(sig: ShellSignal) -> &'static str {
    match sig {
        ShellSignal::Int => "INT",
        ShellSignal::Term => "TERM",
        ShellSignal::Stop => "STOP",
        ShellSignal::Cont => "CONT",
    }
}

/// The `/proc/<pgid>/stat` state char (F5): the first token after the last
/// `)`, because `comm` is parenthesised and may itself contain spaces and
/// parens. `Some(true)` when the group leader is stopped (`T`), `Some(false)`
/// for any other readable state, `None` when `/proc` cannot be read — a
/// missing entry means the process is gone, which is not a paused state.
fn job_state_is_stopped(pgid: i64) -> Option<bool> {
    let s = std::fs::read_to_string(format!("/proc/{pgid}/stat")).ok()?;
    Some(s.rsplit(')').next()?.split_whitespace().next()? == "T")
}

/// True if `chunk` looks like a prompt asking for a secret.
///
/// **Deliberately broad.** This gates *redaction*, not an answer: a false
/// positive costs one line of log fidelity, a false negative writes a
/// password to disk. That is the opposite of the daemon-side detector in
/// `daemon::utils::sudo::classify_prompt`, which matches only its own
/// per-invocation sentinel because there a false positive would inject a
/// credential into an ordinary command. Do not narrow this to match that.
fn looks_like_secret_prompt(chunk: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "password",          // sudo, ssh, su, most programs; case-folded below
        "passphrase",        // ssh key unlock
        "de-sudo-prompt-",   // the sentinel shape 01a installs
        "verification code", // 2FA
        "otp",
    ];
    let lower = chunk.to_ascii_lowercase();
    NEEDLES.iter().any(|n| lower.contains(n))
}

/// True when `bytes` end a secret-entry state: a real newline or carriage
/// return submits the answer, and a Ctrl-C (the give-up write, 0x03) cancels
/// the prompt outright — either way the shell is no longer at the prompt, so
/// the scrub disarms and the next command's text is logged in clear.
fn ends_secret_entry(bytes: &[u8]) -> bool {
    bytes.contains(&b'\n') || bytes.contains(&b'\r') || bytes.contains(&0x03)
}

/// A live shell behind phase-05a's `ShellBackend`: one pump task feeds the
/// subscriber broadcast, the cast log and the screen from the same PTY bytes.
pub struct PtyBackend {
    shell: Mutex<PtyShell>,
    log: Mutex<CastWriter>,
    screen: Mutex<ShellScreen>,
    /// Set when the last output chunk looked like a secret prompt; cleared
    /// when the answer is submitted. Armed **in the pump before `out_tx.send`**
    /// so it cannot lose the race against a daemon that answers the moment it
    /// sees the prompt on the broadcast.
    awaiting_secret: AtomicBool,
    out_tx: broadcast::Sender<Vec<u8>>,
    started: Instant,
}

impl PtyBackend {
    /// `shell` is a path or bare name; `args` are its argv (`&[]` for a
    /// local login shell); `cast` is where the recording goes.
    pub fn spawn(
        shell: &str,
        args: &[String],
        rows: u16,
        cols: u16,
        scrollback: usize,
        cast: &Path,
        started_unix: u64,
    ) -> anyhow::Result<Arc<Self>> {
        let pty = PtyShell::spawn(shell, args, (rows, cols)).context("spawning shell")?;
        let log =
            CastWriter::create(cast, cols, rows, started_unix).context("creating cast log")?;
        let screen = ShellScreen::new(rows, cols, scrollback);
        let (out_tx, _initial_rx) = broadcast::channel(crate::shell::pty::STREAM_CAPACITY);
        let started = Instant::now();

        let this = Arc::new(Self {
            shell: Mutex::new(pty),
            log: Mutex::new(log),
            screen: Mutex::new(screen),
            awaiting_secret: AtomicBool::new(false),
            out_tx,
            started,
        });
        let mut rx = this.shell.lock().unwrap_or_log().subscribe();

        let pump_this = Arc::clone(&this);
        std::thread::spawn(move || {
            loop {
                match rx.blocking_recv() {
                    Ok(chunk) => {
                        if looks_like_secret_prompt(&String::from_utf8_lossy(&chunk)) {
                            pump_this.awaiting_secret.store(true, Ordering::SeqCst);
                        }
                        {
                            let mut screen = pump_this.screen.lock().unwrap_or_log();
                            screen.feed(&chunk);
                            let _ = pump_this.out_tx.send(chunk.clone());
                        }
                        let log_res = pump_this
                            .log
                            .lock()
                            .unwrap_or_log()
                            .write_output(pump_this.started.elapsed(), &chunk);
                        if let Err(e) = log_res {
                            log::error!("shell host: writing cast log: {e:#}");
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });

        Ok(this)
    }
}

#[async_trait]
impl ShellBackend for PtyBackend {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .write_input(bytes)
            .context("write input to shell")?;
        // Real bytes always reach the shell — only the log is scrubbed.
        // Mutation B in the Update Log deletes this branch and shows the same
        // test going red on finding the credential in the cast.
        if self.awaiting_secret.load(Ordering::SeqCst) {
            self.log
                .lock()
                .unwrap_or_log()
                .write_input(self.started.elapsed(), b"[redacted]")
                .context("record redacted input event")?;
            let submitted = ends_secret_entry(bytes);
            if submitted {
                self.awaiting_secret.store(false, Ordering::SeqCst);
            }
        } else {
            self.log
                .lock()
                .unwrap_or_log()
                .write_input(self.started.elapsed(), bytes)
                .context("record input event")?;
        }
        Ok(())
    }

    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .resize(rows, cols)
            .context("resize shell")?;
        // The screen keeps its rows across a resize (vt100 0.18 semantics):
        // shrink pushes rows off the top into scrollback, grow pulls them back.
        self.screen.lock().unwrap_or_log().resize(rows, cols);
        Ok(())
    }

    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .signal_foreground(signal_name(sig))
            .with_context(|| format!("signal shell foreground group with {sig:?}"))
    }

    async fn status(&self) -> ShellResponse {
        let (state, rows, cols, pid, raw_mode, alt_screen, foreground) = {
            let mut shell = self.shell.lock().unwrap_or_log();
            let alive = shell.is_alive().with_context(|| "checking shell liveness");
            let screen = self.screen.lock().unwrap_or_log();
            let (rows, cols) = screen.size();
            let alt_screen = screen.is_alt_screen();
            let raw_mode = shell.raw_mode();
            let foreground = shell.foreground_command();
            let pid = shell.pid().unwrap_or(0);
            let state = match alive {
                Ok(true) => {
                    // Rule 2 checks /proc rather than trusting the remembered
                    // pgid alone: a job resumed or killed by something outside
                    // this process must not leave the shell reporting Paused
                    // forever. A missing /proc entry means the process is
                    // gone, which is not a paused state.
                    let paused = shell
                        .paused_pgid()
                        .is_some_and(|pgid| job_state_is_stopped(pgid).unwrap_or(false));
                    if paused {
                        ShellState::Paused
                    } else if shell.has_foreground_job() || shell.resumed_pgid_alive().is_some() {
                        // After a resume bash keeps the terminal (F2's probe:
                        // fg stays the shell's own while the job runs), so
                        // busy also covers the group this shell last stopped,
                        // while that group still has a live job.
                        ShellState::Running
                    } else {
                        ShellState::Idle
                    }
                }
                Ok(false) => ShellState::Exited,
                Err(e) => {
                    log::error!("shell host: checking shell liveness: {e:#}");
                    ShellState::Exited
                }
            };
            (
                state,
                rows,
                cols,
                pid,
                raw_mode,
                Some(alt_screen),
                foreground,
            )
        };
        ShellResponse::Status {
            state,
            rows,
            cols,
            pid,
            raw_mode,
            alt_screen,
            foreground,
        }
    }

    /// A receiver of output chunks for one subscriber.
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.out_tx.subscribe()
    }

    fn subscribe_with_screen(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
        let screen = self.screen.lock().unwrap_or_log();
        (screen.snapshot(), self.out_tx.subscribe())
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.shell.lock().unwrap_or_log().hangup()
    }

    /// Render under the screen mutex: the last `scrollback` rows that scrolled
    /// off the top, then the marked screen. The scrollback read must come
    /// before the marked render — `scrollback_rows` temporarily moves the
    /// parser's scrollback offset, and the viewport is read after it is
    /// restored, so the frame's screen section is exactly what `contents()`
    /// would show.
    async fn screen_text(&self, annotated: bool, scrollback: u16) -> anyhow::Result<ShellResponse> {
        let (text, rows, cols, cursor, alt_screen, modes) = {
            let mut screen = self.screen.lock().unwrap_or_log();
            let (rows, cols) = screen.size();
            let cursor = screen.cursor();
            let alt_screen = screen.is_alt_screen();
            let modes = screen.modes();
            let head = screen.scrollback_rows(usize::from(scrollback));
            let body = screen.marked(annotated);
            let text = match (head.is_empty(), body.is_empty()) {
                (true, _) => body,
                (false, true) => head,
                (false, false) => format!("{head}\n{body}"),
            };
            (text, rows, cols, cursor, alt_screen, modes)
        };
        Ok(ShellResponse::ScreenText {
            text,
            rows,
            cols,
            cursor,
            alt_screen,
            bracketed_paste: modes.bracketed_paste,
            application_cursor: modes.application_cursor,
        })
    }
}

impl PtyBackend {
    /// The visible screen, exactly as `status()` reports from it.
    pub fn screen_contents(&self) -> String {
        self.screen.lock().unwrap_or_log().contents()
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
