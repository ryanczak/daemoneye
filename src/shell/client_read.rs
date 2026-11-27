//! Frame-reading state machine for marker-wrapped shell commands.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use zeroize::Zeroize;

use super::{
    CommandTimedOut, EXIT_DRAIN_WINDOW, EXIT_POLL_INTERVAL, EXIT_STATUS_TIMEOUT, GIVE_UP_BYTES,
    GIVE_UP_QUIET_WINDOW, GIVE_UP_SETTLE_CAP, INTERRUPT_GRACE, OFFER_WINDOW, OUTPUT_CAP,
    PROMPT_ROUND_TIMEOUT, PromptGivenUp, PromptReply, ShellExited, status,
};
use crate::shell::proto::{ShellRequest, ShellResponse, ShellState, encode};
use crate::shell::pty::{
    CommandOutcome, Nonce, begin_marker, end_marker, find_subslice, parse_outcome,
};

/// How far below [`OUTPUT_CAP`] a cut targets the buffer, as a fraction of
/// the cap: cutting to this fraction means at least this fraction of the cap
/// in new output arrives before the next cut, so steady output past the cap
/// amortizes to one cut per that many bytes instead of copying the whole
/// tail on every chunk.
const CUT_HEADROOM_FRACTION: usize = 3;

/// Cut-to length for a cap: below the cap by the headroom fraction.
fn cut_target(cap: usize) -> usize {
    cap.saturating_sub(cap / CUT_HEADROOM_FRACTION)
}

/// Accumulated output with a middle-cut at [`OUTPUT_CAP`]. The head through
/// the run's begin marker is kept (`parse_outcome` needs it), the most recent
/// tail is kept, and a one-line note marks where the cut happened, like the
/// `Lagged` arm does. The note names the *total* bytes dropped since the
/// begin marker — not the last cut's overflow — so there is always exactly
/// one note: the previous note sits at the head of the region being dropped
/// and is removed with it, and its synthetic bytes are not counted as dropped
/// output. A cut targets [`cut_target`], well below the cap, so the next cut
/// is far off: cuts amortize over the headroom instead of re-copying the
/// tail on every chunk. Each [`absorb`](Self::absorb) folds one chunk in;
/// a cut adjusts `offered_at` so it never points past the buffer.
pub(super) struct OutputCutter {
    cap: usize,
    beg: Vec<u8>,
    /// End of the kept head: one past the begin marker's last byte, or `None`
    /// while the marker has not been seen yet.
    head_end: Option<usize>,
    /// How much of the buffer has been searched for the begin marker, so the
    /// search is incremental and never re-scans old bytes.
    searched: usize,
    /// Length of the note currently in the buffer, `0` before the first cut.
    note_len: usize,
    dropped_total: usize,
}

impl OutputCutter {
    pub(super) fn new(cap: usize, nonce: &Nonce) -> Self {
        Self {
            cap,
            beg: begin_marker(nonce),
            head_end: None,
            searched: 0,
            note_len: 0,
            dropped_total: 0,
        }
    }

    fn find_head(&mut self, buf: &[u8]) {
        if self.head_end.is_some() {
            return;
        }
        let overlap = self.beg.len().saturating_sub(1);
        let from = self.searched.saturating_sub(overlap);
        if let Some(pos) = find_subslice(&buf[from..], &self.beg) {
            self.head_end = Some(from + pos + self.beg.len());
        } else {
            self.searched = buf.len();
        }
    }

    /// Fold one chunk in. Returns `Some(())` when a cut happened, so a test
    /// can count the cuts without reaching into the buffer.
    pub(super) fn absorb(
        &mut self,
        buf: &mut Vec<u8>,
        chunk: &[u8],
        offered_at: &mut usize,
    ) -> Option<()> {
        buf.extend_from_slice(chunk);
        self.find_head(buf);
        if buf.len() <= self.cap {
            return None;
        }
        let head_end = self.head_end.unwrap_or(0).min(buf.len());
        let keep_tail = cut_target(self.cap).saturating_sub(head_end);
        let tail_start = buf.len().saturating_sub(keep_tail).max(head_end);
        let removed = tail_start - head_end;
        // The region starts with the framing CRLF `parse_outcome` strips
        // (first cut) or the previous note (later cuts); neither is output
        // the command printed, so neither counts as dropped.
        let synthetic = if self.note_len == 0 {
            usize::from(buf[head_end..].starts_with(b"\r\n")) * 2
        } else {
            self.note_len
        };
        self.dropped_total += removed.saturating_sub(synthetic);
        let note = format!(
            "\r\n[daemoneye: {} bytes of output dropped here]\r\n",
            self.dropped_total
        );
        let note_len = note.len();
        buf.splice(head_end..tail_start, note.into_bytes());
        self.note_len = note_len;
        *offered_at = (*offered_at).min(buf.len());
        Some(())
    }
}

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
    let mut cutter = OutputCutter::new(OUTPUT_CAP, nonce);
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
                cutter.absorb(&mut buf, &bytes, &mut offered_at);
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
/// bounds the write so a wedged host cannot wedge the caller. The bytes and
/// the encoded wire string may both carry a credential, so both are zeroized
/// on the way out — each after its last use.
async fn write_answer<W>(writer: &mut W, bytes: &[u8]) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let mut req = ShellRequest::Input {
        bytes: bytes.to_vec(),
    };
    let wire = zeroize::Zeroizing::new(encode(&req)?);
    if let ShellRequest::Input { bytes } = &mut req {
        bytes.zeroize();
    }
    tokio::time::timeout(PROMPT_ROUND_TIMEOUT, async {
        writer.write_all(wire.as_bytes()).await?;
        writer.flush().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out writing the prompt answer into the shell"))??;
    Ok(())
}
