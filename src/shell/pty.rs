//! PTY-backed shell spawn and the marker protocol.
//!
//! A command is wrapped in BEGIN/END markers printed through `printf`; the
//! bytes between the markers are the command's exact output and the field
//! after the END marker is its real exit code. Everything but [`PtyShell`]
//! itself is pure over byte slices, so the protocol is testable without a PTY.

use anyhow::Context;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::TryRecvError;

/// Broadcast capacity in chunks (one chunk is one PTY read, at most 4096
/// bytes). A subscriber further behind than this is `Lagged`, never blocked.
pub const STREAM_CAPACITY: usize = 1024;

/// How long `run()` sleeps between polls of its subscription. `broadcast` has
/// no timed blocking receive, so the timed wait is a `try_recv` poll loop.
const RUN_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A per-command marker nonce. 128 bits of randomness rendered as 32 hex
/// characters, so a command's own output cannot plausibly collide with it.
#[derive(Clone, Debug)]
pub struct Nonce(pub(crate) String);

impl Nonce {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Nonce {
    fn default() -> Self {
        Self::new()
    }
}

/// Exit-code variable for a shell: fish, csh and tcsh use `$status`,
/// everything else `$?`. Matches the shell's basename so a path like
/// `/usr/bin/fish` still maps to `$status`.
pub fn exit_var(shell_name: &str) -> &'static str {
    let base = shell_name.trim().rsplit('/').next().unwrap_or("");
    match base {
        "fish" | "csh" | "tcsh" => "$status",
        _ => "$?",
    }
}

/// Wrap `cmd` in the marker protocol, `\n`-terminated.
///
/// The `DE_''BEG` / `DE_''END` split quotes are deliberate: the PTY echoes the
/// joined `DE_BEG`, so a search keyed on the full framed marker can only
/// match the bytes the shell actually wrote, never the echoed command line.
pub fn wrap_command(cmd: &str, nonce: &Nonce, shell_name: &str) -> String {
    format!(
        "printf '\\x1fDE_''BEG {}\\x1f\\n'; {}; printf '\\n\\x1fDE_''END {} %s\\x1f\\n' {}\n",
        nonce.as_str(),
        cmd,
        nonce.as_str(),
        exit_var(shell_name)
    )
}

/// What a completed command produced.
#[derive(Debug)]
pub struct CommandOutcome {
    /// Bytes strictly between the two markers, with the framing CRLF the
    /// protocol itself contributes removed. Not lossy-decoded here.
    pub output: Vec<u8>,
    /// The command's real exit status.
    pub exit_code: i32,
    /// The command's marker nonce, so a caller that did not build the nonce
    /// itself can still find this run's markers in a log.
    pub nonce: Nonce,
}

/// Scan an accumulated buffer for this run's completed command.
/// Returns `None` while the end marker has not arrived yet.
pub fn parse_outcome(buf: &[u8], nonce: &Nonce) -> Option<CommandOutcome> {
    let beg = begin_marker(nonce);
    let end = end_marker(nonce);
    let beg_pos = find_subslice(buf, &beg)?;
    let end_pos_rel = find_subslice(&buf[beg_pos + beg.len()..], &end)?;
    let end_pos = beg_pos + beg.len() + end_pos_rel;

    let code_field = &buf[end_pos + end.len()..];
    let digits_len = code_field.iter().take_while(|b| b.is_ascii_digit()).count();
    let digits = &code_field[..digits_len];
    if digits.is_empty() || code_field.get(digits_len) != Some(&b'\x1f') {
        return None;
    }
    let exit_code: i32 = std::str::from_utf8(digits).ok()?.parse().ok()?;

    let mut output = buf[beg_pos + beg.len()..end_pos].to_vec();
    // BEGIN's trailing `\n` and END's leading `\n` both arrive as `\r\n`
    // through ONLCR — that pair is protocol framing, not command output (F3).
    if output.starts_with(b"\r\n") {
        output.drain(..2);
    }
    if output.ends_with(b"\r\n") {
        output.truncate(output.len() - 2);
    }

    Some(CommandOutcome {
        output,
        exit_code,
        nonce: nonce.clone(),
    })
}

/// The same input with every marker sequence for this nonce removed,
/// including the end marker's exit-code field; `\x1f` bytes that are not part
/// of a marker for this nonce are left alone.
pub fn strip_markers(buf: &[u8], nonce: &Nonce) -> Vec<u8> {
    let beg = begin_marker(nonce);
    let end = end_marker(nonce);
    let mut out = Vec::with_capacity(buf.len());
    let mut rest = buf;
    while !rest.is_empty() {
        if let Some(p) = find_subslice(rest, &beg) {
            out.extend_from_slice(&rest[..p]);
            rest = &rest[p + beg.len()..];
            continue;
        }
        if let Some(p) = find_subslice(rest, &end) {
            out.extend_from_slice(&rest[..p]);
            let mut consumed = p + end.len();
            let mut has_code = false;
            while consumed < rest.len() && rest[consumed].is_ascii_digit() {
                consumed += 1;
                has_code = true;
            }
            if has_code && rest.get(consumed) == Some(&b'\x1f') {
                consumed += 1;
            }
            rest = &rest[consumed..];
            continue;
        }
        out.extend_from_slice(rest);
        break;
    }
    out
}

pub(crate) fn begin_marker(nonce: &Nonce) -> Vec<u8> {
    format!("\u{1f}DE_BEG {}\u{1f}", nonce.as_str()).into_bytes()
}

pub(crate) fn end_marker(nonce: &Nonce) -> Vec<u8> {
    format!("\u{1f}DE_END {} ", nonce.as_str()).into_bytes()
}

