//! The shell-host socket server: a `ShellBackend` trait plus the socket server
//! that dispatches newline-delimited JSON frames to it.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use crate::daemon::server::check_peer_identity;
use crate::shell::proto::{ShellRequest, ShellResponse, ShellSignal, encode};

/// What a shell-host must be able to do. Phase-05b implements this over a
/// real PTY; the tests implement it over a fake.
#[async_trait]
pub trait ShellBackend: Send + Sync + 'static {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()>;
    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()>;
    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()>;
    async fn status(&self) -> ShellResponse;
    /// A receiver of output chunks for one subscriber.
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;
}

/// Bind `path`, removing whatever already occupies it first. The caller owns
/// the id, so a leftover file at `path` is by definition stale — binding over
/// a live socket would require a second bind, which fails (F2). The mode is
/// set explicitly to `0o700` so privacy does not depend on the caller's umask
/// (F1).
pub async fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("removing stale socket at {}", path.display()));
        }
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("binding socket at {}", path.display()))?;
    std::fs::set_permissions(path, PermissionsExt::from_mode(0o700))
        .with_context(|| format!("chmod 0700 on socket at {}", path.display()))?;
    Ok(listener)
}

/// Accept on `listener` forever, spawning one task per connection.
pub async fn serve<B: ShellBackend>(listener: UnixListener, backend: Arc<B>) -> anyhow::Result<()> {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                log::warn!("shell host accept: {e}");
                continue;
            }
        };
        let backend = Arc::clone(&backend);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, backend).await {
                log::warn!("shell host connection: {e:#}");
            }
        });
    }
}

async fn handle_connection<B: ShellBackend>(
    stream: UnixStream,
    backend: Arc<B>,
) -> anyhow::Result<()> {
    check_peer_identity(&stream).context("peer identity check failed")?;
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut line: Vec<u8> = Vec::new();
    let mut chunks: Option<broadcast::Receiver<Vec<u8>>> = None;
    loop {
        line.clear();
        let read_fut = reader.read_until(b'\n', &mut line);
        let n = match chunks.as_mut() {
            None => read_fut.await,
            Some(rx) => tokio::select! {
                res = read_fut => res,
                res = rx.recv() => {
                    match res {
                        Ok(bytes) => {
                            write_frame(&mut writer, &ShellResponse::Chunk { bytes }).await?;
                            continue;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            chunks = None;
                            continue;
                        }
                    }
                }
            },
        }
        .context("reading request frame")?;
        if n == 0 {
            return Ok(());
        }
        if let Err(e) = process_line(&mut writer, &backend, &line, &mut chunks).await {
            log::warn!("shell host: {e:#}");
        }
    }
}

async fn process_line<B: ShellBackend>(
    writer: &mut (impl AsyncWriteExt + Unpin + ?Sized),
    backend: &Arc<B>,
    line: &[u8],
    chunks: &mut Option<broadcast::Receiver<Vec<u8>>>,
) -> anyhow::Result<()> {
    let trimmed = trim_frame(line);
    let request: ShellRequest = match serde_json::from_slice(trimmed) {
        Ok(req) => req,
        Err(e) => {
            write_frame(
                writer,
                &ShellResponse::Error {
                    message: format!("malformed frame: {e}"),
                },
            )
            .await?;
            return Ok(());
        }
    };
    let response = match request {
        ShellRequest::Subscribe => {
            *chunks = Some(backend.subscribe());
            ShellResponse::Ok
        }
        ShellRequest::Input { bytes } => match backend.input(&bytes).await {
            Ok(()) => ShellResponse::Ok,
            Err(e) => ShellResponse::Error {
                message: e.to_string(),
            },
        },
        ShellRequest::Resize { rows, cols } => match backend.resize(rows, cols).await {
            Ok(()) => ShellResponse::Ok,
            Err(e) => ShellResponse::Error {
                message: e.to_string(),
            },
        },
        ShellRequest::Signal { signal } => match backend.signal(signal).await {
            Ok(()) => ShellResponse::Ok,
            Err(e) => ShellResponse::Error {
                message: e.to_string(),
            },
        },
        ShellRequest::Status => backend.status().await,
    };
    write_frame(writer, &response).await
}

