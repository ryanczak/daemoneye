//! Client for a shell host's socket: run a marker-wrapped command over the
//! wire frames defined in `proto.rs` and parse the real outcome from the
//! accumulated chunk stream.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::shell::proto::{ShellRequest, ShellResponse, ShellSignal, ShellState, encode};
use crate::shell::pty::{CommandOutcome, Nonce, parse_outcome, wrap_command};
#[cfg(test)]
use crate::shell::test_begin_marker;
#[cfg(test)]
use crate::shell::test_end_marker;

/// How long one prompt answer may take to reach the shell. Bounded well below
/// any plausible command timeout so a wedged host cannot wedge the caller.
const PROMPT_ROUND_TIMEOUT: Duration = Duration::from_secs(10);

/// How often `read_to_marker` wakes to ask the host whether the shell's
/// program is still alive. 500 ms keeps a dead shell's failure well under
/// "a few seconds" while staying off a live command's critical path.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// One `status` probe's own budget. Shorter than a poll interval so the probe
/// itself cannot delay the next slice by more than it saves.
const EXIT_STATUS_TIMEOUT: Duration = Duration::from_secs(1);

/// How long `read_to_marker` keeps draining after the host says the shell's
/// program has exited: an end marker for a command that ran to completion may
/// already be in flight on the connection.
const EXIT_DRAIN_WINDOW: Duration = Duration::from_millis(250);

/// The shell's program exited before this command's end marker arrived.
///
/// Distinct from a timeout on purpose: nothing the caller wrote exceeded any
/// limit, and the command is *not* still running — its shell is gone. The
/// error model sees it and stops saying "may still be running"; the caller is
/// told to aim the command at a live shell instead of retrying into a dead one.
#[derive(Debug)]
pub struct ShellExited {
    /// The command that was sent into the dead shell, as it was sent.
    pub cmd: String,
}

impl std::fmt::Display for ShellExited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "shell exited before the command finished: {:?}",
            self.cmd
        )
    }
}

impl std::error::Error for ShellExited {}

/// Send `sig` to the shell hosted at `sock`. Returns when the host has
/// acknowledged, which is not the same as the job having reacted.
pub async fn send_signal(sock: &Path, sig: ShellSignal, timeout: Duration) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Signal { signal: sig }).context("encoding Signal frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Signal frame")?;
    writer.flush().await.context("flushing Signal frame")?;

    read_ack(sock, &mut reader, timeout).await
}

/// Write `bytes` into the shell hosted at `sock`. Returns when the host has
/// acknowledged the write, which is not the same as the program having
/// reacted — reading what happened is `read_shell`'s or `watch_shell`'s job.
pub async fn send_input(sock: &Path, bytes: &[u8], timeout: Duration) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Input {
        bytes: bytes.to_vec(),
    })
    .context("encoding Input frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Input frame")?;
    writer.flush().await.context("flushing Input frame")?;

    read_ack(sock, &mut reader, timeout).await
}

/// Resize the shell hosted at `sock` to `rows` × `cols`. Returns when the host
/// has acknowledged, which is not the same as the program having reflowed —
/// reading what happened is `read_shell`'s or `watch_shell`'s job.
pub async fn send_resize(
    sock: &Path,
    rows: u16,
    cols: u16,
    timeout: Duration,
) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Resize { rows, cols }).context("encoding Resize frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Resize frame")?;
    writer.flush().await.context("flushing Resize frame")?;

    read_ack(sock, &mut reader, timeout).await
}

/// Ask the host to end its shell (`ShellRequest::Close`). A host from before
/// `Close` refuses the frame, so fall back to `Signal { TERM }`, the only
/// closure it understands.
pub async fn close_or_term(sock: &Path, timeout: Duration) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let wire = encode(ShellRequest::Close).context("encoding Close frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Close frame")?;
    writer.flush().await.context("flushing Close frame")?;
    match read_ack(sock, &mut reader, timeout).await {
        Ok(()) => Ok(()),
        Err(_) => send_signal(sock, ShellSignal::Term, timeout).await,
    }
}

/// The shell's current state, rows, cols and pid.
pub async fn status(sock: &Path, timeout: Duration) -> anyhow::Result<ShellResponse> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Status).context("encoding Status frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Status frame")?;
    writer.flush().await.context("flushing Status frame")?;

    read_one(sock, &mut reader, timeout).await
}

