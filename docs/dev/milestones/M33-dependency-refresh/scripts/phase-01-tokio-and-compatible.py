"""M33 phase 01: every semver-compatible dependency at its latest, tokio 1.54's
AsyncFd deprecation fixed, and the stream-idle test's race closed."""

def edit(path, pairs):
    s = open(path).read()
    for old, new, n in pairs:
        assert s.count(old) == n, (path, old[:70], s.count(old))
        s = s.replace(old, new)
    open(path, 'w').write(s)

# ---- Cargo.toml: each requirement names the latest stable release ----
# rusqlite, syn and vt100 are the breaking upgrades of phases 02 and 03.
edit('Cargo.toml', [
    ('axum = { version = "0.8",', 'axum = { version = "0.8.9",', 1),
    ('async-trait = "0.1"\n', 'async-trait = "0.1.92"\n', 1),
    ('chrono = { version = "0.4",', 'chrono = { version = "0.4.45",', 1),
    ('cron = "0.17"\n', 'cron = "0.17.0"\n', 1),
    ('clap = { version = "4.5",', 'clap = { version = "4.6.7",', 1),
    ('env_logger = { version = "0.11",', 'env_logger = { version = "0.11.11",', 1),
    ('futures-util = "0.3"\n', 'futures-util = "0.3.34"\n', 1),
    ('libc = "0.2"\n', 'libc = "0.2.191"\n', 1),
    ('log = "0.4"\n', 'log = "0.4.35"\n', 1),
    ('portable-pty = "0.9"\n', 'portable-pty = "0.9.0"\n', 1),
    ('regex = "1"\n', 'regex = "1.13.1"\n', 1),
    ('reqwest = { version = "0.13.4",', 'reqwest = { version = "0.13.5",', 1),
    ('ratatui = { version = "0.30",', 'ratatui = { version = "0.30.2",', 1),
    ('crossterm = "0.29"\n', 'crossterm = "0.29.0"\n', 1),
    ('serde_json = "1.0.151"\n', 'serde_json = "1.0.154"\n', 1),
    ('similar = "3"\n', 'similar = "3.2.1"\n', 1),
    ('tokio = { version = "1.53.1",', 'tokio = { version = "1.54.1",', 1),
    ('toml = "1.1"\nuuid = { version = "1", features = ["v4"] }\nzeroize = "1"\n',
     'toml = "1.1.8"\nuuid = { version = "1.29.0", features = ["v4"] }\nzeroize = "1.9.1"\n', 1),
    # The dev entries for serde, serde_json, toml and uuid repeat the normal
    # ones and add nothing; tests see normal dependencies already.
    ('''[dev-dependencies]
filetime = "0.2"
proc-macro2 = { version = "1.0.107", features = ["span-locations"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
syn = { version = "2.0.119", features = ["full", "visit"] }
tempfile = "3"
tokio = { version = "1", features = ["test-util"] }
toml = "1.1"
uuid = { version = "1", features = ["v4"] }
''', '''[dev-dependencies]
filetime = "0.2.29"
proc-macro2 = { version = "1.0.107", features = ["span-locations"] }
syn = { version = "2.0.119", features = ["full", "visit"] }
tempfile = "3.27.0"
tokio = { version = "1.54.1", features = ["test-util"] }
''', 1),
])
print("Cargo.toml requires the latest of every compatible dependency")

