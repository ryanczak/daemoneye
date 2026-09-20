//! Client for a shell host's socket: run a marker-wrapped command over the
//! wire frames defined in `proto.rs` and parse the real outcome from the
//! accumulated chunk stream.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::shell::proto::{ShellRequest, ShellResponse, ShellSignal, encode};
use crate::shell::pty::{CommandOutcome, Nonce, parse_outcome, wrap_command};
#[cfg(test)]
use crate::shell::test_begin_marker;
#[cfg(test)]
use crate::shell::test_end_marker;

/// How long one prompt answer may take to reach the shell. Bounded well below
/// any plausible command timeout so a wedged host cannot wedge the caller.
const PROMPT_ROUND_TIMEOUT: Duration = Duration::from_secs(10);

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
/// - [`PromptReply::GiveUp(reason)`] — un-park the shell with a single `\n`
///   (a shell at a sudo prompt eats the next command's bytes; Probe B proved
///   the refusal completes sudo cleanly), and return `Err(reason)`.
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
    // The accumulated output a `Send`/`GiveUp` decision was already acted on
    // for. A prompt still on screen in the next chunk does not re-trigger the
    // same callback answer.
    let mut answered_at: Option<usize> = None;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut line: Vec<u8> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        line.clear();
        let read = tokio::time::timeout_at(deadline, reader.read_until(b'\n', &mut line)).await;
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                return Err(e).with_context(|| format!("reading frame from {}", sock.display()));
            }
            Err(_) => {
                anyhow::bail!("timed out after {timeout:?} waiting for command output: {cmd:?}");
            }
        };
        if n == 0 {
            anyhow::bail!(
                "shell socket at {} closed while running {cmd:?}",
                sock.display()
            );
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
        match frame {
            ShellResponse::Chunk { bytes } => {
                // Chunks split arbitrarily (a PTY read can land mid-sequence),
                // so never decode a chunk on its own — accumulate bytes and
                // hand the whole buffer to `parse_outcome`.
                buf.extend_from_slice(&bytes);
                if let Some(outcome) = parse_outcome(&buf, nonce) {
                    return Ok(outcome);
                }
                if last_answered_at(&answered_at) != Some(buf.len()) {
                    let seen = String::from_utf8_lossy(&buf).into_owned();
                    match on_chunk(&seen).await {
                        PromptReply::Wait => {}
                        PromptReply::Send(answer) => {
                            write_answer(writer, &answer).await?;
                            answered_at = Some(buf.len());
                        }
                        PromptReply::GiveUp(reason) => {
                            // Un-park the shell: sudo at an empty newline fails
                            // fast with "no password was provided" and the end
                            // marker still arrives. Never the credential, never
                            // a signal.
                            write_answer(writer, b"\n").await?;
                            anyhow::bail!("{reason}");
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

/// The buffer length at which the last callback decision was made, or `None`.
/// Asking the callback again on every chunk is safe — the answer to a new
/// question is a new decision — but a chunk that adds *no* bytes would re-ask
/// exactly the question the previous chunk was answered with, which is why the
/// not-yet-answered state is `None` and a decision is recorded at the length
/// it was made at.
fn last_answered_at(answered_at: &Option<usize>) -> Option<usize> {
    *answered_at
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
mod tests {
    use super::*;
    use crate::shell::PtyBackend;
    use crate::shell::host;
    use crate::shell::proto::{ShellRequest, encode};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixStream;

    /// A real PTY-backed shell host on a temp socket — exactly the shape F1's
    /// probe used. Avoids `launch::run_host` because that owns the per-id
    /// instance-lock bookkeeping this client does not need.
    struct Host {
        path: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// Hold the home guard across the PTY spawn (bug-05c-1): no
    /// `PtyShell::spawn` may overlap another thread's `set_var`.
    async fn spawn_host() -> Host {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        let cast = dir.path().join("e2e.cast");
        let backend =
            PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0).expect("bash available");
        let path = dir.path().join("shell.sock");
        let listener = host::bind(&path).await.expect("bind the shell socket");
        tokio::spawn(host::serve(listener, Arc::clone(&backend)));
        Host { path, _dir: dir }
    }

    #[tokio::test]
    async fn client_run_command_returns_the_real_exit_code() {
        let host = spawn_host().await;
        let outcome = run_command(
            &host.path,
            "printf 'CLIENT_OK\\n'; sh -c 'exit 5'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the command completes inside the timeout");
        assert_eq!(outcome.exit_code, 5, "real exit code must surface");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("CLIENT_OK"),
            "output must contain the command's bytes: {text:?}"
        );
    }

    #[tokio::test]
    async fn client_run_command_output_excludes_the_markers() {
        let host = spawn_host().await;
        let outcome = run_command(
            &host.path,
            "printf 'MARK\\n'; sh -c 'exit 3'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the command completes inside the timeout");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            !text.contains("DE_BEG"),
            "the begin marker must be stripped: {text:?}"
        );
        assert!(
            !text.contains("DE_END"),
            "the end marker must be stripped: {text:?}"
        );
        assert!(
            !text.contains('\u{1f}'),
            "marker framing must be stripped: {text:?}"
        );
        assert_eq!(outcome.exit_code, 3);
    }

    #[tokio::test]
    async fn client_run_command_survives_a_chunk_split_mid_marker() {
        let host = spawn_host().await;
        // 20 KB of output forces the PTY reads (≤4 KiB each) to split the
        // stream — and the end marker — across several chunks.
        let outcome = run_command(
            &host.path,
            "head -c 20000 /dev/zero | tr '\\0' 'x'; printf 'TAIL\\n'; sh -c 'exit 2'",
            "bash",
            Duration::from_secs(20),
        )
        .await
        .expect("a large command resolves through accumulation");
        assert_eq!(outcome.exit_code, 2, "exit code survives chunk splits");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("TAIL"),
            "the distinct tail must survive: ...{text:?}"
        );
        let x_count = text.chars().filter(|c| *c == 'x').count();
        assert!(
            x_count >= 20000,
            "the streamed bytes must accumulate, not be dropped: {x_count} x's"
        );
    }

    #[tokio::test]
    async fn client_run_command_times_out_on_a_silent_command() {
        let host = spawn_host().await;
        let started = std::time::Instant::now();
        let result = run_command(&host.path, "sleep 30", "bash", Duration::from_millis(500)).await;
        let elapsed = started.elapsed();
        assert!(result.is_err(), "a silent command must Err");
        assert!(
            elapsed < Duration::from_secs(8),
            "the client must fail at the deadline, not wait out the command: elapsed={elapsed:?}"
        );
        let msg = format!("{result:?}");
        assert!(
            msg.contains("timed out") && msg.contains("sleep 30"),
            "the error must name the timeout and the command: {msg}"
        );
    }

    #[tokio::test]
    async fn client_run_command_ignores_interleaved_non_chunk_frames() {
        let host = spawn_host().await;
        // Fire a Status request on a peer connection while the command runs on
        // the command connection: the host answers Status on the peer socket
        // while Chunk frames stream to the command socket, and neither
        // connection's traffic may corrupt the other. The command connection
        // itself also receives — and must ignore — the two `Ok`
        // acknowledgements for its Subscribe and Input frames before any
        // chunk arrives (bug-05a-1 shape from the client side).
        let stream = UnixStream::connect(&host.path)
            .await
            .expect("second connection");
        let (peer_reader, mut peer_writer) = stream.into_split();
        let mut peer_reader = BufReader::new(peer_reader);
        let status_task = tokio::spawn(async move {
            let wire = encode(ShellRequest::Status).expect("encode Status");
            peer_writer
                .write_all(wire.as_bytes())
                .await
                .expect("send Status");
            let mut buf = Vec::new();
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                peer_reader.read_until(b'\n', &mut buf),
            )
            .await;
        });

        let outcome = run_command(
            &host.path,
            "printf 'INTERLEAVE\\n'; sh -c 'exit 9'",
            "bash",
            Duration::from_secs(15),
        )
        .await
        .expect("the command resolves despite concurrent Status traffic");
        assert_eq!(outcome.exit_code, 9);
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("INTERLEAVE"),
            "the command's own bytes must be intact: {text:?}"
        );
        let _ = status_task.await;
    }

    #[tokio::test]
    async fn client_run_command_errors_when_the_socket_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("missing.sock");
        let started = std::time::Instant::now();
        let result = run_command(&sock, "echo hi", "bash", Duration::from_secs(10)).await;
        let elapsed = started.elapsed();
        assert!(result.is_err(), "an absent socket must Err, not hang");
        assert!(
            elapsed < Duration::from_secs(3),
            "failure must be prompt, not wait out the deadline: elapsed={elapsed:?}"
        );
    }

    #[tokio::test]
    async fn client_spawn_command_returns_before_the_command_finishes() {
        let host = spawn_host().await;
        let mut handle = spawn_command(
            &host.path,
            "sleep 1; printf 'LATE\\n'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the handshake completes and the command starts");

        let pending = handle
            .outcome
            .try_recv()
            .expect_err("a still-running command must not have resolved yet");
        assert!(
            matches!(pending, tokio::sync::oneshot::error::TryRecvError::Empty),
            "the call must return while the command still runs: {pending:?}"
        );

        let outcome = handle
            .outcome
            .await
            .expect("the reader task stays alive to deliver")
            .expect("the command finishes inside the timeout");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("LATE"),
            "the late bytes must still arrive: {text:?}"
        );
    }

    #[tokio::test]
    async fn client_spawn_command_delivers_the_real_exit_code() {
        let host = spawn_host().await;
        let handle = spawn_command(
            &host.path,
            "sh -c 'exit 7'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the command starts");
        let outcome = handle
            .outcome
            .await
            .expect("the reader task delivers")
            .expect("exit 7 is a normal completion");
        assert_eq!(
            outcome.exit_code, 7,
            "the real exit code must survive the detached round-trip, not 0 and not 1"
        );
    }

    #[tokio::test]
    async fn client_spawn_command_delivers_the_output_between_markers() {
        let host = spawn_host().await;
        let handle = spawn_command(
            &host.path,
            "printf 'BG_OK\\n'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the command starts");
        let outcome = handle
            .outcome
            .await
            .expect("the reader task delivers")
            .expect("printf cannot fail");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("BG_OK"),
            "the command's own bytes must arrive: {text:?}"
        );
        assert!(
            !text.contains("DE_END") && !text.contains("DE_BEG"),
            "the marker literal must be stripped from the delivered output: {text:?}"
        );
    }

    #[tokio::test]
    async fn client_spawn_command_errors_when_the_shell_socket_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("missing.sock");
        let result = spawn_command(&sock, "echo hi", "bash", Duration::from_secs(10)).await;
        let err = result.expect_err("a start failure must be an Err from the call itself");
        assert!(
            err.to_string().contains("missing.sock"),
            "the error must name the socket it could not reach: {err:#}"
        );
    }

    #[tokio::test]
    async fn client_spawn_command_timeout_resolves_the_receiver_with_an_error() {
        let host = spawn_host().await;
        let handle = spawn_command(&host.path, "sleep 30", "bash", Duration::from_millis(500))
            .await
            .expect("a silent command still starts");
        let result = handle
            .outcome
            .await
            .expect("the reader task delivers its outcome, including failure")
            .expect_err("a command outliving the timeout must resolve to Err");
        let msg = format!("{result:#}");
        assert!(
            msg.contains("timed out") && msg.contains("sleep 30"),
            "the detached timeout must name the timeout and the command like run_command's: {msg}"
        );
    }

    #[tokio::test]
    async fn client_run_command_and_spawn_command_agree_on_one_command() {
        let host = spawn_host().await;
        let cmd = "printf 'PARITY\\n'; sh -c 'exit 3'";

        let awaited = run_command(&host.path, cmd, "bash", Duration::from_secs(10))
            .await
            .expect("the awaited path completes");
        let detached = spawn_command(&host.path, cmd, "bash", Duration::from_secs(10))
            .await
            .expect("the detached path starts")
            .outcome
            .await
            .expect("the reader task delivers")
            .expect("the detached path completes");

        assert_eq!(
            awaited.exit_code, detached.exit_code,
            "both paths must report the same real exit code"
        );
        assert_eq!(
            String::from_utf8_lossy(&awaited.output),
            String::from_utf8_lossy(&detached.output),
            "both paths must deliver the same output bytes: awaited={:?} detached={:?}",
            String::from_utf8_lossy(&awaited.output),
            String::from_utf8_lossy(&detached.output),
        );
    }

    /// A fake `ShellBackend` over a real socket, on the `shell::host` tests'
    /// pattern — no PTY needed to exercise the client's one-shot round-trips.
    mod oneshot {
        use super::*;
        use crate::shell::host::{ShellBackend, bind, serve};
        use crate::shell::proto::ShellState;
        use std::sync::Mutex;

        #[derive(Debug, Clone, PartialEq, Eq)]
        enum Call {
            Signal(ShellSignal),
            Input(Vec<u8>),
            Status,
        }

        struct SignalBackend {
            calls: Mutex<Vec<Call>>,
            status: ShellResponse,
            fail_input: std::sync::atomic::AtomicBool,
        }

        impl SignalBackend {
            fn new(status: ShellResponse) -> Arc<Self> {
                Arc::new(Self {
                    calls: Mutex::new(Vec::new()),
                    status,
                    fail_input: std::sync::atomic::AtomicBool::new(false),
                })
            }
            fn calls(&self) -> Vec<Call> {
                self.calls.lock().unwrap().clone()
            }
            fn fail_input(&self, yes: bool) {
                self.fail_input
                    .store(yes, std::sync::atomic::Ordering::SeqCst);
            }
        }

        #[async_trait::async_trait]
        impl ShellBackend for SignalBackend {
            async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
                self.calls.lock().unwrap().push(Call::Input(bytes.to_vec()));
                if self.fail_input.load(std::sync::atomic::Ordering::SeqCst) {
                    Err(anyhow::anyhow!("input rejected"))
                } else {
                    Ok(())
                }
            }
            async fn resize(&self, _rows: u16, _cols: u16) -> anyhow::Result<()> {
                Ok(())
            }
            async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()> {
                self.calls.lock().unwrap().push(Call::Signal(sig));
                Ok(())
            }
            async fn status(&self) -> ShellResponse {
                self.calls.lock().unwrap().push(Call::Status);
                self.status.clone()
            }
            fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>> {
                tokio::sync::broadcast::channel(1).0.subscribe()
            }
        }

        struct Server {
            backend: Arc<SignalBackend>,
            path: PathBuf,
            _dir: tempfile::TempDir,
        }

        async fn start_server(status: ShellResponse) -> Server {
            let backend = SignalBackend::new(status);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("one-shot.sock");
            let listener = bind(&path).await.unwrap();
            let serve_backend = Arc::clone(&backend);
            tokio::spawn(async move { serve(listener, serve_backend).await });
            Server {
                backend,
                path,
                _dir: dir,
            }
        }

        #[tokio::test]
        async fn client_send_signal_round_trips() {
            let server = start_server(ShellResponse::Ok).await;
            send_signal(&server.path, ShellSignal::Stop, Duration::from_secs(5))
                .await
                .expect("the host acknowledges the signal");
            send_signal(&server.path, ShellSignal::Cont, Duration::from_secs(5))
                .await
                .expect("the host acknowledges the second signal");
            assert_eq!(
                server.backend.calls(),
                vec![
                    Call::Signal(ShellSignal::Stop),
                    Call::Signal(ShellSignal::Cont)
                ],
                "both signals must reach the backend in order"
            );
        }

        #[tokio::test]
        async fn client_send_signal_carries_the_host_error_back() {
            // A host that refuses the signal: the Error frame must surface as
            // an `Err` carrying the host's message.
            struct Refusing;
            #[async_trait::async_trait]
            impl ShellBackend for Refusing {
                async fn input(&self, _bytes: &[u8]) -> anyhow::Result<()> {
                    Ok(())
                }
                async fn resize(&self, _rows: u16, _cols: u16) -> anyhow::Result<()> {
                    Ok(())
                }
                async fn signal(&self, _sig: ShellSignal) -> anyhow::Result<()> {
                    Err(anyhow::anyhow!("no such group"))
                }
                async fn status(&self) -> ShellResponse {
                    ShellResponse::Ok
                }
                fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>> {
                    tokio::sync::broadcast::channel(1).0.subscribe()
                }
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("refusing.sock");
            let listener = bind(&path).await.unwrap();
            tokio::spawn(async move { serve(listener, Arc::new(Refusing)).await });
            let err = send_signal(&path, ShellSignal::Int, Duration::from_secs(5))
                .await
                .expect_err("a refusing host must surface an Err");
            assert!(
                err.to_string().contains("no such group"),
                "the error must carry the host's message: {err:#}"
            );
        }

        #[tokio::test]
        async fn client_status_reports_state() {
            let expected = ShellResponse::Status {
                state: ShellState::Paused,
                rows: 31,
                cols: 100,
                pid: 4711,
            };
            let server = start_server(expected.clone()).await;
            let answered = status(&server.path, Duration::from_secs(5))
                .await
                .expect("the one-shot Status round-trip answers");
            assert_eq!(answered, expected, "the frame must survive");
            match answered {
                ShellResponse::Status {
                    state: ShellState::Paused,
                    rows,
                    cols,
                    pid,
                } => {
                    assert_eq!(rows, 31, "rows round-trip");
                    assert_eq!(cols, 100, "cols round-trip");
                    assert_eq!(pid, 4711, "pid round-trip");
                }
                other => panic!("expected a Status frame, got {other:?}"),
            }
            assert_eq!(
                server.backend.calls(),
                vec![Call::Status],
                "exactly one Status dispatch"
            );
        }

        #[tokio::test]
        async fn send_input_frame_is_acked_by_the_host() {
            let server = start_server(ShellResponse::Ok).await;
            send_input(&server.path, b"hello\n", Duration::from_secs(5))
                .await
                .expect("the host acknowledges the input write");
            send_input(&server.path, b"", Duration::from_secs(5))
                .await
                .expect("an empty write is still acknowledged");
        }

        #[tokio::test]
        async fn send_input_frame_reports_a_host_error() {
            let server = start_server(ShellResponse::Ok).await;
            server.backend.fail_input(true);
            let err = send_input(&server.path, b"hello\n", Duration::from_secs(5)).await;
            assert!(
                err.is_err(),
                "an Error frame surfaces as an Err, not an ack: {err:?}"
            );
            assert!(
                format!("{:#}", err.unwrap_err()).contains("rejected"),
                "the host's message reaches the caller"
            );
        }
    }

    /// A streaming fake host: a real socket (the `oneshot` module's pattern)
    /// whose backend plays a scripted sequence of chunks onto its broadcast
    /// channel while the client's read loop runs, and records every byte the
    /// client writes on the same connection. This is what lets a test put a
    /// sudo-shaped prompt in front of `run_command_with_prompts` and watch
    /// the answer arrive as an `Input` frame.
    mod scripted {
        use super::*;
        use crate::shell::host::{ShellBackend, bind, serve};
        use std::sync::Mutex;

        struct ScriptedBackend {
            /// Every write the client made, byte-exact, in order.
            writes: Mutex<Vec<Vec<u8>>>,
            /// The broadcast every connection's subscriber reads from.
            tx: tokio::sync::broadcast::Sender<Vec<u8>>,
        }

        impl ScriptedBackend {
            fn new() -> (Arc<Self>, tokio::sync::broadcast::Sender<Vec<u8>>) {
                let (tx, _) = tokio::sync::broadcast::channel(64);
                let backend = Arc::new(Self {
                    writes: Mutex::new(Vec::new()),
                    tx: tx.clone(),
                });
                (backend, tx)
            }

            fn writes(&self) -> Vec<Vec<u8>> {
                self.writes.lock().unwrap().clone()
            }
        }

        #[async_trait::async_trait]
        impl ShellBackend for ScriptedBackend {
            async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
                self.writes.lock().unwrap().push(bytes.to_vec());
                Ok(())
            }
            async fn resize(&self, _rows: u16, _cols: u16) -> anyhow::Result<()> {
                Ok(())
            }
            async fn signal(&self, _sig: ShellSignal) -> anyhow::Result<()> {
                Ok(())
            }
            async fn status(&self) -> ShellResponse {
                ShellResponse::Ok
            }
            fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>> {
                self.tx.subscribe()
            }
        }

        struct ScriptedHost {
            backend: Arc<ScriptedBackend>,
            path: PathBuf,
            _dir: tempfile::TempDir,
        }

        /// Wait until the client's wrapped command arrives in the recorded
        /// writes, so the scripted chunks are published strictly after the
        /// client subscribed to the broadcast.
        async fn await_input(server: &ScriptedHost) -> Vec<u8> {
            for _ in 0..500 {
                {
                    let writes = server.backend.writes();
                    if let Some(bytes) = writes.first() {
                        return bytes.clone();
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("the client never wrote its command to the fake host");
        }

        async fn spawn_scripted() -> ScriptedHost {
            let (backend, _) = ScriptedBackend::new();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("scripted.sock");
            let listener = bind(&path).await.unwrap();
            let serve_backend = Arc::clone(&backend);
            tokio::spawn(async move { serve(listener, serve_backend).await });
            ScriptedHost {
                backend,
                path,
                _dir: dir,
            }
        }

        /// The marker pair the client's own wrapped command will resolve to,
        /// as a real bash session emits them: the shell executes the wrapped
        /// text's `printf`s, so the raw `\x1f`-framed markers reach the
        /// stream. The nonce is recovered from the recorded command text (the
        /// quoted `DE_''BEG` spelling), keeping it test-private.
        fn markers_for_sent_command(wrapped: &[u8]) -> (Vec<u8>, Vec<u8>) {
            let text = String::from_utf8_lossy(wrapped);
            let nonce = text
                .split("DE_''BEG ")
                .nth(1)
                .and_then(|rest| rest.split('\\').next())
                .expect("the wrapped command carries its begin marker");
            let nonce = Nonce(nonce.to_string());
            (test_begin_marker(&nonce), test_end_marker(&nonce, 0))
        }

        #[tokio::test]
        async fn run_command_with_prompts_answers_a_prompt() {
            let host = spawn_scripted().await;
            let tx = host.backend.tx.clone();
            let path = host.path.clone();
            let client = tokio::spawn(async move {
                run_command_with_prompts(
                    &path,
                    "sudo true",
                    "bash",
                    Duration::from_secs(10),
                    |output: &str| {
                        if output.contains("[de-sudo-prompt-0]") {
                            std::future::ready(PromptReply::Send(b"let-me-through\n".to_vec()))
                        } else {
                            std::future::ready(PromptReply::Wait)
                        }
                    },
                )
                .await
            });
            let sent = await_input(&host).await;
            let (begin, end) = markers_for_sent_command(&sent);
            let publisher = tokio::spawn(async move {
                tx.send(begin).unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                tx.send(b"\r\n[de-sudo-prompt-0] ".to_vec()).unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                tx.send(end).unwrap();
            });
            let outcome = client
                .await
                .expect("the client task joins")
                .expect("the answered prompt completes the command");
            publisher.await.expect("the script task finishes");

            assert_eq!(outcome.exit_code, 0, "the scripted exit code surfaces");
            let writes = host.backend.writes();
            assert_eq!(
                writes.len(),
                2,
                "exactly the command plus the answer crosses the socket: {writes:?}"
            );
            assert_eq!(
                String::from_utf8_lossy(&writes[1]),
                "let-me-through\n",
                "the answer reaches the shell byte-exact: {:?}",
                writes[1]
            );
        }

        #[tokio::test]
        async fn run_command_with_prompts_gives_up_without_an_answer() {
            let host = spawn_scripted().await;
            let tx = host.backend.tx.clone();
            let path = host.path.clone();
            let client = tokio::spawn(async move {
                run_command_with_prompts(
                    &path,
                    "sudo true",
                    "bash",
                    Duration::from_secs(10),
                    |output: &str| {
                        if output.contains("[de-sudo-prompt-1]") {
                            std::future::ready(PromptReply::GiveUp(
                                "cannot answer here".to_string(),
                            ))
                        } else {
                            std::future::ready(PromptReply::Wait)
                        }
                    },
                )
                .await
            });
            let sent = await_input(&host).await;
            let (begin, _end) = markers_for_sent_command(&sent);
            tx.send(begin).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(b"\r\n[de-sudo-prompt-1] ".to_vec()).unwrap();
            let err = client
                .await
                .expect("the client task joins")
                .expect_err("a give-up abandons the command with its reason");

            assert!(
                err.to_string().contains("cannot answer here"),
                "the give-up reason is the error: {err:#}"
            );
            // The write races the host's recording task: poll until the
            // un-parking newline lands (bounded, like `await_input`). Mutation
            // B removes that write, so this test must then fail on the
            // missing byte rather than pass with one write fewer.
            let mut writes = Vec::new();
            for _ in 0..500 {
                writes = host.backend.writes();
                if writes.len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_ne!(
                writes.len(),
                1,
                "mutation B guard: the give-up path must still un-park the shell \
                 with its own write — exactly one write here means the newline \
                 was dropped"
            );
            assert_eq!(
                writes.len(),
                2,
                "the command plus exactly one un-parking write: {writes:?}"
            );
            assert_eq!(
                writes[1],
                b"\n".to_vec(),
                "the un-parking write is a single newline, never the credential"
            );
        }

        #[tokio::test]
        async fn run_command_is_unchanged_without_prompts() {
            let host = spawn_scripted().await;
            let tx = host.backend.tx.clone();
            let path = host.path.clone();
            let client = tokio::spawn(async move {
                run_command(&path, "echo plain", "bash", Duration::from_secs(10)).await
            });
            let sent = await_input(&host).await;
            let (begin, end) = markers_for_sent_command(&sent);
            tx.send(begin).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(b"plain output\r\n".to_vec()).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(end).unwrap();

            let awaited = client
                .await
                .expect("the client task joins")
                .expect("the plain path completes");

            let writes = host.backend.writes();
            assert_eq!(
                writes.len(),
                1,
                "the always-Wait path never writes beyond the command: {writes:?}"
            );
            assert_eq!(
                awaited.exit_code, 0,
                "the scripted exit code surfaces through the plain path"
            );
            let out = String::from_utf8_lossy(&awaited.output);
            assert!(
                out.contains("plain output") && !out.contains("DE_"),
                "the output is the command's own bytes without marker framing: {out:?}"
            );
        }

        /// Task 5's "identically through both entry points" half: the same
        /// plain command through `run_command_with_prompts` with an always-
        /// `Wait` callback produces the same outcome shape — the exit code,
        /// the exact output, and no write beyond the command itself.
        #[tokio::test]
        async fn run_command_with_prompts_without_a_prompt_matches_run_command() {
            let host = spawn_scripted().await;
            let tx = host.backend.tx.clone();
            let path = host.path.clone();
            let client = tokio::spawn(async move {
                run_command_with_prompts(
                    &path,
                    "echo plain",
                    "bash",
                    Duration::from_secs(10),
                    |_: &str| async { PromptReply::Wait },
                )
                .await
            });
            let sent = await_input(&host).await;
            let (begin, end) = markers_for_sent_command(&sent);
            tx.send(begin).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(b"plain output\r\n".to_vec()).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(end).unwrap();

            let awaited = client
                .await
                .expect("the client task joins")
                .expect("the always-Wait path completes like run_command");

            let writes = host.backend.writes();
            assert_eq!(
                writes.len(),
                1,
                "the always-Wait callback never writes beyond the command: {writes:?}"
            );
            assert_eq!(
                awaited.exit_code, 0,
                "the scripted exit code surfaces through the always-Wait path"
            );
            let out = String::from_utf8_lossy(&awaited.output);
            assert_eq!(
                out, "plain output",
                "the output is the command's own bytes, identical to run_command's shape"
            );
        }
    }
}