/// Read the shell's rendered screen from the host at `sock`: the last
/// `scrollback` rows that scrolled off the top prepended to the visible
/// screen, inverse-video runs marked `[SEL: …]`, and — when `annotated` —
/// the colour runs marked too.
pub async fn screen(
    sock: &Path,
    annotated: bool,
    scrollback: u16,
    timeout: Duration,
) -> anyhow::Result<ShellResponse> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Screen {
        annotated,
        scrollback,
    })
    .context("encoding Screen frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Screen frame")?;
    writer.flush().await.context("flushing Screen frame")?;

    read_one(sock, &mut reader, timeout).await
}

/// The single answer frame of a one-shot request that expects `Ok`.
async fn read_ack(
    sock: &Path,
    reader: &mut (impl AsyncBufReadExt + Unpin),
    timeout: Duration,
) -> anyhow::Result<()> {
    let frame = read_one(sock, reader, timeout).await?;
    match frame {
        ShellResponse::Ok => Ok(()),
        ShellResponse::Error { message } => Err(anyhow::anyhow!(
            "shell host at {} rejected the request: {message}",
            sock.display()
        )),
        other => Err(anyhow::anyhow!(
            "shell host at {} answered a signal with {other:?}",
            sock.display()
        )),
    }
}

/// Read exactly one response frame, bounded by `timeout`.
async fn read_one(
    sock: &Path,
    reader: &mut (impl AsyncBufReadExt + Unpin),
    timeout: Duration,
) -> anyhow::Result<ShellResponse> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut line: Vec<u8> = Vec::new();
    let read = tokio::time::timeout_at(deadline, reader.read_until(b'\n', &mut line)).await;
    let n = match read {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return Err(e).with_context(|| format!("reading frame from {}", sock.display()));
        }
        Err(_) => anyhow::bail!(
            "timed out after {timeout:?} waiting for the shell host's reply at {}",
            sock.display()
        ),
    };
    if n == 0 {
        anyhow::bail!("shell socket at {} closed before a reply", sock.display());
    }
    serde_json::from_slice::<ShellResponse>(&line).with_context(|| {
        format!(
            "malformed frame from {} while expecting one reply",
            sock.display()
        )
    })
}

/// What the prompt handler decided about the output so far.
#[derive(Debug, PartialEq, Eq)]
pub enum PromptReply {
    /// Nothing to answer; keep reading.
    Wait,
    /// Write these bytes into the shell, then keep reading.
    Send(Vec<u8>),
    /// Stop: this prompt cannot be answered. The command is abandoned. The
    /// string is the reason, surfaced in the returned error.
    GiveUp(String),
}

/// A prompt the caller could not answer was cancelled with Ctrl-C.
#[derive(Debug)]
pub struct PromptGivenUp {
    /// Why the prompt was abandoned, in the caller's words.
    pub reason: String,
}

impl std::fmt::Display for PromptGivenUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for PromptGivenUp {}

/// How long the shell's output must have been quiet after a Ctrl-C give-up
/// before the caller may reuse it: sudo-rs has already read the next
/// command's bytes as password characters when they land in the same
/// millisecond as the cancel.
const GIVE_UP_QUIET_WINDOW: Duration = Duration::from_millis(300);

/// Hard cap on the give-up settle wait, so a chatty shell cannot stretch the
/// abandon past a bounded time.
const GIVE_UP_SETTLE_CAP: Duration = Duration::from_secs(3);

/// The byte written to cancel a prompt the caller could not answer: a real
/// sudo treats an empty newline as a wrong password and re-prompts.
const GIVE_UP_BYTES: &[u8] = b"\x03";

/// Run `cmd` in the shell hosted at `sock` and return its real outcome, with
/// no prompt handling: the always-[`PromptReply::Wait`] callback keeps every
/// frame flowing exactly as an unwrapped read loop would.
///
/// Opens a single connection, subscribes first so no byte of this command's
/// output can precede the subscription, sends the marker-wrapped command as an
/// `Input` frame, then accumulates `Chunk` frames until `parse_outcome`
/// answers or `timeout` elapses. Every other response frame is ignored —
/// `Ok`, `Status`, `Exited` and `Error` may interleave and are not this
/// command's output. On timeout the error names the command and the timeout;
/// a partial outcome is never returned and an exit code is never invented.
pub async fn run_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
) -> anyhow::Result<CommandOutcome> {
    run_command_with_prompts(sock, cmd, shell_name, timeout, |_: &str| async {
        PromptReply::Wait
    })
    .await
}