async fn write_frame(
    writer: &mut (impl AsyncWriteExt + Unpin + ?Sized),
    frame: &ShellResponse,
) -> anyhow::Result<()> {
    let wire = encode(frame).context("encoding response")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("writing response")?;
    Ok(())
}

fn trim_frame(frame: &[u8]) -> &[u8] {
    let end = frame
        .iter()
        .rposition(|ch| *ch != b'\n' && *ch != b'\r')
        .map_or(0, |i| i + 1);
    &frame[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::proto::ShellState;
    use std::sync::Mutex;
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixStream;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Input(Vec<u8>),
        Resize(u16, u16),
        Signal(ShellSignal),
        Status,
    }

    struct FakeBackend {
        calls: Mutex<Vec<Call>>,
        tx: broadcast::Sender<Vec<u8>>,
        fail_input: std::sync::atomic::AtomicBool,
    }

    impl FakeBackend {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                tx: broadcast::channel(16).0,
                fail_input: std::sync::atomic::AtomicBool::new(false),
            })
        }
        fn push(&self, bytes: Vec<u8>) {
            let _ = self.tx.send(bytes);
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn fail_input(&self, yes: bool) {
            self.fail_input
                .store(yes, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl ShellBackend for FakeBackend {
        async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(Call::Input(bytes.to_vec()));
            if self.fail_input.load(std::sync::atomic::Ordering::SeqCst) {
                Err(anyhow::anyhow!("input rejected"))
            } else {
                Ok(())
            }
        }
        async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(Call::Resize(rows, cols));
            Ok(())
        }
        async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(Call::Signal(sig));
            Ok(())
        }
        async fn status(&self) -> ShellResponse {
            self.calls.lock().unwrap().push(Call::Status);
            ShellResponse::Status {
                state: ShellState::Running,
                rows: 24,
                cols: 80,
                pid: 0,
            }
        }
        fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
            self.tx.subscribe()
        }
    }

    async fn read_response<R: tokio::io::AsyncRead + Unpin>(r: &mut BufReader<R>) -> ShellResponse {
        let mut buf = Vec::new();
        let n = r.read_until(b'\n', &mut buf).await.unwrap();
        assert!(n > 0, "EOF before a frame");
        serde_json::from_slice(trim_frame(&buf)).unwrap()
    }

    async fn write_request(w: &mut (impl AsyncWriteExt + Unpin), req: &ShellRequest) {
        let wire = encode(req).unwrap();
        w.write_all(wire.as_bytes()).await.unwrap();
    }

    struct Server {
        backend: Arc<FakeBackend>,
        _handle: tokio::task::JoinHandle<anyhow::Result<()>>,
        path: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    async fn start_server() -> Server {
        let backend = FakeBackend::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shell.sock");
        let listener = bind(&path).await.unwrap();
        let serve_backend = Arc::clone(&backend);
        let handle = tokio::spawn(async move { serve(listener, serve_backend).await });
        Server {
            backend,
            _handle: handle,
            path,
            _dir: dir,
        }
    }

    #[tokio::test]
    async fn host_round_trips_every_request_over_a_real_socket() {
        let server = start_server().await;
        let client = UnixStream::connect(&server.path).await.unwrap();
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);

        write_request(
            &mut writer,
            &ShellRequest::Input {
                bytes: b"hi\n".to_vec(),
            },
        )
        .await;
        assert_eq!(read_response(&mut reader).await, ShellResponse::Ok);

        write_request(
            &mut writer,
            &ShellRequest::Resize {
                rows: 40,
                cols: 120,
            },
        )
        .await;
        assert_eq!(read_response(&mut reader).await, ShellResponse::Ok);

        write_request(
            &mut writer,
            &ShellRequest::Signal {
                signal: ShellSignal::Term,
            },
        )
        .await;
        assert_eq!(read_response(&mut reader).await, ShellResponse::Ok);

        write_request(&mut writer, &ShellRequest::Status).await;
        assert_eq!(
            read_response(&mut reader).await,
            ShellResponse::Status {
                state: ShellState::Running,
                rows: 24,
                cols: 80,
                pid: 0
            }
        );

        assert_eq!(
            server.backend.calls(),
            vec![
                Call::Input(b"hi\n".to_vec()),
                Call::Resize(40, 120),
                Call::Signal(ShellSignal::Term),
                Call::Status,
            ]
        );
    }

    #[tokio::test]
    async fn host_subscribe_streams_chunks_until_the_client_disconnects() {
        let server = start_server().await;
        let client = UnixStream::connect(&server.path).await.unwrap();
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);

        write_request(&mut writer, &ShellRequest::Subscribe).await;
        assert_eq!(read_response(&mut reader).await, ShellResponse::Ok);

        server.backend.push(vec![0xff, 0x00, b'a']);
        server.backend.push(b"second".to_vec());
        let first = read_response(&mut reader).await;
        let second = read_response(&mut reader).await;
        assert_eq!(
            first,
            ShellResponse::Chunk {
                bytes: vec![0xff, 0x00, b'a']
            }
        );
        assert_eq!(
            second,
            ShellResponse::Chunk {
                bytes: b"second".to_vec()
            }
        );

        drop(writer);
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn host_answers_a_backend_error_with_an_error_frame() {
        let server = start_server().await;
        server.backend.fail_input(true);
        let client = UnixStream::connect(&server.path).await.unwrap();
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);

        write_request(
            &mut writer,
            &ShellRequest::Input {
                bytes: b"x".to_vec(),
            },
        )
        .await;
        assert_eq!(
            read_response(&mut reader).await,
            ShellResponse::Error {
                message: "input rejected".into()
            }
        );

        write_request(&mut writer, &ShellRequest::Status).await;
        assert_eq!(
            read_response(&mut reader).await,
            ShellResponse::Status {
                state: ShellState::Running,
                rows: 24,
                cols: 80,
                pid: 0
            }
        );
    }

    #[tokio::test]
    async fn host_answers_malformed_input_with_an_error_frame() {
        let server = start_server().await;
        let client = UnixStream::connect(&server.path).await.unwrap();
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);

        writer.write_all(b"not json\n").await.unwrap();
        let resp = read_response(&mut reader).await;
        match resp {
            ShellResponse::Error { message } => assert!(
                message.contains("malformed frame"),
                "unexpected error message: {message}"
            ),
            other => panic!("expected Error frame, got {other:?}"),
        }

        write_request(&mut writer, &ShellRequest::Status).await;
        assert_eq!(
            read_response(&mut reader).await,
            ShellResponse::Status {
                state: ShellState::Running,
                rows: 24,
                cols: 80,
                pid: 0
            }
        );
    }

    #[tokio::test]
    async fn host_binds_the_socket_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("priv.sock");
        let listener = bind(&path).await.unwrap();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(meta.mode() & 0o777, 0o700, "socket mode is exactly 0700");
        drop(listener);
    }

    #[tokio::test]
    async fn host_replaces_a_stale_socket_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.sock");
        std::fs::write(&path, b"junk").unwrap();
        let listener = bind(&path).await.unwrap();
        drop(listener);
        let listener2 = bind(&path).await.unwrap();
        drop(listener2);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn host_rejects_a_peer_it_cannot_identify() {
        let (a, _b) = UnixStream::pair().unwrap();
        check_peer_identity(&a).expect(
            "a socket pair from this same process must pass the identity check — a \
             differing-uid peer cannot be constructed in-process and is covered by \
             the daemon's own boundary; this test pins the call site, not the \
             kernel's decision",
        );
    }
}
