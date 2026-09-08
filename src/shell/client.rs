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

/// Run `cmd` in the shell hosted at `sock` and return its real outcome.
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
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

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
                if let Some(outcome) = parse_outcome(&buf, &nonce) {
                    return Ok(outcome);
                }
            }
            _ => {
                // `Ok`, `Status`, `Exited` and `Error` may interleave with the
                // command's own output and are not part of it.
            }
        }
    }
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
        let backend = PtyBackend::spawn("bash", 24, 80, 100, &cast, 0).expect("bash available");
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
            Status,
        }

        struct SignalBackend {
            calls: Mutex<Vec<Call>>,
            status: ShellResponse,
        }

        impl SignalBackend {
            fn new(status: ShellResponse) -> Arc<Self> {
                Arc::new(Self {
                    calls: Mutex::new(Vec::new()),
                    status,
                })
            }
            fn calls(&self) -> Vec<Call> {
                self.calls.lock().unwrap().clone()
            }
        }

        #[async_trait::async_trait]
        impl ShellBackend for SignalBackend {
            async fn input(&self, _bytes: &[u8]) -> anyhow::Result<()> {
                Ok(())
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
    }
}