/// [`run_command`], reacting to the output while the command is still running.
///
/// Each time a `Chunk` arrives the accumulated output so far is lossy-decoded
/// and handed to `on_chunk`. Its [`PromptReply`] drives the connection:
///
/// - [`PromptReply::Wait`] — keep reading, as `run_command` does.
/// - [`PromptReply::Send(bytes)`] — the bytes are written through **the same
///   single connection's writer**, exactly as the command itself was (the
///   write half is load-bearing: dropping it half-shuts the stream and the
///   host tears the connection down before the end marker arrives). Reading
///   then continues for this command's end marker.
/// - [`PromptReply::GiveUp(reason)`] — cancel the prompt with the single byte
///   `0x03` (Ctrl-C, an input byte, not a signal frame): a real sudo treats
///   an empty newline as a wrong password and re-prompts, eating the next
///   command's bytes. The shell's output is then given time to settle —
///   quiet for [`GIVE_UP_QUIET_WINDOW`], capped at [`GIVE_UP_SETTLE_CAP`] —
///   so the caller's next command cannot be read as password characters, and
///   the reason is returned as an error whose chain carries
///   [`PromptGivenUp`]. The command's end marker is **not** awaited: on
///   sudo-rs it never comes after a cancel.
///
/// A decision is acted on as it is returned. `on_chunk` is free to answer a
/// later chunk differently — there is no internal dedup — so a caller that
/// must answer a given prompt only once tracks that itself (Task 4 does, by
/// root the answer in the sentinel and a fresh sentinel per command).
pub async fn run_command_with_prompts<F, Fut>(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
    mut on_chunk: F,
) -> anyhow::Result<CommandOutcome>
where
    F: FnMut(&str) -> Fut,
    Fut: std::future::Future<Output = PromptReply>,
{
    let (mut reader, mut writer, nonce) = start_command(sock, cmd, shell_name).await?;
    // The writing half must outlive the read loop: tokio shuts the stream's
    // write direction down when it is dropped, and a host that reads EOF on
    // its side tears the connection down — before the end marker could ever
    // arrive. So it is dropped only after the read loop returns. Prompt
    // answers flow through this same writer, never a second connection.
    let result = read_to_marker(
        &mut reader,
        &nonce,
        sock,
        cmd,
        timeout,
        &mut writer,
        &mut on_chunk,
    )
    .await;
    drop(writer);
    result
}

/// Connect, subscribe, and write the marker-wrapped command. Returns the
/// reader, the still-open write half, and the nonce the command's end marker
/// will carry. Everything that can fail *before the command is running* fails
/// here. The caller keeps the write half alive until it is done with the
/// connection: dropping it half-shuts the stream (tokio's documented
/// `OwnedWriteHalf` drop), which the host reads as end-of-writes.
async fn start_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
) -> anyhow::Result<(
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
    Nonce,
)> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);

    // Subscribe before writing, so no byte of this command's output can
    // precede the subscription.
    let wire = encode(ShellRequest::Subscribe).context("encoding Subscribe frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Subscribe frame")?;

    let nonce = Nonce::new();
    let wrapped = wrap_command(cmd, &nonce, shell_name);
    let wire = encode(ShellRequest::Input {
        bytes: wrapped.into_bytes(),
    })
    .context("encoding Input frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Input frame")?;
    writer.flush().await.context("flushing Input frame")?;
    Ok((reader, writer, nonce))
}

