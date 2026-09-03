//! PTY-backed shell spawn and the marker protocol.
//!
//! A command is wrapped in BEGIN/END markers printed through `printf`; the
//! bytes between the markers are the command's exact output and the field
//! after the END marker is its real exit code. Everything but [`PtyShell`]
//! itself is pure over byte slices, so the protocol is testable without a PTY.

use anyhow::Context;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A per-command marker nonce. 128 bits of randomness rendered as 32 hex
/// characters, so a command's own output cannot plausibly collide with it.
pub struct Nonce(String);

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

    Some(CommandOutcome { output, exit_code })
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

fn begin_marker(nonce: &Nonce) -> Vec<u8> {
    format!("\u{1f}DE_BEG {}\u{1f}", nonce.as_str()).into_bytes()
}

fn end_marker(nonce: &Nonce) -> Vec<u8> {
    format!("\u{1f}DE_END {} ", nonce.as_str()).into_bytes()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A PTY-backed shell. Owns the master, a cloned reader, the writer, and the
/// spawned child.
pub struct PtyShell {
    shell: String,
    master: Box<dyn MasterPty + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl PtyShell {
    /// Spawn `shell` (a path like `/bin/bash` or a bare name) in a new PTY of
    /// `(rows, cols)`.
    pub fn spawn(shell: &str, size: (u16, u16)) -> anyhow::Result<Self> {
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
        cmd.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("spawn_command failed")?;
        // Without dropping the slave, the master never sees EOF.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("try_clone_reader failed")?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("take_writer failed")?;

        Ok(Self {
            shell: shell.to_string(),
            master: pair.master,
            reader,
            writer,
            child,
        })
    }

    /// Run `cmd` in the shell, returning its output and real exit code.
    ///
    /// Generates a fresh nonce, writes the wrapped command, then accumulates
    /// reads until the end marker for that nonce arrives or `timeout` passes.
    /// On timeout returns an `Err` naming the timeout and the command — never
    /// a fabricated exit code.
    pub fn run(&mut self, cmd: &str, timeout: Duration) -> anyhow::Result<CommandOutcome> {
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

        // The reader is a blocking OS fd, so an unbounded read would park past
        // the deadline on a silent command. `take_reader` moves it to a worker
        // thread that owns the blocking read and delivers each chunk on a
        // channel; reading the channel's rx end is what we bound.
        let (reader_handle, rx) = take_reader(&mut self.reader);
        let mut buf: Vec<u8> = Vec::new();
        let start = Instant::now();
        loop {
            let remaining = timeout
                .checked_sub(start.elapsed())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                drop(rx);
                drop(reader_handle); // worker will exit on its own once the shell writes or dies
                self.refresh_reader()?;
                return Err(anyhow::anyhow!(
                    "timed out after {timeout:?} waiting for command output: {cmd:?}"
                ));
            }
            match rx.recv_timeout(remaining) {
                Ok(chunk) => buf.extend_from_slice(&chunk),
                Err(RecvTimeoutError::Timeout) => {
                    drop(rx);
                    drop(reader_handle); // worker will exit on its own once the shell writes or dies
                    self.refresh_reader()?;
                    return Err(anyhow::anyhow!(
                        "timed out after {timeout:?} waiting for command output: {cmd:?}"
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.refresh_reader()?;
                    return Err(anyhow::anyhow!("PTY closed while running {cmd:?}"));
                }
            }
            if let Some(outcome) = parse_outcome(&buf, &nonce) {
                drop(rx);
                drop(reader_handle); // worker finished once it delivered the end marker
                return Ok(outcome);
            }
        }
    }

    /// Re-seat `reader` with a fresh clone from the master so the next `run`
    /// has a live fd (the previous one lives in a detached worker until the
    /// shell writes again).
    fn refresh_reader(&mut self) -> anyhow::Result<()> {
        self.reader = self
            .master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("re-seat reader clone")?;
        Ok(())
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

/// Move `reader` to a worker thread that reads until EOF/error, sending each
/// chunk on a synchronous channel; the caller is handed the receiver so it can
/// bound each wait with `recv_timeout`.
fn take_reader(reader: &mut Box<dyn Read + Send>) -> (JoinHandle<()>, mpsc::Receiver<Vec<u8>>) {
    let mut reader = std::mem::replace(reader, Box::new(std::io::empty()));
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    (handle, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut buf = b"matt@scrappy:~$ printf '\\x1fDE_''BEG n0nc3\\x1f\\n'; printf 'no-trailing-newline'; (exit 42); printf '\\n\\x1fDE_''END n0nc3 %s\\x1f\\n' $?\r\n".to_vec();
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
    fn pty_run_times_out_on_a_silent_command() {
        let mut shell = PtyShell::spawn("bash", (24, 80)).expect("bash available in the test env");
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
        let mut shell = PtyShell::spawn("bash", (24, 80)).expect("bash available in the test env");
        let outcome = shell
            .run("echo hello; sh -c 'exit 42'", Duration::from_secs(10))
            .expect("the command completes well inside the timeout");
        assert_eq!(outcome.exit_code, 42);
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("hello"),
            "output must contain hello: {text:?}"
        );
    }
}