/// A well-formed end marker for `nonce` carrying `exit_code`, the bytes a bash
/// session emits when the command finishes. Test-only: the client's fake-host
/// tests must be able to terminate a command without a real shell, and the
/// marker is the protocol's private framing, spelt out nowhere else.
#[cfg(test)]
pub(crate) fn test_end_marker(nonce: &Nonce, exit_code: i32) -> Vec<u8> {
    format!("\u{1f}DE_END {} {exit_code}\u{1f}\r\n", nonce.as_str()).into_bytes()
}

/// The begin marker's raw bytes, as a shell executing `wrap_command`'s printf
/// emits them. Test-only companion to [`test_end_marker`].
#[cfg(test)]
pub(crate) fn test_begin_marker(nonce: &Nonce) -> Vec<u8> {
    format!("\u{1f}DE_BEG {}\u{1f}\r\n", nonce.as_str()).into_bytes()
}

pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A PTY-backed shell. Owns the master, the writer, and the spawned child,
/// plus a long-lived reader thread that publishes every chunk it reads onto a
/// broadcast, so any number of consumers can [`subscribe`](Self::subscribe).
pub struct PtyShell {
    shell: String,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    stream_tx: broadcast::Sender<Vec<u8>>,
    /// The pgid this shell stopped, `None` when nothing is paused (F2: a
    /// `CONT` cannot re-derive its target once the job has stopped).
    paused_pgid: Option<i64>,
    /// The pgid last resumed and still alive — bash keeps the terminal after
    /// a resume (F2's own probe), so "busy" must be answerable from this
    /// memory rather than from tc.
    resumed_pgid: Option<i64>,
    _reader_handle: Option<JoinHandle<()>>,
}

