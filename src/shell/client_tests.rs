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
    let backend = PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0).expect("bash available");
    let path = dir.path().join("shell.sock");
    let listener = host::bind(&path).await.expect("bind the shell socket");
    tokio::spawn(host::serve(listener, Arc::clone(&backend)));
    Host { path, _dir: dir }
}

/// The same host shape with `program`/`args` as the shell's program — the
/// seam the exit tests use to put a program that dies immediately behind
/// an otherwise-live socket.
async fn spawn_host_running(program: &str, args: &[String]) -> Host {
    let _home = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    let cast = dir.path().join("e2e.cast");
    let backend = PtyBackend::spawn(program, args, 24, 80, 100, &cast, 0)
        .expect("the shell program is spawnable");
    let path = dir.path().join("shell.sock");
    let listener = host::bind(&path).await.expect("bind the shell socket");
    tokio::spawn(host::serve(listener, Arc::clone(&backend)));
    Host { path, _dir: dir }
}

/// A host whose program has already exited, leaving the socket bound and
/// the host process alive but the PTY child gone.
async fn spawn_host_exited() -> Host {
    spawn_host_running("sh", &["-c".to_string(), "exit 0".to_string()]).await
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
        Screen { annotated: bool, scrollback: u16 },
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
        async fn screen_text(
            &self,
            annotated: bool,
            scrollback: u16,
        ) -> anyhow::Result<ShellResponse> {
            self.calls.lock().unwrap().push(Call::Screen {
                annotated,
                scrollback,
            });
            Ok(self.status.clone())
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
    async fn close_falls_back_to_term_on_a_host_without_close() {
        // SignalBackend has no `close` override, so it uses the trait's
        // default refusal: the frame must fall back to a TERM signal.
        let server = start_server(ShellResponse::Ok).await;
        close_or_term(&server.path, Duration::from_secs(5))
            .await
            .expect("the fallback must close");
        assert_eq!(
            server.backend.calls(),
            vec![Call::Signal(ShellSignal::Term)],
            "the refusal must fall back to a TERM signal"
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
            raw_mode: None,
            alt_screen: None,
            foreground: None,
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
                ..
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
    async fn client_screen_round_trip() {
        let expected = ShellResponse::ScreenText {
            text: "line1\n[SEL: picked]\n$ ".into(),
            rows: 24,
            cols: 80,
            cursor: (12, 34),
            alt_screen: true,
            bracketed_paste: false,
            application_cursor: false,
        };
        let server = start_server(expected.clone()).await;
        let answered = screen(&server.path, true, 40, Duration::from_secs(5))
            .await
            .expect("the one-shot Screen round-trip answers");
        assert_eq!(answered, expected, "the frame must survive");
        assert_eq!(
            server.backend.calls(),
            vec![Call::Screen {
                annotated: true,
                scrollback: 40
            }],
            "the request's two fields must reach the backend"
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
    async fn run_command_with_prompts_offers_only_output_after_the_last_answer() {
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
            tx.send(b"\r\nROOT_OK\r\n".to_vec()).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(end).unwrap();
        });
        let outcome = client
            .await
            .expect("the client task joins")
            .expect("the command completes");
        let _ = publisher.await;
        assert_eq!(outcome.exit_code, 0, "the scripted outcome is exit 0");

        let writes = host.backend.writes();
        assert_eq!(
            writes.len(),
            2,
            "exactly the command plus one answer crosses the socket; the \
                 ordinary output after the answer must not re-offer the \
                 already-answered prompt: {writes:?}"
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
                        std::future::ready(PromptReply::GiveUp("cannot answer here".to_string()))
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
        assert!(
            err.chain()
                .any(|c| c.downcast_ref::<PromptGivenUp>().is_some()),
            "the give-up is the typed PromptGivenUp, so callers and the \
                 model-facing text can name it: {err:#}"
        );
        // The write races the host's recording task: poll until the
        // cancel write lands (bounded, like `await_input`). Mutation B
        // removes that write, so this test must then fail on the missing
        // byte rather than pass with one write fewer.
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
            "mutation B guard: the give-up path must still cancel the \
                 prompt with its own write — exactly one write here means the \
                 Ctrl-C byte was dropped"
        );
        assert_eq!(
            writes.len(),
            2,
            "the command plus exactly one cancel write: {writes:?}"
        );
        assert_eq!(
            writes[1],
            b"\x03".to_vec(),
            "the cancel write is the single Ctrl-C byte, never the \
                 credential, never a newline a real sudo would treat as a \
                 wrong password"
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

// ── shell-exit detection (M21 phase-08b) ─────────────────────────────────

#[tokio::test]
async fn client_run_command_fails_fast_when_the_shell_program_has_exited() {
    let host = spawn_host_exited().await;
    let started = std::time::Instant::now();
    let result = run_command(&host.path, "echo hi", "sh", Duration::from_secs(20)).await;
    let elapsed = started.elapsed();
    let err = result.expect_err("a dead shell must fail the command");
    assert!(
        elapsed < Duration::from_secs(5),
        "the failure must land in seconds, not at the 20 s timeout: {elapsed:?}"
    );
    assert!(
        err.chain()
            .any(|c| c.downcast_ref::<ShellExited>().is_some()),
        "the chain must carry the typed error: {err:#?}"
    );
    let text = format!("{err:#}");
    assert!(text.contains("exited"), "the error says exited: {text}");
}

#[tokio::test]
async fn client_run_command_fails_fast_when_the_command_exits_the_shell() {
    let host = spawn_host().await;
    let started = std::time::Instant::now();
    let result = run_command(&host.path, "exit 7", "bash", Duration::from_secs(20)).await;
    let elapsed = started.elapsed();
    let err = result.expect_err("exiting the shell must fail the command");
    assert!(
        elapsed < Duration::from_secs(5),
        "the failure must land in seconds, not at the 20 s timeout: {elapsed:?}"
    );
    let text = format!("{err:#}");
    assert!(text.contains("exited"), "the error says exited: {text}");
}

#[tokio::test]
async fn client_run_command_on_a_live_shell_is_not_cut_short_by_the_exit_check() {
    let host = spawn_host().await;
    let outcome = run_command(
        &host.path,
        "sleep 2; echo LIVE_OK",
        "bash",
        Duration::from_secs(10),
    )
    .await
    .expect("the live shell completes the command");
    let text = String::from_utf8_lossy(&outcome.output);
    assert!(
        text.contains("LIVE_OK"),
        "the live command's output must arrive intact: {text:?}"
    );
}

#[tokio::test]
async fn client_run_command_pause_does_not_count_against_the_deadline() {
    let host = spawn_host().await;
    let sock = host.path.clone();
    let task = tokio::spawn(async move {
        run_command(
            &sock,
            "sleep 1; echo PAUSED_OK",
            "bash",
            Duration::from_secs(2),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_signal(&host.path, ShellSignal::Stop, Duration::from_secs(5))
        .await
        .expect("the shell acknowledges the stop");
    // Sleep past the 2 s limit entirely while paused: none of it may
    // count against the command's deadline.
    tokio::time::sleep(Duration::from_secs(3)).await;
    send_signal(&host.path, ShellSignal::Cont, Duration::from_secs(5))
        .await
        .expect("the shell acknowledges the continue");
    let outcome = task
        .await
        .expect("the command task joins")
        .expect("paused time is not the command's run time");
    let text = String::from_utf8_lossy(&outcome.output);
    assert!(
        text.contains("PAUSED_OK"),
        "the command must complete after the resume, with its output: {text:?}"
    );
}

#[tokio::test]
async fn client_run_command_still_times_out_when_not_paused() {
    let host = spawn_host().await;
    let started = std::time::Instant::now();
    let result = run_command(&host.path, "sleep 5", "bash", Duration::from_secs(1)).await;
    let elapsed = started.elapsed();
    let err = result.expect_err("an unpaused command past its deadline must Err");
    assert!(
        elapsed < Duration::from_secs(3),
        "the extension must apply only while paused: elapsed={elapsed:?}"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("timed out"),
        "the error must name the timeout: {msg}"
    );
}

#[tokio::test]
async fn client_spawn_command_fails_fast_when_the_shell_program_has_exited() {
    let host = spawn_host_exited().await;
    let started = std::time::Instant::now();
    let handle = spawn_command(&host.path, "echo hi", "sh", Duration::from_secs(20))
        .await
        .expect("the spawn starts");
    let outcome = handle.outcome.await.expect("the reader task joins");
    let elapsed = started.elapsed();
    let err = outcome.expect_err("a dead shell must fail the spawned command");
    assert!(
        elapsed < Duration::from_secs(5),
        "the spawned command must fail in seconds, not at the 20 s timeout: {elapsed:?}"
    );
    let text = format!("{err:#}");
    assert!(text.contains("exited"), "the error says exited: {text}");
}

/// The live check (Task 6): a real `ssh` whose connection is refused in
/// BatchMode — the shape 08c's ghost shells take — fails in seconds with
/// "exited", not at the command's timeout. Run by hand:
/// `cargo test --lib a_batchmode_ssh_shell_that_cannot_connect_fails_fast_on_pinky -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn a_batchmode_ssh_shell_that_cannot_connect_fails_fast_on_pinky() {
    let host = {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        let known = dir.path().join("known_hosts");
        std::fs::write(&known, b"").unwrap();
        let cast = dir.path().join("e2e.cast");
        let args: Vec<String> = [
            "-tt",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            &format!("UserKnownHostsFile={}", known.display()),
            "-o",
            "StrictHostKeyChecking=ask",
            "pinky",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let path = dir.path().join("shell.sock");
        let listener = host::bind(&path).await.expect("bind the shell socket");
        let backend =
            PtyBackend::spawn("ssh", &args, 24, 80, 100, &cast, 0).expect("ssh is on PATH");
        tokio::spawn(host::serve(listener, Arc::clone(&backend)));
        Host { path, _dir: dir }
    };
    let started = std::time::Instant::now();
    let result = run_command(&host.path, "echo hi", "sh", Duration::from_secs(20)).await;
    let elapsed = started.elapsed();
    println!("elapsed={elapsed:?}");
    let err = result.expect_err("a refused ssh must fail the command");
    let text = format!("{err:#}");
    assert!(
        text.contains("exited"),
        "the failure names the exit, not a timeout: {text}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the failure must land in seconds, not at the 20 s timeout: {elapsed:?}"
    );
}
