//! Frame-reading state machine for marker-wrapped shell commands.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{
    CommandTimedOut, EXIT_DRAIN_WINDOW, EXIT_POLL_INTERVAL, EXIT_STATUS_TIMEOUT, GIVE_UP_BYTES,
    GIVE_UP_QUIET_WINDOW, GIVE_UP_SETTLE_CAP, INTERRUPT_GRACE, OFFER_WINDOW, PROMPT_ROUND_TIMEOUT,
    PromptGivenUp, PromptReply, ShellExited, status,
};
use crate::shell::proto::{ShellRequest, ShellResponse, ShellState, encode};
use crate::shell::pty::{CommandOutcome, Nonce, end_marker, parse_outcome};

/// Handle a timeout slice in [`read_to_marker`]: poll whether the shell has
/// exited or paused, extend `deadline` if paused, or type Ctrl-C into the PTY
/// on initial timeout and wait for [`INTERRUPT_GRACE`].
#[allow(clippy::too_many_arguments)]
async fn handle_read_timeout<W>(
    sock: &Path,
    cmd: &str,
    timeout: Duration,
    slice_started: tokio::time::Instant,
    drain_until: &mut Option<tokio::time::Instant>,
    interrupted_until: &mut Option<tokio::time::Instant>,
    deadline: &mut tokio::time::Instant,
    writer: &mut W,
) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Some(end) = *drain_until {
        // The drain window is over and no end marker arrived: the
        // shell's program is gone and this command will never finish.
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
                *drain_until = Some(tokio::time::Instant::now() + EXIT_DRAIN_WINDOW);
            }
            Ok(ShellResponse::Status {
                state: ShellState::Paused,
                ..
            }) => {
                // A pause is not the command's run time either, so
                // this slice's paused span is added back to the
                // deadline, the same way prompt time is.
                *deadline += slice_started.elapsed();
            }
            _ => {}
        }
    }
    if tokio::time::Instant::now() >= *deadline {
        match *interrupted_until {
            None => {
                // Free the shell the way the prompt give-up path
                // does: the Ctrl-C is typed as input into the
                // PTY, so it reaches whatever holds the terminal
                // — local or remote. `send_signal` would signal
                // the PTY's foreground process group, which on a
                // remote shell is the local ssh itself.
                write_answer(writer, GIVE_UP_BYTES).await?;
                let grace = tokio::time::Instant::now() + INTERRUPT_GRACE;
                *interrupted_until = Some(grace);
                *deadline = grace;
            }
            Some(_) => {
                return Err(CommandTimedOut {
                    timeout,
                    cmd: cmd.to_string(),
                    interrupted: None,
                }
                .into());
            }
        }
    }
    Ok(())
}

/// Offer any newly accumulated output tail in `buf[offered_at..]` to
/// `on_chunk`, extending `deadline` by the time spent awaiting the callback
/// and writing the answer or cancelling with Ctrl-C on `GiveUp`.
async fn offer_chunk_to_prompt_handler<W, F, Fut>(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    buf: &[u8],
    offered_at: &mut usize,
    deadline: &mut tokio::time::Instant,
    writer: &mut W,
    on_chunk: &mut F,
) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    F: FnMut(&str) -> Fut,
    Fut: std::future::Future<Output = PromptReply>,
{
    if buf.len() <= *offered_at {
        return Ok(());
    }
    let since = &buf[*offered_at..];
    // A prompt sits at the stream's tail, so only the last
    // window is offered: the decoded offer stays bounded no
    // matter how much output arrived since the last answer.
    let window = since.len().saturating_sub(OFFER_WINDOW);
    let seen = String::from_utf8_lossy(&since[window..]).into_owned();
    // The callback blocks while a prompt waits for the user's
    // answer. That wait is not the command's run time, so it is
    // added back to the deadline: a slow typist is not cut off
    // after their password was already sent.
    let asked = tokio::time::Instant::now();
    let reply = on_chunk(&seen).await;
    *deadline += asked.elapsed();
    match reply {
        PromptReply::Wait => Ok(()),
        PromptReply::Send(answer) => {
            write_answer(writer, &answer).await?;
            *offered_at = buf.len();
            Ok(())
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
            Err(PromptGivenUp { reason }.into())
        }
    }
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
pub(super) async fn read_to_marker<W, F, Fut>(
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
    let mut offered_at: usize = 0;
    let mut deadline = tokio::time::Instant::now() + timeout;
    let mut line: Vec<u8> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut end_seen = false;
    let mut drain_until: Option<tokio::time::Instant> = None;
    let mut interrupted_until: Option<tokio::time::Instant> = None;
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
                handle_read_timeout(
                    sock,
                    cmd,
                    timeout,
                    slice_started,
                    &mut drain_until,
                    &mut interrupted_until,
                    &mut deadline,
                    writer,
                )
                .await?;
                continue;
            }
        };
        if n == 0 && interrupted_until.is_none() {
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
        if n == 0 {
            return Err(CommandTimedOut {
                timeout,
                cmd: cmd.to_string(),
                interrupted: None,
            }
            .into());
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
                buf.extend_from_slice(&bytes);
                if end_seen || {
                    let overlap = end_marker(nonce).len().saturating_sub(1);
                    let scan_from = buf.len().saturating_sub(bytes.len() + overlap);
                    scan_from < buf.len()
                        && buf[scan_from..]
                            .windows(end_marker(nonce).len())
                            .any(|w| w == end_marker(nonce).as_slice())
                } {
                    end_seen = true;
                    if let Some(outcome) = parse_outcome(&buf, nonce) {
                        if interrupted_until.is_some() {
                            return Err(CommandTimedOut {
                                timeout,
                                cmd: cmd.to_string(),
                                interrupted: Some(outcome),
                            }
                            .into());
                        }
                        return Ok(outcome);
                    }
                }
                offer_chunk_to_prompt_handler(
                    reader,
                    &buf,
                    &mut offered_at,
                    &mut deadline,
                    writer,
                    on_chunk,
                )
                .await?;
            }
            ShellResponse::Lagged { skipped } => {
                buf.extend_from_slice(
                    format!(
                        "\r\n[daemoneye: {skipped} output chunks dropped here: the reader fell behind]\r\n"
                    )
                    .as_bytes(),
                );
            }
            _ => {}
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