impl PtyShell {
    /// Spawn `shell` (a path like `/bin/bash` or a bare name) in a new PTY of
    /// `(rows, cols)`, passing it `args` as its argv (`&[]` = none).
    pub fn spawn(shell: &str, args: &[String], size: (u16, u16)) -> anyhow::Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        // portable-pty starts the child in $HOME when no cwd is set; a HOME
        // that is not a directory makes the child's chdir fail and the whole
        // spawn return ENOENT. Start in HOME when it exists, else in `/`.
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|h| h.is_dir());
        cmd.cwd(home.unwrap_or_else(|| std::path::PathBuf::from("/")));
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("spawn_command failed")?;
        // Without dropping the slave, the master never sees EOF.
        drop(pair.slave);

        let master = pair.master;
        let writer = master
            .take_writer()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("take_writer failed")?;
        let reader = master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("try_clone_reader failed")?;

        let (stream_tx, _initial_rx) = broadcast::channel(STREAM_CAPACITY);
        let reader_handle = take_reader(reader, stream_tx.clone());

        Ok(Self {
            shell: shell.to_string(),
            master,
            writer,
            child,
            stream_tx,
            paused_pgid: None,
            resumed_pgid: None,
            _reader_handle: Some(reader_handle),
        })
    }

    /// A receiver for every chunk the PTY produces from now on. Each
    /// subscriber sees the same bytes; one that falls more than
    /// [`STREAM_CAPACITY`] chunks behind receives `Lagged` on its next read.
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.stream_tx.subscribe()
    }

    /// Write raw bytes to the shell as a user typing would.
    pub fn write_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer
            .write_all(bytes)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("write input to PTY")?;
        self.writer
            .flush()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("flush input to PTY")
    }

    /// The shell process id, if the platform reports one.
    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// `true` while the shell child has not exited.
    pub fn is_alive(&mut self) -> anyhow::Result<bool> {
        Ok(self
            .child
            .try_wait()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("try_wait shell child")?
            .is_none())
    }
    /// Run `cmd` in the shell, returning its output and real exit code.
    ///
    /// Generates a fresh nonce, writes the wrapped command, then accumulates
    /// reads until the end marker for that nonce arrives or `timeout` passes.
    /// On timeout returns an `Err` naming the timeout and the command — never
    /// a fabricated exit code.
    pub fn run(&mut self, cmd: &str, timeout: Duration) -> anyhow::Result<CommandOutcome> {
        // Subscribe before writing, so no byte of this command's output can
        // precede the subscription.
        let mut rx = self.subscribe();
        let nonce = Nonce::new();
        let wrapped = wrap_command(cmd, &nonce, &self.shell);
        self.writer
            .write_all(wrapped.as_bytes())
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("write wrapped command to PTY")?;
        self.writer
            .flush()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("flush wrapped command to PTY")?;

        // The reader thread belongs to the shell; bytes a timed-out command
        // keeps producing reach later subscriptions (or nobody) instead of
        // racing a re-seated reader. The marker for this run's nonce claims
        // only this run's bytes, and earlier bytes precede its begin marker.
        let mut buf: Vec<u8> = Vec::new();
        let start = Instant::now();
        loop {
            let remaining = timeout
                .checked_sub(start.elapsed())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                self.terminate_foreground()?;
                return Err(anyhow::anyhow!(
                    "timed out after {timeout:?} waiting for command output: {cmd:?}"
                ));
            }
            match rx.try_recv() {
                Ok(chunk) => buf.extend_from_slice(&chunk),
                Err(TryRecvError::Empty) => {
                    std::thread::sleep(RUN_POLL_INTERVAL.min(remaining));
                    continue;
                }
                Err(TryRecvError::Lagged(n)) => {
                    // A dropped chunk could have carried the end marker, so
                    // this command's outcome is unknowable: fail it and free
                    // the shell rather than wait out the timeout in silence.
                    self.terminate_foreground()?;
                    return Err(anyhow::anyhow!(
                        "output subscription lagged by {n} chunks while running {cmd:?}"
                    ));
                }
                Err(TryRecvError::Closed) => {
                    return Err(anyhow::anyhow!("PTY closed while running {cmd:?}"));
                }
            }
            if let Some(outcome) = parse_outcome(&buf, &nonce) {
                return Ok(outcome);
            }
        }
    }

    /// Send SIGTERM to the current foreground process group so a timed-out
    /// command cannot keep occupying the shell.
    fn terminate_foreground(&mut self) -> anyhow::Result<()> {
        self.signal_foreground("TERM")
    }

    /// Send `sig` — a `kill(1)` signal name such as `INT`, `TERM`, `STOP` or
    /// `CONT` — to a process group.
    ///
    /// `STOP` targets the group this shell is tracking — the paused or
    /// resumed memory first, so a later target never has to re-derive it
    /// from a tty query; a shell tracking nothing falls back to the live
    /// foreground group, unless that group is the shell's own (F1). It stops
    /// **the shell first, then the job**: a running bash reaps a stopped job
    /// and runs on — the rest of a `for` loop keeps going, and a wrapped
    /// command prints its end marker with exit 147, so the command reads as
    /// finished while its job sits stopped (measured). A stopped shell
    /// cannot. `CONT` resumes the remembered group first, then the shell, so
    /// bash wakes to a running job and never sees the stop; it clears the
    /// memory. `INT`/`TERM` keep the live foreground target and clear the
    /// memory, because a cancelled job leaves nothing paused (F4); on a
    /// paused shell they also resume the job and the shell, or the signal
    /// would wait on two stopped processes forever.
    ///
    /// portable-pty exposes no signal-sending API, so the platform `kill(1)`
    /// is invoked with the negative PGID. The shell itself sits in its own
    /// process group and is untouched; when no job is in the foreground the
    /// group is the shell's own, and an interactive shell ignores
    /// `INT`/`TERM`.
    pub fn signal_foreground(&mut self, sig: &str) -> anyhow::Result<()> {
        let current = self.current_foreground_pgid();
        match sig {
            "STOP" => {
                // The terminal's foreground group is only a *hint* here: after
                // a resume bash keeps the terminal (F2's own probe), and a
                // shell's rc (PROMPT_COMMAND) can transiently hold it —
                // when this shell remembers a live tracked group, that group
                // is the job to pause, never the tty's current answer. Only a
                // shell tracking nothing falls back to the terminal, and even
                // then only when that group is not the shell's own (F1:
                // stopping the shell's own group would stop the shell).
                let pid = self.pid().map(i64::from);
                let mut target = self
                    .paused_group_alive()
                    .or_else(|| self.resumed_pgid_alive())
                    .or_else(|| current.filter(|&fg| Some(fg) != pid));
                // The window between bash forking a job and setpgid(2) reads
                // as "the shell's own group in the foreground" — a STOP that
                // landed there would silently do nothing. When nothing is
                // remembered, re-read the terminal for a short settle first;
                // an idle answer is stable, a job's group appears within it.
                if target.is_none() {
                    for _ in 0..3 {
                        std::thread::sleep(Duration::from_millis(20));
                        target = self.current_foreground_pgid().filter(|&fg| Some(fg) != pid);
                        if target.is_some() {
                            break;
                        }
                    }
                }
                if let Some(pgid) = target {
                    self.signal_shell("STOP")?;
                    self.signal_group(pgid, sig)?;
                    self.paused_pgid = Some(pgid);
                    self.resumed_pgid = None;
                }
            }
            "CONT" => {
                // The remembered group, never a re-derived one: a tty query
                // can answer the shell's own group and leave the job stopped.
                if let Some(pgid) = self.paused_pgid.take() {
                    self.signal_group(pgid, sig)?;
                    self.signal_shell("CONT")?;
                    self.resumed_pgid = Some(pgid);
                }
            }
            _ => {
                if let Some(pgid) = current {
                    self.signal_group(pgid, sig)?;
                }
                if let Some(pgid) = self.paused_pgid.take() {
                    self.signal_group(pgid, "CONT")?;
                    self.signal_shell("CONT")?;
                }
                self.resumed_pgid = None;
            }
        }
        Ok(())
    }

    pub(crate) fn current_foreground_pgid(&self) -> Option<i64> {
        self.master.process_group_leader().map(|pgid| pgid as i64)
    }

    /// Hang up the shell itself: SIGHUP to its pid, not to the foreground
    /// group. An interactive bash exits on it and hangs up its own jobs. A
    /// paused shell is stopped, so the hangup would wait forever: it and its
    /// job are resumed after the signal is queued.
    pub fn hangup(&self) -> anyhow::Result<()> {
        self.signal_shell("HUP")?;
        if let Some(pgid) = self.paused_pgid {
            self.signal_group(pgid, "CONT")?;
            self.signal_shell("CONT")?;
        }
        Ok(())
    }

    /// Send `sig` to the shell's own pid.
    fn signal_shell(&self, sig: &str) -> anyhow::Result<()> {
        let pid = self
            .pid()
            .ok_or_else(|| anyhow::anyhow!("the shell has no pid to signal"))?;
        let status = std::process::Command::new("kill")
            .arg("-s")
            .arg(sig)
            .arg("--")
            .arg(pid.to_string())
            .status()
            .with_context(|| format!("running kill -s {sig}"))?;
        if !status.success() {
            anyhow::bail!("kill -s {sig} -- {pid} failed: {status}");
        }
        Ok(())
    }

    fn signal_group(&self, pgid: i64, sig: &str) -> anyhow::Result<()> {
        if pgid > 0 {
            std::process::Command::new("kill")
                .arg("-s")
                .arg(sig)
                .arg("--")
                .arg(format!("-{pgid}"))
                .status()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context(format!("kill -s {sig} -- -{pgid} failed"))?;
        }
        Ok(())
    }

    /// The process group this shell stopped, if a job is currently paused.
    pub fn paused_pgid(&self) -> Option<i64> {
        self.paused_pgid
    }

    /// The paused group while its job is verifiably stopped: `/proc/<pgid>/stat`
    /// must read `T` (F3, F5). `None` on any read error, so a job removed by
    /// something outside this process never keeps the shell tracking a ghost.
    fn paused_group_alive(&self) -> Option<i64> {
        let pgid = self.paused_pgid?;
        let s = std::fs::read_to_string(format!("/proc/{pgid}/stat")).ok()?;
        let is_stopped = matches!(s.rsplit(')').next()?.split_whitespace().next(), Some("T"));
        Some(pgid).filter(|_| is_stopped)
    }

    /// The group last resumed while its job is still alive, read from `/proc`
    /// (F5). `None` once the job is gone, so a resumed job cannot keep the
    /// shell reporting busy forever.
    pub fn resumed_pgid_alive(&self) -> Option<i64> {
        let pgid = self.resumed_pgid?;
        match std::fs::read_to_string(format!("/proc/{pgid}/stat")) {
            Ok(s) => Some(pgid).filter(|_| {
                matches!(
                    // F5: first token after the last `)` is the state char.
                    s.rsplit(')').next().unwrap_or("").split_whitespace().next(),
                    Some("S") | Some("R") | Some("T")
                )
            }),
            Err(_) => None,
        }
    }

    /// Whether a foreground job holds the terminal — the tty's foreground
    /// group present and different from the shell's own pid (F1).
    pub fn has_foreground_job(&self) -> bool {
        match (self.current_foreground_pgid(), self.pid()) {
            (Some(pgid), Some(pid)) => pgid != i64::from(pid),
            _ => false,
        }
    }

    /// Resize the PTY.
    pub fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("resize PTY")
    }

    /// Kill the child.
    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.child
            .kill()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("kill shell child")
    }

    /// Wait for the child to exit.
    pub fn wait(&mut self) -> anyhow::Result<()> {
        self.child
            .wait()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("wait for shell child")?;
        Ok(())
    }
}

