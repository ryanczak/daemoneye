//! Bounded reads for the daemon's IPC socket and the shell host: a line
//! reader that refuses a line past a cap before buffering it whole, and an
//! accept wrapper that holds a semaphore permit so the accept loop cannot
//! spawn an unbounded number of connection tasks.
//!
//! Both tools are cancel-safe in the way the socket loops need: a line
//! dropped mid-read keeps its consumed bytes in the caller's buffer, and a
//! permit-wait dropped before `accept` returns the permit unused.

use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::net::UnixListener;
use tokio::net::UnixStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The daemon's cap on a single request line (1 MiB): the largest request
/// `Request` deserialises from before the connection is refused.
pub const MAX_IPC_MESSAGE_BYTES: usize = 1 << 20;

/// The shell host's cap on a single frame (16 MiB): an `Input` frame encodes
/// its bytes as a JSON number array, about 4 bytes per input byte, so 16 MiB
/// is about 4 MiB of pasted input.
pub const MAX_SHELL_FRAME_BYTES: usize = 16 << 20;

/// How many connections the daemon's accept loop serves concurrently.
pub const MAX_DAEMON_CONNECTIONS: usize = 128;

/// How many connections one shell host serves concurrently.
pub const MAX_SHELL_HOST_CONNECTIONS: usize = 32;

/// Read up to and including the first `\n` into `buf`, refusing when `buf`
/// as a whole would pass `cap`.
///
/// `cap` bounds `buf` as a whole, including bytes left in it by an earlier
/// call, so a caller reading frames one at a time must drain `buf` between
/// calls (drain keeps the unread tail). `Ok(0)` means EOF with nothing
/// appended; a partial line at EOF is returned as it is, like
/// `read_until`. Cancel-safe: every consumed byte is already in `buf`, so
/// dropping a future mid-line loses nothing.
pub async fn read_bounded_line<R>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<usize>
where
    R: AsyncBufRead + Unpin,
{
    let mut appended = 0usize;
    loop {
        let available = match reader.fill_buf().await {
            Ok(slice) => slice,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            // EOF: a partial line is returned as it is; nothing new is Ok(0).
            return Ok(appended);
        }
        let remaining = cap.saturating_sub(buf.len());
        match available.iter().position(|&b| b == b'\n') {
            None => {
                // No newline yet: refuse as soon as the buffer would pass
                // the cap, appending only what fits before the error.
                if available.len() > remaining {
                    buf.extend_from_slice(&available[..remaining]);
                    reader.consume(remaining);
                    return Err(oversized(cap));
                }
                let chunk = available.len();
                buf.extend_from_slice(available);
                reader.consume(chunk);
                appended += chunk;
            }
            Some(newline) => {
                // The line including its newline is buf.len() + newline + 1
                // bytes in total.
                let line_len = newline + 1;
                if buf.len() + line_len > cap {
                    return Err(oversized(cap));
                }
                buf.extend_from_slice(&available[..line_len]);
                reader.consume(line_len);
                return Ok(appended + line_len);
            }
        }
    }
}

fn oversized(cap: usize) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("line exceeds cap of {cap} bytes"),
    )
}