/// Read frames until `parse_outcome` finds this nonce's end marker, the
/// deadline passes, or the socket closes.
///
/// This is the one implementation of the framing: `run_command` and
/// `spawn_command` both get their outcome through it, so the two can never
/// diverge on frame handling, the nonce or the write-half lifetime rule.
///
/// `writer` is borrowed rather than consumed so the prompt-detector variant
/// (`run_command_with_prompts`) can answer a prompt through the *same*
/// connection's write half while the read loop runs — the [`PromptReply`] on
/// each chunk comes from `on_chunk`, which sees the accumulated output
/// lossy-decoded.
#[allow(clippy::too_many_arguments)]
async fn read_to_marker<W, F, Fut>(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    nonce: &Nonce,
    sock: &Path,
    cmd: &str,
    timeout: Duration,
    writer: &mut W,
    on_chunk: &mut F,
) -> anyhow::Result<CommandOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    F: FnMut(&str) -> Fut,
    Fut: std::future::Future<Output = PromptReply>,
{
    // The byte offset in `buf` up to which the output was already offered to
    // `on_chunk` and its decision acted on. A prompt still on screen in the
    // next chunk is not offered again — the callback sees only output that
    // arrived **after** the last `Send`/`GiveUp` — so a prompt already
    // answered cannot re-trigger its own answer.
    let mut offered_at: usize = 0;
    let mut deadline = tokio::time::Instant::now() + timeout;
    // A slice that elapses may leave a half frame in `line`: `read_until` is
    // not cancellation-safe, and tokio keeps whatever it had appended. So
    // `line` is cleared only after a *complete* frame has been parsed — after
    // a slice the next `read_until` continues filling it, and the frame is
    // parsed from the accumulated bytes, never from the last read's count.
    let mut line: Vec<u8> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    // When the host reports the shell's program dead, drain this long for an
    // end marker that may already be in flight; past that, the shell has
    // exited and this command will never complete.
    let mut drain_until: Option<tokio::time::Instant> = None;
    loop {
        let slice_end = match drain_until {
            Some(end) => end,
            None => deadline.min(tokio::time::Instant::now() + EXIT_POLL_INTERVAL),
        };
        let slice_started = tokio::time::Instant::now();
        let read = tokio::time::timeout_at(slice_end, reader.read_until(b'\n', &mut line)).await;
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                return Err(e).with_context(|| format!("reading frame from {}", sock.display()));
            }
            Err(_) => {
                if let Some(end) = drain_until {
                    // The drain window is over and no end marker arrived: the
                    // shell's program is gone and this command will never
                    // finish.
                    if tokio::time::Instant::now() >= end {
                        return Err(ShellExited {
                            cmd: cmd.to_string(),
                        }
                        .into());
                    }
                } else {
                    match status(sock, EXIT_STATUS_TIMEOUT).await {
                        Ok(ShellResponse::Status {
                            state: ShellState::Exited,
                            ..
                        }) => {
                            drain_until = Some(tokio::time::Instant::now() + EXIT_DRAIN_WINDOW);
                        }
                        Ok(ShellResponse::Status {
                            state: ShellState::Paused,
                            ..
                        }) => {
                            // A pause is not the command's run time either, so
                            // this slice's paused span is added back to the
                            // deadline, the same way prompt time is.
                            deadline += slice_started.elapsed();
                        }
                        _ => {}
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "timed out after {timeout:?} waiting for command output: {cmd:?}"
                    );
                }
                continue;
            }
        };
        if n == 0 {
            return Err(ShellExited {
                cmd: cmd.to_string(),
            })
            .with_context(|| {
                format!(
                    "shell socket at {} closed while running {cmd:?}",
                    sock.display()
                )
            });
        }
        let frame = match serde_json::from_slice::<ShellResponse>(&line) {
            Ok(frame) => frame,
            Err(e) => {
                anyhow::bail!(
                    "malformed frame from {} while running {cmd:?}: {e}",
                    sock.display()
                );
            }
        };
        line.clear();
        match frame {
            ShellResponse::Chunk { bytes } => {
                // Chunks split arbitrarily (a PTY read can land mid-sequence),
                // so never decode a chunk on its own — accumulate bytes and
                // hand the whole buffer to `parse_outcome`.
                buf.extend_from_slice(&bytes);
                if let Some(outcome) = parse_outcome(&buf, nonce) {
                    return Ok(outcome);
                }
                if buf.len() > offered_at {
                    let seen = String::from_utf8_lossy(&buf[offered_at..]).into_owned();
                    // The callback blocks while a prompt waits for the user's
                    // answer. That wait is not the command's run time, so it is
                    // added back to the deadline: a slow typist is not cut off
                    // after their password was already sent.
                    let asked = tokio::time::Instant::now();
                    let reply = on_chunk(&seen).await;
                    deadline += asked.elapsed();
                    match reply {
                        PromptReply::Wait => {}
                        PromptReply::Send(answer) => {
                            write_answer(writer, &answer).await?;
                            offered_at = buf.len();
                        }
                        PromptReply::GiveUp(reason) => {
                            // Cancel the prompt with Ctrl-C — a real sudo
                            // treats an empty newline as a wrong password and
                            // re-prompts, eating the next command's bytes —
                            // then give the shell time to settle before the
                            // caller reuses it. Never the credential.
                            write_answer(writer, GIVE_UP_BYTES).await?;
                            let cap = tokio::time::Instant::now() + GIVE_UP_SETTLE_CAP;
                            loop {
                                let quiet = tokio::time::Instant::now() + GIVE_UP_QUIET_WINDOW;
                                let mut scratch = Vec::new();
                                match tokio::time::timeout_at(
                                    quiet.min(cap),
                                    reader.read_until(b'\n', &mut scratch),
                                )
                                .await
                                {
                                    Ok(Ok(n)) if n > 0 && tokio::time::Instant::now() < cap => {
                                        continue;
                                    }
                                    _ => break,
                                }
                            }
                            return Err(PromptGivenUp { reason }.into());
                        }
                    }
                }
            }
            _ => {
                // `Ok`, `Status`, `Exited` and `Error` may interleave with the
                // command's own output and are not part of it.
            }
        }
    }
}