/// Move `reader` to a thread that reads until EOF/error, publishing each chunk
/// on the broadcast. The thread is the only reader of the PTY for the shell's
/// whole lifetime; consumers subscribe rather than read.
fn take_reader(reader: Box<dyn Read + Send>, tx: broadcast::Sender<Vec<u8>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                // `send` errs only when no subscriber exists at that instant;
                // those bytes belong to nobody, so dropping them is correct
                // and the reader keeps going.
                Ok(n) => {
                    let _ = tx.send(chunk[..n].to_vec());
                }
            }
        }
    })
}

#[cfg(test)]
mod pause_resume_tests {
    use super::*;
    use std::time::Duration;
    use std::time::Instant;

    /// The `/proc/<pgid>/stat` state char of `pgid`, read the F5 way: the
    /// state char is the first token after the last `)`, because `comm` is
    /// parenthesised and may itself contain spaces and parens.
    fn job_state_char(pgid: i64) -> Option<String> {
        let s = std::fs::read_to_string(format!("/proc/{pgid}/stat")).ok()?;
        s.rsplit(')')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .next()
            .map(str::to_string)
    }

    /// Poll `pred` until it holds or `deadline` passes; a stop/cont transition
    /// is asynchronous, so a single read raced against the signal.
    fn poll_until(deadline: Instant, mut pred: impl FnMut() -> bool, label: &str) {
        loop {
            if pred() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{label} not reached within the wait"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Start `cmd` as this shell's foreground job and wait until the terminal
    /// belongs to *that job*: the foreground group must differ from the
    /// shell's own pid (F1) and its leader's `/proc` cmdline must be the
    /// command just typed. A shell's startup rc (e.g. a `PROMPT_COMMAND`
    /// hook) can fork a transient child that holds the terminal for whole
    /// seconds before the queued line executes; without the cmdline check
    /// `STOP` fires at that transient and the test's own assumption about
    /// which job it pauses silently dies. The pty echoes a written line as
    /// the line discipline receives it, so the *echo* is not evidence the
    /// job has started — only the leader's cmdline is. Returns the broadcast
    /// subscription, which the caller keeps alive.
    fn start_job(shell: &mut PtyShell, cmd: &[u8]) {
        // Drop the newline: the executable the shell forks is `cmd` without
        // its terminator, split at spaces.
        let argv: Vec<&[u8]> = cmd[..cmd.len() - 1]
            .split(|b| *b == b' ')
            .filter(|p| !p.is_empty())
            .collect();
        let mut wanted = argv.join(&0u8);
        wanted.push(0);
        shell.write_input(cmd).expect("start the foreground job");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || {
                let Some(fg) = shell.current_foreground_pgid() else {
                    return false;
                };
                if Some(fg) == shell.pid().map(i64::from) {
                    return false;
                }
                std::fs::read(format!("/proc/{fg}/cmdline"))
                    .map(|c| c == wanted)
                    .unwrap_or(false)
            },
            "the queued job to hold the terminal",
        );
    }

    /// Panic-path cleanup for tests that start `sleep 30`: an assertion that
    /// fails between STOP and the job's teardown would otherwise leave a
    /// stopped `sleep` on the host (the phase's section-G leak check counts
    /// exactly that). Resumes the remembered group and kills the shell; Drop
    /// runs on unwind.
    struct JobGuard(PtyShell);

    impl std::ops::Deref for JobGuard {
        type Target = PtyShell;
        fn deref(&self) -> &PtyShell {
            &self.0
        }
    }

    impl std::ops::DerefMut for JobGuard {
        fn deref_mut(&mut self) -> &mut PtyShell {
            &mut self.0
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            for pgid in [self.0.paused_pgid(), self.0.resumed_pgid_alive()]
                .into_iter()
                .flatten()
            {
                let _ = std::process::Command::new("kill")
                    .arg("-s")
                    .arg("CONT")
                    .arg("--")
                    .arg(format!("-{pgid}"))
                    .status();
            }
            let shell_pid = self.0.pid().map(|p| p as i64);
            for pid in child_pids_of(shell_pid) {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(format!("{pid}"))
                    .status();
            }
            let _ = self.0.kill();
        }
    }

    /// The pids whose PPID is `pid`, by a bounded `/proc` walk, so the panic
    /// path kills the exact job this test started. Same reader as
    /// `shell::backend`'s tests; duplicated rather than shared to keep the
    /// two test modules under their own authorizations.
    fn child_pids_of(pid: Option<i64>) -> Vec<i64> {
        let Some(pid) = pid else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return found;
        };
        for entry in entries.flatten() {
            let Ok(raw) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(tpid) = raw.parse::<i64>() else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{tpid}/stat")) else {
                continue;
            };
            let Some(fields) = stat.rsplit(')').next() else {
                continue;
            };
            let mut it = fields.split_whitespace();
            let _state = it.next();
            if it.next() == Some(&pid.to_string()) {
                found.push(tpid);
            }
        }
        found
    }

    #[test]
    fn pty_pause_then_resume_actually_restarts_the_job() {
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
            .map(JobGuard)
            .expect("bash available");
        start_job(&mut shell, b"sleep 30\n");

        let started_pgid = shell.has_foreground_job();
        assert!(started_pgid, "the job must hold the terminal before STOP");

        shell
            .signal_foreground("STOP")
            .expect("STOP reaches the group");
        let paused = shell
            .paused_pgid()
            .expect("STOP must remember the job's pgid (F2)");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || job_state_char(paused).as_deref() == Some("T"),
            "the remembered job to be in /proc state T after STOP",
        );

        // The shell is stopped too, so it cannot reap the stopped job and
        // run on: the job keeps the terminal for the whole pause.
        let shell_pid = shell.pid().map(i64::from).expect("the shell has a pid");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || job_state_char(shell_pid).as_deref() == Some("T"),
            "the shell itself to be in /proc state T after STOP",
        );
        assert!(
            shell.has_foreground_job(),
            "the paused job keeps the terminal: bash never took it back"
        );

        shell
            .signal_foreground("CONT")
            .expect("CONT reaches the job");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || matches!(job_state_char(paused).as_deref(), Some("S") | Some("R")),
            "the remembered job to resume (state S or R) after CONT",
        );
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || matches!(job_state_char(shell_pid).as_deref(), Some("S") | Some("R")),
            "the shell to resume (state S or R) after CONT",
        );
        assert!(
            shell.paused_pgid().is_none(),
            "CONT consumed the memory: nothing is paused afterwards"
        );
        // The job still holds the terminal, so the shell cannot run the next
        // command until it is gone.
        shell.signal_foreground("INT").expect("end the resumed job");
        let outcome = shell
            .run("printf 'RESUMED\\n'", Duration::from_secs(10))
            .expect("the shell still answers after a pause/resume cycle");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("RESUMED"),
            "shell usable after resume: {text:?}"
        );
    }

    #[test]
    fn pty_stop_with_no_foreground_job_remembers_nothing() {
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
            .map(JobGuard)
            .expect("bash available");
        let deadline = Instant::now() + Duration::from_secs(10);
        poll_until(
            deadline,
            || !shell.has_foreground_job(),
            "the fresh shell to be idle (no fg job)",
        );
        shell
            .signal_foreground("STOP")
            .expect("STOP with no job present");
        assert!(
            shell.paused_pgid().is_none(),
            "an idle shell must not remember a stopped group (F1)"
        );
        // A STOP aimed at the shell's own group would stop the shell, and the
        // next command would hang: prove it has not.
        let outcome = shell
            .run("printf 'STILL_ALIVE\\n'", Duration::from_secs(10))
            .expect("the shell must still run after a no-op STOP");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(text.contains("STILL_ALIVE"), "shell still usable: {text:?}");
    }

    #[test]
    fn pty_cancel_clears_the_paused_group() {
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
            .map(JobGuard)
            .expect("bash available");
        start_job(&mut shell, b"sleep 30\n");
        shell.signal_foreground("STOP").expect("stop the job");
        assert!(
            shell.paused_pgid().is_some(),
            "the job must be paused before the cancel"
        );
        shell.signal_foreground("INT").expect("cancel the job");
        assert!(
            shell.paused_pgid().is_none(),
            "INT must clear the paused memory (F4)"
        );
        let outcome = shell
            .run(
                "printf 'STILL_ALIVE\\n'; sh -c 'exit 0'",
                Duration::from_secs(10),
            )
            .expect("the shell still answers after a cancel");
        assert_eq!(outcome.exit_code, 0, "the shell's exit code survives");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("STILL_ALIVE"),
            "shell usable after INT: {text:?}"
        );
    }

    /// `Ctrl-p` is a toggle (design line 371): pause, resume, pause again
    /// must drive the job back to `/proc` state `T` and re-arm the
    /// remembered group. The second STOP is the case bug-08a-2 measured as a
    /// silent no-op — bash keeps the terminal after a resume, so the arm must
    /// fall back to the group it last resumed (F2's probe, `after_second_CONT`).
    #[test]
    fn pty_pause_resume_pause_toggles_more_than_once() {
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
            .map(JobGuard)
            .expect("bash available");
        start_job(&mut shell, b"sleep 30\n");

        // First cycle: STOP remembers the group, CONT resumes it and keeps it
        // as the resumed hint.
        shell.signal_foreground("STOP").expect("the first STOP");
        let pgid = shell
            .paused_pgid()
            .expect("the first STOP must remember the job's pgid");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || job_state_char(pgid).as_deref() == Some("T"),
            "the job to be in /proc state T after the first STOP",
        );
        shell.signal_foreground("CONT").expect("the first CONT");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || matches!(job_state_char(pgid).as_deref(), Some("S") | Some("R")),
            "the job to resume (state S or R) after the first CONT",
        );
        assert!(
            shell.paused_pgid().is_none(),
            "the first CONT must consume the paused memory"
        );
        assert_eq!(
            shell.resumed_pgid_alive(),
            Some(pgid),
            "the resumed group must still be alive before the second STOP"
        );
        // Second cycle: the STOP reaches the job through the remembered
        // group, not through a tty query — inside its own group bash can
        // transiently hand the terminal to an rc-spawned child.
        shell.signal_foreground("STOP").expect("the second STOP");
        assert_eq!(
            shell.paused_pgid(),
            Some(pgid),
            "the second STOP must re-arm the paused memory for the job"
        );
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || job_state_char(pgid).as_deref() == Some("T"),
            "the job to be back in /proc state T after the second STOP",
        );
        shell.signal_foreground("CONT").expect("the second CONT");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || matches!(job_state_char(pgid).as_deref(), Some("S") | Some("R")),
            "the job to resume (state S or R) after the second CONT",
        );
        assert!(
            shell.paused_pgid().is_none(),
            "the second CONT must consume the paused memory too"
        );
    }

    /// The property a pause exists for: a command paused mid-run does not
    /// finish during the pause, and after the resume it finishes with its
    /// own exit code. With only the job stopped, bash reaped the stop and
    /// printed `END 147` at once (measured).
    #[test]
    fn pty_a_paused_command_finishes_with_its_real_exit_code() {
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
            .map(JobGuard)
            .expect("bash available");
        let mut rx = shell.subscribe();
        shell
            // `E''ND` in the typed line: the echo must not match the needle.
            .write_input(b"sleep 2; printf 'E''ND %s\\n' $?\n")
            .expect("start the command");
        let pid = shell.pid().map(i64::from);
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || {
                shell.current_foreground_pgid().is_some_and(|fg| {
                    Some(fg) != pid
                        && std::fs::read(format!("/proc/{fg}/cmdline"))
                            .is_ok_and(|c| c == b"sleep\x002\x00")
                })
            },
            "the command's sleep to hold the terminal",
        );
        shell.signal_foreground("STOP").expect("pause the command");
        let paused = shell.paused_pgid().expect("STOP remembers the job");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || job_state_char(paused).as_deref() == Some("T"),
            "the job to be in /proc state T after STOP",
        );
        shell.signal_foreground("CONT").expect("resume the command");

        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&seen).contains("END ") {
            assert!(
                Instant::now() < deadline,
                "the command never finished: {:?}",
                String::from_utf8_lossy(&seen)
            );
            match rx.try_recv() {
                Ok(chunk) => seen.extend_from_slice(&chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let text = String::from_utf8_lossy(&seen);
        assert!(
            text.contains("END 0"),
            "the paused command finished with its own exit code: {text:?}"
        );
        assert!(
            !text.contains("Stopped"),
            "bash never saw the job stop: {text:?}"
        );
    }

    /// Closing a paused shell ends it: the hangup reaches a shell that is
    /// itself stopped, so the shell and its job are resumed after it.
    #[test]
    fn pty_hangup_while_paused_ends_the_shell() {
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
            .map(JobGuard)
            .expect("bash available");
        start_job(&mut shell, b"sleep 30\n");
        shell.signal_foreground("STOP").expect("pause the job");
        let paused = shell.paused_pgid().expect("STOP remembers the job");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || job_state_char(paused).as_deref() == Some("T"),
            "the job to be in /proc state T after STOP",
        );
        shell.hangup().expect("hang up the paused shell");
        poll_until(
            Instant::now() + Duration::from_secs(10),
            || !shell.is_alive().unwrap_or(true),
            "the paused shell to exit after the hangup",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Serialises the tests in this module that spawn a PTY, so their forks
    /// never overlap. The HOME guard each of them also holds is what keeps a
    /// HOME-mutating test from deleting `HOME` under one of these spawns.
    static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn nonce(s: &str) -> Nonce {
        Nonce(s.to_string())
    }

    #[test]
    fn wrap_command_splits_the_marker_word() {
        let cmd = wrap_command("echo hi", &nonce("abc"), "bash");
        assert!(
            cmd.contains("DE_''BEG"),
            "must keep the split quote: {cmd:?}"
        );
        assert!(
            cmd.contains("DE_''END"),
            "must keep the split quote: {cmd:?}"
        );
        assert!(
            !cmd.contains("DE_BEG "),
            "must not contain the joined form the echo would produce: {cmd:?}"
        );
        assert!(
            !cmd.contains("DE_END "),
            "must not contain the joined form the echo would produce: {cmd:?}"
        );
    }

    #[test]
    fn wrap_command_uses_status_for_fish_and_question_for_others() {
        assert!(wrap_command(":", &nonce("a"), "fish").ends_with("$status\n"));
        assert!(wrap_command(":", &nonce("a"), "/usr/bin/fish").ends_with("$status\n"));
        assert!(wrap_command(":", &nonce("a"), "bash").ends_with("$?\n"));
        assert!(wrap_command(":", &nonce("a"), "zsh").ends_with("$?\n"));
        assert!(wrap_command(":", &nonce("a"), "/bin/bash").ends_with("$?\n"));
        assert!(wrap_command(":", &nonce("a"), "sh").ends_with("$?\n"));
    }

    #[test]
    fn parse_outcome_returns_none_before_the_end_marker() {
        let n = nonce("abc");
        let mut buf = b"matt@scrappy:~$ printf '\\x1fDE_''BEG abc\\x1f\\n'; echo hi; printf '\\n\\x1fDE_''END abc %s\\x1f\\n' $?\r\n".to_vec();
        buf.extend_from_slice(b"\x1fDE_BEG abc\x1f\r\n"); // begin marker arrived, echo precedes it
        assert!(parse_outcome(&buf, &n).is_none());
    }

    #[test]
    fn parse_outcome_ignores_the_echoed_command_line() {
        let n = nonce("n0nc3");
        let mut buf =
            b"matt@scrappy:~$ printf '\\x1fDE_''BEG n0nc3\\x1f\\n'; printf 'no-trailing-newline'; sh -c 'exit 42'; printf '\\n\\x1fDE_''END n0nc3 %s\\x1f\\n' $?\r\n"
                .to_vec();
        buf.extend_from_slice(b"\x1fDE_BEG n0nc3\x1f\r\n");
        buf.extend_from_slice(b"no-trailing-newline\r\n");
        buf.extend_from_slice(b"\x1fDE_END n0nc3 42\x1f\r\n");
        let o = parse_outcome(&buf, &n).expect("end marker present, must parse");
        assert_eq!(o.exit_code, 42);
        assert_eq!(o.output, b"no-trailing-newline");
    }

    #[test]
    fn parse_outcome_extracts_output_between_markers() {
        let n = nonce("abc");
        let mut buf = b"prompt-and-echo\r\n".to_vec();
        buf.extend_from_slice(b"\x1fDE_BEG abc\x1f");
        buf.extend_from_slice(b"hello");
        buf.extend_from_slice(b"\x1fDE_END abc 42\x1f");
        let o = parse_outcome(&buf, &n).expect("both markers present");
        assert_eq!(o.exit_code, 42);
        assert_eq!(o.output, b"hello");
    }

    #[test]
    fn parse_outcome_ignores_a_foreign_nonce() {
        let n = nonce("abc");
        let mut buf = b"\x1fDE_BEG abc\x1f".to_vec();
        buf.extend_from_slice(b"output");
        buf.extend_from_slice(b"\x1fDE_END feedfacefeedface 0\x1f");
        assert!(
            parse_outcome(&buf, &n).is_none(),
            "foreign end marker must be ignored"
        );
        buf.extend_from_slice(b"\x1fDE_END abc 7\x1f");
        let o = parse_outcome(&buf, &n).expect("this run's end marker now present");
        assert_eq!(o.exit_code, 7);
        assert_eq!(o.output, b"output\x1fDE_END feedfacefeedface 0\x1f");
    }

    #[test]
    fn parse_outcome_keeps_a_unit_separator_inside_output() {
        let n = nonce("abc");
        let mut buf = b"\x1fDE_BEG abc\x1f".to_vec();
        buf.extend_from_slice(b"a\x1fb");
        buf.extend_from_slice(b"\x1fDE_END abc 3\x1f");
        let o = parse_outcome(&buf, &n).expect("bare unit separator is ordinary output");
        assert_eq!(o.exit_code, 3);
        assert_eq!(o.output, b"a\x1fb");
    }

    #[test]
    fn parse_outcome_rejects_a_non_numeric_exit_field() {
        let n = nonce("abc");
        let mut buf = b"\x1fDE_BEG abc\x1f".to_vec();
        buf.extend_from_slice(b"x");
        buf.extend_from_slice(b"\x1fDE_END abc abc\x1f");
        assert!(
            parse_outcome(&buf, &n).is_none(),
            "non-numeric exit field must yield None"
        );
    }

    #[test]
    fn strip_markers_removes_only_this_nonces_markers() {
        let n = nonce("abc");
        let buf = b"lead \x1fDE_BEG abc\x1f between \x1fDE_END abc 5\x1f tail \x1fDE_END feedfacefeedface 0\x1f end"
            .to_vec();
        let stripped = strip_markers(&buf, &n);
        let text = String::from_utf8_lossy(&stripped);
        assert!(
            !text.contains("DE_BEG"),
            "this run's begin marker removed: {text:?}"
        );
        assert!(
            !text.contains("DE_END abc"),
            "this run's end marker removed: {text:?}"
        );
        assert!(text.contains("lead"), "bytes before our begin marker kept");
        assert!(text.contains(" between "), "bytes between markers kept");
        assert!(text.contains(" tail "), "bytes after our end marker kept");
        assert!(
            text.contains("\x1fDE_END feedfacefeedface 0\x1f"),
            "foreign marker must survive stripping: {text:?}"
        );
        assert_eq!(
            text,
            "lead  between  tail \x1fDE_END feedfacefeedface 0\x1f end"
        );
    }

    #[test]
    fn spawn_passes_argv_to_the_program() {
        let _home = crate::test_home_guard();
        let arg = "argv-probe-ok".to_string();
        let shell = PtyShell::spawn("/bin/echo", &[arg], (24, 80)).expect("echo available");
        poll_for_output(&shell, b"argv-probe-ok");
    }

    #[test]
    fn spawn_without_argv_is_unchanged() {
        let _home = crate::test_home_guard();
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("bash available in the test env");
        let outcome = shell
            .run("printf 'no-argv-ok\n'", Duration::from_secs(10))
            .expect("empty argv still starts a usable shell");
        assert_eq!(outcome.exit_code, 0, "printf exit code");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("no-argv-ok"),
            "empty-argv shell usable: {text:?}"
        );
    }

    /// Wait for the shell's reader task to surface `needle` on the broadcast
    /// stream. `echo` needs no input, so the only wait is the reader's.
    fn poll_for_output(shell: &PtyShell, needle: &[u8]) {
        let mut rx = shell.subscribe();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(chunk) if chunk.windows(needle.len()).any(|w| w == needle) => return,
                _ => {}
            }
            assert!(
                Instant::now() < deadline,
                "output {:?} never appeared within the wait",
                std::str::from_utf8(needle)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_missing_home_still_opens_a_shell() {
        let _pty = PTY_TEST_LOCK.lock().unwrap();
        let _home = crate::test_home_guard();
        let missing =
            std::env::temp_dir().join(format!("daemoneye-pty-missing-home-{}", std::process::id()));
        if missing.exists() {
            std::fs::remove_dir_all(&missing).unwrap();
        }
        unsafe {
            std::env::set_var("HOME", &missing);
        }
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("spawn must succeed with no HOME");
        let out = shell
            .run("pwd", Duration::from_secs(10))
            .expect("pwd must run");
        let text = String::from_utf8_lossy(&out.output);
        assert_eq!(text.trim(), "/", "missing HOME must fall back to /");
    }

    #[test]
    fn a_shell_still_starts_in_home() {
        let _pty = PTY_TEST_LOCK.lock().unwrap();
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("spawn must succeed with a temp HOME");
        let out = shell
            .run("pwd", Duration::from_secs(10))
            .expect("pwd must run");
        let text = String::from_utf8_lossy(&out.output);
        assert_eq!(
            text.trim(),
            dir.path().to_string_lossy(),
            "an existing HOME must be the child's cwd"
        );
    }

    #[test]
    fn pty_run_times_out_on_a_silent_command() {
        let _home = crate::test_home_guard();
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("bash available in the test env");
        let started = Instant::now();
        let budget = Duration::from_secs(2);
        let result = shell.run("sleep 20", budget);
        let elapsed = started.elapsed();
        assert!(result.is_err(), "a silent command must time out and Err");
        assert!(
            elapsed < Duration::from_secs(8),
            "the read must be bounded, not wait out the command: elapsed={elapsed:?} budget={budget:?}"
        );
        let msg = format!("{result:?}");
        assert!(
            msg.contains("timed out") && msg.contains("sleep 20"),
            "error must name the timeout and command: {msg}"
        );
    }

    #[test]
    fn pty_bash_roundtrip_returns_real_exit_code() {
        let _home = crate::test_home_guard();
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("bash available in the test env");
        let outcome = shell
            .run("echo hello; sh -c 'exit 42'", Duration::from_secs(10))
            .expect("the command completes well inside the timeout");
        assert_eq!(outcome.exit_code, 42);
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("hello"),
            "output must contain hello: {text:?}"
        );
        let again = shell
            .run(
                "printf 'second\\n'; sh -c 'exit 7'",
                Duration::from_secs(10),
            )
            .expect("the same shell runs a second command");
        assert_eq!(again.exit_code, 7);
        let second = String::from_utf8_lossy(&again.output);
        assert!(
            second.contains("second"),
            "second run's own output: {second:?}"
        );
    }

    #[test]
    fn pty_runs_many_commands_on_one_shell() {
        let _home = crate::test_home_guard();
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("bash available in the test env");
        for (i, expected_code) in [0, 0, 42, 7, 0].iter().enumerate() {
            let cmd = format!("printf 'out{i}\\n'; sh -c 'exit {expected_code}'");
            let outcome = shell
                .run(&cmd, Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("run {i} failed on a healthy shell: {e}"));
            assert_eq!(outcome.exit_code, *expected_code, "exit code for run {i}");
            let text = String::from_utf8_lossy(&outcome.output);
            assert!(
                text.contains(&format!("out{i}")),
                "run {i} must return its own output: {text:?}"
            );
        }
    }

    #[test]
    fn pty_shell_is_usable_after_a_timeout() {
        let _home = crate::test_home_guard();
        let mut shell =
            PtyShell::spawn("bash", &[], (24, 80)).expect("bash available in the test env");
        let logs = [false, true, false];
        for (i, expect_timeout) in logs.iter().enumerate() {
            let cmd = if *expect_timeout {
                "sleep 30".to_string()
            } else {
                format!("printf 'ok{i}\\n'; sh -c 'exit 0'")
            };
            let budget = if *expect_timeout {
                Duration::from_millis(300)
            } else {
                Duration::from_secs(10)
            };
            let before = Instant::now();
            let result = shell.run(&cmd, budget);
            let elapsed = before.elapsed();
            if *expect_timeout {
                assert!(
                    result.is_err(),
                    "run {i} with sleep 30 under a {budget:?} budget must Err"
                );
                assert!(
                    elapsed < Duration::from_secs(2),
                    "run {i} must fail fast, not wait out the command: elapsed={elapsed:?}"
                );
                continue;
            }
            let outcome = result.unwrap_or_else(|e| panic!("run {i} after a timeout failed: {e}"));
            assert_eq!(outcome.exit_code, 0, "run {i} exit code");
            let text = String::from_utf8_lossy(&outcome.output);
            assert!(
                text.contains(&format!("ok{i}")),
                "run {i} must return its own output: {text:?}"
            );
        }
    }
}