/// Accept one connection, holding a permit from `permits` before `accept`
/// runs.
///
/// When every permit is held, nothing more is accepted: new connections
/// wait in the kernel backlog. Both awaits (`acquire_owned` and `accept`)
/// are cancel-safe, so dropping a waiting call leaks no permit.
pub async fn accept_bounded(
    listener: &UnixListener,
    permits: &Arc<Semaphore>,
) -> std::io::Result<(UnixStream, OwnedSemaphorePermit)> {
    let permit = Arc::clone(permits)
        .acquire_owned()
        .await
        .map_err(|_| std::io::Error::other("semaphore closed"))?;
    let (stream, _addr) = listener.accept().await?;
    Ok((stream, permit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::AsyncWriteExt;
    use tokio::time::{Duration, timeout};

    async fn bounded<R>(reader: &mut R, buf: &mut Vec<u8>, cap: usize) -> std::io::Result<usize>
    where
        R: AsyncBufRead + Unpin,
    {
        timeout(Duration::from_secs(5), read_bounded_line(reader, buf, cap))
            .await
            .expect("read completes within 5 s")
    }

    #[tokio::test]
    async fn bounded_line_reads_one_line_and_leaves_the_rest() {
        let mut reader = Cursor::new(b"first\nsecond\n".to_vec());
        let mut buf = Vec::new();
        let n = bounded(&mut reader, &mut buf, 1024).await.unwrap();
        assert_eq!(n, 6, "only the first line's bytes are reported");
        assert_eq!(buf, b"first\n");
        buf.clear();
        let n = bounded(&mut reader, &mut buf, 1024).await.unwrap();
        assert_eq!(n, 7);
        assert_eq!(buf, b"second\n");
    }

    #[tokio::test]
    async fn bounded_line_accepts_a_line_of_exactly_the_cap() {
        let mut line = vec![b'a'; 15];
        line.push(b'\n');
        let mut reader = Cursor::new(line.clone());
        let mut buf = Vec::new();
        let n = bounded(&mut reader, &mut buf, 16).await.unwrap();
        assert_eq!(n, 16, "a line exactly the cap is accepted");
        assert_eq!(buf, line);
    }

    #[tokio::test]
    async fn bounded_line_refuses_one_byte_over_the_cap() {
        let mut line = vec![b'a'; 16];
        line.push(b'\n');
        let mut reader = Cursor::new(line);
        let mut buf = Vec::new();
        let err = bounded(&mut reader, &mut buf, 16).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn bounded_line_returns_a_partial_line_at_eof() {
        let mut reader = Cursor::new(b"tail without newline".to_vec());
        let mut buf = Vec::new();
        let n = bounded(&mut reader, &mut buf, 1024).await.unwrap();
        assert_eq!(n, 20, "a partial line at EOF is returned as it is");
        assert_eq!(buf, b"tail without newline");

        buf.clear();
        let n = bounded(&mut reader, &mut buf, 1024).await.unwrap();
        assert_eq!(n, 0, "EOF with nothing appended is Ok(0)");
    }

    #[tokio::test]
    async fn bounded_line_cap_counts_bytes_already_in_the_buffer() {
        let mut reader = Cursor::new(b"abc\n".to_vec());
        let mut buf = vec![b'x'; 14];
        let err = bounded(&mut reader, &mut buf, 16).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// An `AsyncBufRead` source that hands out one 64 KiB chunk of `a` per
    /// `fill_buf` and counts every byte `consume` has taken, enough for
    /// 64 MiB in total.
    struct CountingSource {
        remaining: usize,
        handed_out: usize,
        exposed: Vec<u8>,
    }

    impl CountingSource {
        const CHUNK: usize = 64 * 1024;
        fn new(total: usize) -> Self {
            Self {
                remaining: total,
                handed_out: 0,
                exposed: Vec::new(),
            }
        }
    }

    impl tokio::io::AsyncRead for CountingSource {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _out: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            unreachable!("read_bounded_line goes through fill_buf exclusively")
        }
    }

    impl tokio::io::AsyncBufRead for CountingSource {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            let this = self.get_mut();
            if this.exposed.is_empty() && this.remaining > 0 {
                let n = Self::CHUNK.min(this.remaining);
                this.exposed = vec![b'a'; n];
                this.remaining -= n;
            }
            std::task::Poll::Ready(Ok(&this.exposed[..]))
        }
        fn consume(self: std::pin::Pin<&mut Self>, amt: usize) {
            let this = self.get_mut();
            this.exposed.drain(..amt);
            this.handed_out += amt;
        }
    }

    #[tokio::test]
    async fn bounded_line_refusal_reads_within_budget() {
        const CAP: usize = 1024 * 1024;
        let mut reader = CountingSource::new(64 * 1024 * 1024);
        let mut buf = Vec::new();
        let err = bounded(&mut reader, &mut buf, CAP).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            reader.handed_out <= CAP + CountingSource::CHUNK,
            "refusal must come after at most cap + one chunk: handed out {}",
            reader.handed_out
        );
    }

    #[tokio::test]
    async fn bounded_line_keeps_bytes_across_a_cancelled_call() {
        let (mut tx, rx) = tokio::io::duplex(64 * 1024);
        tx.write_all(b"half ").await.unwrap();
        tx.flush().await.unwrap();

        let mut rx = tokio::io::BufReader::new(rx);
        let mut buf = Vec::new();
        {
            let call = read_bounded_line(&mut rx, &mut buf, 1024);
            tokio::pin!(call);
            // The timeout fires before a newline ever arrives, dropping the
            // call mid-line.
            let _ = timeout(Duration::from_millis(50), &mut call).await;
        }

        tx.write_all(b"of a line\n").await.unwrap();
        tx.flush().await.unwrap();

        let n = bounded(&mut rx, &mut buf, 1024).await.unwrap();
        assert_eq!(n, 10, "the second call completes the whole line");
        assert_eq!(buf, b"half of a line\n");
    }

    fn temp_listener() -> (tempfile::TempDir, Arc<UnixListener>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accept.sock");
        let listener = UnixListener::bind(&path).unwrap();
        (dir, Arc::new(listener))
    }

    fn socket_path(listener: &UnixListener) -> std::path::PathBuf {
        listener
            .local_addr()
            .unwrap()
            .as_pathname()
            .unwrap()
            .to_path_buf()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn accept_bounded_waits_for_a_free_permit() {
        let (_dir, listener) = temp_listener();
        let permits = Arc::new(Semaphore::new(1));
        let path = socket_path(&listener);

        let _client1 = UnixStream::connect(&path).await.unwrap();
        let first = accept_bounded(&listener, &permits).await.unwrap();
        assert_eq!(permits.available_permits(), 0, "the permit is held");
        let second = tokio::spawn({
            let listener = Arc::clone(&listener);
            let permits = Arc::clone(&permits);
            async move { accept_bounded(&listener, &permits).await }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !second.is_finished(),
            "the second accept must wait while the permit is held"
        );

        let _client2 = UnixStream::connect(&path).await.unwrap();
        drop(first);
        drop(_client1);
        tokio::task::yield_now().await;
        let (stream, _permit) = timeout(Duration::from_secs(2), second)
            .await
            .expect("the second accept returns once the permit is free")
            .unwrap()
            .unwrap();
        drop(stream);
        assert_eq!(permits.available_permits(), 0, "its permit is held");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accept_bounded_waiting_call_can_be_dropped() {
        let (_dir, listener) = temp_listener();
        let permits = Arc::new(Semaphore::new(1));
        let path = socket_path(&listener);

        let _client1 = UnixStream::connect(&path).await.unwrap();
        let first = accept_bounded(&listener, &permits).await.unwrap();

        let _client2 = UnixStream::connect(&path).await.unwrap();
        let waiting = tokio::spawn({
            let listener = Arc::clone(&listener);
            let permits = Arc::clone(&permits);
            async move { accept_bounded(&listener, &permits).await }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "the call is still waiting");
        waiting.abort();

        drop(first);
        let fresh = accept_bounded(&listener, &permits).await.unwrap();
        drop(fresh);
        assert_eq!(
            permits.available_permits(),
            1,
            "dropping a waiting call leaks no permit"
        );
    }
}