/// Write the answer through the same connection's write half. `PROMPT_ROUND`
/// bounds the write so a wedged host cannot wedge the caller.
async fn write_answer<W>(writer: &mut W, bytes: &[u8]) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let wire = encode(ShellRequest::Input {
        bytes: bytes.to_vec(),
    })?;
    tokio::time::timeout(PROMPT_ROUND_TIMEOUT, async {
        writer.write_all(wire.as_bytes()).await?;
        writer.flush().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out writing the prompt answer into the shell"))??;
    Ok(())
}

/// A command that was started in a shell but not awaited.
#[derive(Debug)]
pub struct CommandHandle {
    /// Resolves once the end marker arrives, the timeout expires, or the
    /// socket closes. A dropped receiver simply discards the outcome; the
    /// reader task ends at its own deadline either way.
    pub outcome: tokio::sync::oneshot::Receiver<anyhow::Result<CommandOutcome>>,
    /// The nonce this command's end marker carries — diagnostic only. The
    /// reader task holds its own clone, since `read_to_marker` needs the live
    /// `Nonce` to strip markers with.
    pub nonce: Nonce,
}

/// Start `cmd` in the shell at `sock` and return **as soon as it is running**.
///
/// The handshake — connect, subscribe, write, flush — completes before this
/// returns, so a missing socket, a dead host or an encode failure is an `Err`
/// from this call. After `Ok`, the command is running and its outcome arrives
/// on [`CommandHandle::outcome`]; a start failure is never delivered as a late
/// outcome, because the caller must be able to tell the model "it is running"
/// or "it did not start" synchronously.
///
/// The write half moves into the reader task and is dropped only when that
/// task ends, so the connection stays open in both directions for as long as
/// the outcome can still arrive — the host must not see EOF on its read side
/// while the command runs.
pub async fn spawn_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
) -> anyhow::Result<CommandHandle> {
    let (reader, mut writer, nonce) = start_command(sock, cmd, shell_name).await?;
    let task_sock = sock.to_path_buf();
    let task_cmd = cmd.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task_nonce = nonce.clone();
    tokio::spawn(async move {
        let mut reader = reader;
        let mut never_wait = |_: &str| async { PromptReply::Wait };
        let result = read_to_marker(
            &mut reader,
            &task_nonce,
            &task_sock,
            &task_cmd,
            timeout,
            &mut writer,
            &mut never_wait,
        )
        .await;
        // The writer must outlive the read loop: dropping it half-closes the
        // stream and the host tears the connection down before the end marker
        // can arrive. It dies here, when the outcome has already been handed
        // over.
        drop(writer);
        let _ = tx.send(result);
    });
    Ok(CommandHandle { outcome: rx, nonce })
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