# ---- tty.rs: an owned File registered with AsyncFd::register ----
T = 'src/cli/input/tty.rs'
edit(T, [
    ('''/// Owned file descriptor for /dev/tty opened with O_NONBLOCK.
///
/// We open a *fresh* file description for the controlling terminal rather than
/// setting O_NONBLOCK on STDIN_FILENO (fd 0).  fcntl(F_SETFL) operates on the
/// open file description, which stdin/stdout/stderr typically share (they are
/// dup'd from the same terminal fd).  Setting O_NONBLOCK on fd 0 therefore
/// propagates to fd 1 (stdout), causing write() to return EAGAIN when the
/// terminal output buffer is full — which Rust's print! macro converts into a
/// panic.  By using an independent /dev/tty fd we avoid touching the shared
/// file description at all.
struct TtyFd(libc::c_int);

impl Drop for TtyFd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

impl std::os::unix::io::AsRawFd for TtyFd {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.0
    }
}

/// Single async reader over the controlling terminal, shared by the main input
/// loop and tool-call approval prompts.  Supports raw-mode byte-at-a-time
/// reading (for the interactive line editor) and cooked-mode line reading (for
/// simple y/n prompts) through the same `AsyncFd` registration.
pub struct AsyncStdin(tokio::io::unix::AsyncFd<TtyFd>);

impl AsyncStdin {
    pub fn new() -> anyhow::Result<Self> {
        // Open the controlling terminal as a fresh, independent file description
        // with O_NONBLOCK.  This leaves the file description shared by
        // stdin/stdout/stderr (fd 0/1/2) in blocking mode.
        let fd = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(anyhow::anyhow!(
                "open /dev/tty: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(tokio::io::unix::AsyncFd::new(TtyFd(fd))?))
    }

    /// Construct from an already-open non-blocking fd.
    ///
    /// Used in tests to inject a pipe instead of `/dev/tty`.
    #[cfg(test)]
    pub(crate) fn from_raw_fd(fd: std::os::unix::io::RawFd) -> anyhow::Result<Self> {
        Ok(Self(tokio::io::unix::AsyncFd::new(TtyFd(fd))?))
    }

    /// Read one raw byte from the terminal asynchronously.
    pub async fn read_byte(&self) -> Option<u8> {
        let mut buf = [0u8; 1];
        let fd = self.0.get_ref().0;
        loop {
            let mut guard = self.0.readable().await.ok()?;
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            if n == 1 {
                return Some(buf[0]); // guard dropped → readiness retained for next byte
            } else if n == 0 {
                return None; // EOF
            } else {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    guard.clear_ready(); // stale readiness; wait for next epoll event
                } else {
                    return None;
                }
            }
        }
    }
''', '''/// Single async reader over the controlling terminal, shared by the main input
/// loop and tool-call approval prompts.  Supports raw-mode byte-at-a-time
/// reading (for the interactive line editor) and cooked-mode line reading (for
/// simple y/n prompts) through the same `AsyncFd` registration.
///
/// The `File` is /dev/tty opened with O_NONBLOCK as a *fresh* file description,
/// rather than O_NONBLOCK set on STDIN_FILENO (fd 0).  fcntl(F_SETFL) operates
/// on the open file description, which stdin/stdout/stderr typically share
/// (they are dup'd from the same terminal fd).  Setting O_NONBLOCK on fd 0
/// therefore propagates to fd 1 (stdout), causing write() to return EAGAIN
/// when the terminal output buffer is full — which Rust's print! macro
/// converts into a panic.  By using an independent /dev/tty fd we avoid
/// touching the shared file description at all.
pub struct AsyncStdin(tokio::io::unix::AsyncFd<std::fs::File>);

impl AsyncStdin {
    pub fn new() -> anyhow::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        // Open the controlling terminal as a fresh, independent file description
        // with O_NONBLOCK.  This leaves the file description shared by
        // stdin/stdout/stderr (fd 0/1/2) in blocking mode.
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/tty")
            .map_err(|e| anyhow::anyhow!("open /dev/tty: {e}"))?;
        Self::register(tty)
    }

    /// Construct from an already-open non-blocking fd, which the reader then
    /// owns and closes.
    ///
    /// Used in tests to inject a pipe instead of `/dev/tty`.
    #[cfg(test)]
    pub(crate) fn from_raw_fd(fd: std::os::unix::io::RawFd) -> anyhow::Result<Self> {
        use std::os::unix::io::FromRawFd;
        // SAFETY: the callers pass a pipe end they just created and never use
        // again, so this `File` is its only owner.
        Self::register(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    fn register(file: std::fs::File) -> anyhow::Result<Self> {
        // SAFETY: `AsyncFd` takes the `File` by value and owns it until it is
        // dropped: nothing else holds the fd, so it stays open and names the
        // same file description for the registration's whole life, and
        // `File::as_raw_fd` always returns that one fd. No `AsyncFd`
        // constructor is safe from tokio 1.54 on (tokio PR #8587).
        let fd = unsafe { tokio::io::unix::AsyncFd::register(file) }
            .map_err(std::io::Error::from)?;
        Ok(Self(fd))
    }

    /// Read one raw byte from the terminal asynchronously.
    pub async fn read_byte(&self) -> Option<u8> {
        use std::io::Read;
        let mut buf = [0u8; 1];
        loop {
            let mut guard = self.0.readable().await.ok()?;
            let mut tty: &std::fs::File = self.0.get_ref();
            match tty.read(&mut buf) {
                Ok(1) => return Some(buf[0]), // guard dropped → readiness retained for next byte
                Ok(_) => return None,         // EOF
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    guard.clear_ready(); // stale readiness; wait for next epoll event
                }
                Err(_) => return None,
            }
        }
    }
''', 1),
])
print("AsyncStdin owns a File registered with AsyncFd::register")

# ---- the stream-idle test's fake server reads the request first ----
edit('src/ai/mod.rs', [
    ('''            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = sock
''', '''            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                // Read the whole request before answering. A response that
                // arrives before the request is written is an
                // `UnexpectedMessage` to hyper, and under tokio 1.54's
                // scheduling that race fired on every run.
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\\r\\n\\r\\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = sock
''', 1),
])
print("the stream-idle fake server reads the request before it answers")

# ---- a test for the drained-pipe read, the arm no test reached ----
edit(T, [
    ('''    #[tokio::test]
    async fn read_key_bare_cr_yields_enter() {
''', '''    /// The second byte arrives after the first was drained: the read sees
    /// the readiness it kept, gets `WouldBlock`, clears it and waits for the
    /// byte instead of reporting EOF.
    #[tokio::test]
    async fn read_byte_waits_out_a_drained_pipe() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"a").await;
        assert_eq!(stdin.read_byte().await, Some(b'a'));
        let writer = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            write_bytes(&write_file, b"b").await;
        };
        let (byte, ()) = tokio::join!(
            tokio::time::timeout(KEY_READ_BOUND, stdin.read_byte()),
            writer
        );
        assert_eq!(
            byte.expect("the read waits for the byte"),
            Some(b'b'),
            "a drained pipe is not EOF"
        );
    }

    #[tokio::test]
    async fn read_key_bare_cr_yields_enter() {
''', 1),
])
print("read_byte_waits_out_a_drained_pipe pins the WouldBlock arm")
