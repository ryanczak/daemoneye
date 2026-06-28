/// Owned file descriptor for /dev/tty opened with O_NONBLOCK.
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

    /// Read a line (up to `\n` or `\r`, not included).  Works in both cooked
    /// and raw terminal modes.
    pub async fn read_line(&self) -> Option<String> {
        let mut line = String::new();
        loop {
            match self.read_byte().await? {
                b'\n' | b'\r' => return Some(line),
                b => line.push(b as char),
            }
        }
    }
}

/// Parsed key event from raw-mode terminal input.
#[derive(Debug, PartialEq)]
pub enum Key {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Enter,
    /// Insert a newline in the editor (Alt+Enter / Ctrl+J).
    CtrlJ,
    CtrlA,
    CtrlE,
    CtrlK,
    CtrlU,
    CtrlC,
    CtrlD,
    /// Terminal focus-in report (`ESC [ I`) — this tmux pane regained focus.
    FocusGained,
    /// Terminal focus-out report (`ESC [ O`) — this tmux pane lost focus.
    FocusLost,
    /// A pasted block of text (bracketed paste).
    Paste(String),
}

/// Switch stdin to raw (non-canonical, no-echo) mode.
/// Returns the saved termios for later restoration via `restore_termios`.
pub fn set_raw_mode() -> anyhow::Result<libc::termios> {
    unsafe {
        let mut old = std::mem::MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(libc::STDIN_FILENO, old.as_mut_ptr()) != 0 {
            return Err(anyhow::anyhow!(
                "tcgetattr: {}",
                std::io::Error::last_os_error()
            ));
        }
        let old = old.assume_init();
        let mut raw = old;
        // Disable: echo, canonical mode, extended processing, signal generation.
        // This ensures Ctrl+C is read as 0x03 instead of generating SIGINT.
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        // Return after each byte, no timeout.
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw) != 0 {
            return Err(anyhow::anyhow!(
                "tcsetattr: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(old)
    }
}

pub fn restore_termios(old: Option<libc::termios>) {
    if let Some(old) = old {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &old);
        }
    }
}

/// Read and parse one key event from raw-mode stdin.
///
/// Arrow keys and other escape sequences are consumed with a 30 ms inter-byte
/// timeout so a lone Escape is distinguishable from a CSI sequence.
pub async fn read_key(stdin: &AsyncStdin) -> Option<Key> {
    use tokio::time::{Duration, timeout};

    let b = stdin.read_byte().await?;
    Some(match b {
        b'\r' | b'\n' => Key::Enter,
        b'\x7f' | b'\x08' => Key::Backspace,
        b'\x01' => Key::CtrlA,
        b'\x03' => Key::CtrlC,
        b'\x04' => Key::CtrlD,
        b'\x05' => Key::CtrlE,
        b'\x0b' => Key::CtrlK,
        b'\x15' => Key::CtrlU,
        b'\x1b' => {
            match timeout(Duration::from_millis(30), stdin.read_byte()).await {
                Ok(Some(b'[')) => {
                    match timeout(Duration::from_millis(30), stdin.read_byte()).await {
                        Ok(Some(b'A')) => Key::Up,
                        Ok(Some(b'B')) => Key::Down,
                        Ok(Some(b'C')) => Key::Right,
                        Ok(Some(b'D')) => Key::Left,
                        Ok(Some(b'H')) => Key::Home,
                        Ok(Some(b'F')) => Key::End,
                        // Focus reporting (DEC private mode 1004): ESC[I / ESC[O.
                        Ok(Some(b'I')) => Key::FocusGained,
                        Ok(Some(b'O')) => Key::FocusLost,
                        Ok(Some(b'3')) => {
                            // \x1b[3~ = Delete
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::Delete
                        }
                        Ok(Some(b'1')) | Ok(Some(b'7')) => {
                            // \x1b[1~ / \x1b[7~ = Home
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::Home
                        }
                        Ok(Some(b'4')) | Ok(Some(b'8')) => {
                            // \x1b[4~ / \x1b[8~ = End
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::End
                        }
                        Ok(Some(b'2')) => {
                            // Could be bracketed paste start: ESC [ 2 0 0 ~
                            match timeout(Duration::from_millis(30), stdin.read_byte()).await {
                                Ok(Some(b'0')) => {
                                    match timeout(Duration::from_millis(30), stdin.read_byte())
                                        .await
                                    {
                                        Ok(Some(b'0')) => {
                                            match timeout(
                                                Duration::from_millis(30),
                                                stdin.read_byte(),
                                            )
                                            .await
                                            {
                                                Ok(Some(b'~')) => {
                                                    // ESC[200~ = bracketed paste start
                                                    let text = read_bracketed_paste(stdin).await;
                                                    Key::Paste(text)
                                                }
                                                _ => Key::Char('\x1b'),
                                            }
                                        }
                                        _ => Key::Char('\x1b'),
                                    }
                                }
                                _ => Key::Char('\x1b'),
                            }
                        }
                        _ => Key::Char('\x1b'),
                    }
                }
                Ok(Some(b'O')) => {
                    match timeout(Duration::from_millis(30), stdin.read_byte()).await {
                        Ok(Some(b'H')) => Key::Home,
                        Ok(Some(b'F')) => Key::End,
                        _ => Key::Char('\x1b'),
                    }
                }
                Ok(Some(b'\r')) | Ok(Some(b'\n')) => {
                    // Alt+Enter: ESC followed by CR/LF → deliberate newline
                    Key::CtrlJ
                }
                _ => Key::Char('\x1b'), // bare Escape
            }
        }
        c if c < 0x20 => Key::Char('\0'), // ignore other control chars
        c if c < 0x80 => Key::Char(c as char),
        c => {
            // Multi-byte UTF-8: accumulate continuation bytes.
            let extra = if c >= 0xF0 {
                3
            } else if c >= 0xE0 {
                2
            } else {
                1
            };
            let mut utf8 = vec![c];
            for _ in 0..extra {
                match tokio::time::timeout(
                    tokio::time::Duration::from_millis(30),
                    stdin.read_byte(),
                )
                .await
                {
                    Ok(Some(b)) => utf8.push(b),
                    _ => break,
                }
            }
            match std::str::from_utf8(&utf8) {
                Ok(s) => s.chars().next().map_or(Key::Char('\0'), Key::Char),
                Err(_) => Key::Char('\0'),
            }
        }
    })
}

/// Read a bracketed paste: bytes between `ESC[200~` (already consumed) and `ESC[201~`.
/// Returns the pasted text as a String.
async fn read_bracketed_paste(stdin: &AsyncStdin) -> String {
    use tokio::time::{Duration, timeout};
    let mut paste_buf = Vec::new();
    loop {
        match timeout(Duration::from_millis(50), stdin.read_byte()).await {
            Ok(Some(0x1b)) => {
                // Check for ESC[201~ (bracketed paste end)
                match timeout(Duration::from_millis(10), stdin.read_byte()).await {
                    Ok(Some(b'[')) => {
                        let b2 = timeout(Duration::from_millis(10), stdin.read_byte()).await;
                        let b3 = timeout(Duration::from_millis(10), stdin.read_byte()).await;
                        let b4 = timeout(Duration::from_millis(10), stdin.read_byte()).await;
                        let b5 = timeout(Duration::from_millis(10), stdin.read_byte()).await;
                        if b2 == Ok(Some(b'2'))
                            && b3 == Ok(Some(b'0'))
                            && b4 == Ok(Some(b'1'))
                            && b5 == Ok(Some(b'~'))
                        {
                            return String::from_utf8_lossy(&paste_buf).to_string();
                        }
                        // Not the end sequence — push ESC and whatever we read back
                        paste_buf.push(0x1b);
                        if let Ok(Some(b)) = b2 {
                            paste_buf.push(b);
                        }
                        if let Ok(Some(b)) = b3 {
                            paste_buf.push(b);
                        }
                        if let Ok(Some(b)) = b4 {
                            paste_buf.push(b);
                        }
                        if let Ok(Some(b)) = b5 {
                            paste_buf.push(b);
                        }
                    }
                    _ => {
                        // Not ESC[, treat as bare ESC
                        paste_buf.push(0x1b);
                    }
                }
            }
            Ok(Some(b)) => {
                paste_buf.push(b);
            }
            _ => {
                // Timeout — return what we have
                return String::from_utf8_lossy(&paste_buf).to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::{AsRawFd, FromRawFd};

    /// Create a pipe and return an AsyncStdin reading from the read end.
    fn make_pipe_stdin() -> (AsyncStdin, std::fs::File) {
        // pipe2 with O_NONBLOCK on both ends
        let mut fds: [libc::c_int; 2] = [-1, -1];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        assert_eq!(ret, 0, "pipe2 failed: {}", std::io::Error::last_os_error());

        let read_end = fds[0];
        let write_end = fds[1];

        let stdin = AsyncStdin::from_raw_fd(read_end).expect("from_raw_fd");

        // Wrap the write end in a File for easy writing
        let write_file = unsafe { std::fs::File::from_raw_fd(write_end) };
        (stdin, write_file)
    }

    /// Write bytes into the write file and wait a bit for them to be available.
    async fn write_bytes(file: &std::fs::File, bytes: &[u8]) {
        let fd = file.as_raw_fd();
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let n = unsafe {
                libc::write(
                    fd,
                    remaining.as_ptr() as *const libc::c_void,
                    remaining.len(),
                )
            };
            if n > 0 {
                remaining = &remaining[n as usize..];
            } else {
                // EAGAIN is fine, just loop
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        // Give the async reader time to see the data
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn read_key_bare_cr_yields_enter() {
        let (stdin, write_file) = make_pipe_stdin();
        // Write a bare CR
        write_bytes(&write_file, b"\r").await;
        let key = read_key(&stdin).await;
        assert_eq!(key, Some(Key::Enter), "bare CR should yield Enter");
    }

    #[tokio::test]
    async fn read_key_bare_lf_yields_enter() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\n").await;
        let key = read_key(&stdin).await;
        assert_eq!(key, Some(Key::Enter), "bare LF should yield Enter");
    }

    #[tokio::test]
    async fn read_key_alt_enter_yields_ctrlj() {
        let (stdin, write_file) = make_pipe_stdin();
        // Alt+Enter: ESC followed by CR
        write_bytes(&write_file, &[0x1b, b'\r']).await;
        let key = read_key(&stdin).await;
        assert_eq!(
            key,
            Some(Key::CtrlJ),
            "Alt+Enter (ESC CR) should yield CtrlJ"
        );
    }

    #[tokio::test]
    async fn read_key_alt_enter_lf_yields_ctrlj() {
        let (stdin, write_file) = make_pipe_stdin();
        // Alt+Enter: ESC followed by LF
        write_bytes(&write_file, &[0x1b, b'\n']).await;
        let key = read_key(&stdin).await;
        assert_eq!(
            key,
            Some(Key::CtrlJ),
            "Alt+Enter (ESC LF) should yield CtrlJ"
        );
    }

    #[tokio::test]
    async fn read_key_bracketed_paste_single_line() {
        let (stdin, write_file) = make_pipe_stdin();
        // ESC[200~ hello ESC[201~
        let payload: Vec<u8> = vec![
            0x1b, b'[', b'2', b'0', b'0', b'~', b'h', b'e', b'l', b'l', b'o', 0x1b, b'[', b'2',
            b'0', b'1', b'~',
        ];
        write_bytes(&write_file, &payload).await;
        let key = read_key(&stdin).await;
        assert!(
            matches!(&key, Some(Key::Paste(s)) if s == "hello"),
            "bracketed paste should yield Paste(\"hello\"), got: {:?}",
            key
        );
    }

    #[tokio::test]
    async fn read_key_bracketed_paste_multiline() {
        let (stdin, write_file) = make_pipe_stdin();
        // ESC[200~ line1\nline2 ESC[201~
        let payload: Vec<u8> = vec![
            0x1b, b'[', b'2', b'0', b'0', b'~', b'l', b'i', b'n', b'e', b'1', b'\n', b'l', b'i',
            b'n', b'e', b'2', 0x1b, b'[', b'2', b'0', b'1', b'~',
        ];
        write_bytes(&write_file, &payload).await;
        let key = read_key(&stdin).await;
        assert!(
            matches!(&key, Some(Key::Paste(s)) if s == "line1\nline2"),
            "multiline bracketed paste should yield Paste(\"line1\\nline2\"), got: {:?}",
            key
        );
    }

    #[tokio::test]
    async fn read_key_bracketed_paste_no_enter_mid_paste() {
        let (stdin, write_file) = make_pipe_stdin();
        // ESC[200~ a\nb ESC[201~ — the \n inside should NOT produce Key::Enter
        let payload: Vec<u8> = vec![
            0x1b, b'[', b'2', b'0', b'0', b'~', b'a', b'\n', b'b', 0x1b, b'[', b'2', b'0', b'1',
            b'~',
        ];
        write_bytes(&write_file, &payload).await;
        let key = read_key(&stdin).await;
        // Should be a single Paste, not Enter
        assert!(
            matches!(&key, Some(Key::Paste(s)) if s == "a\nb"),
            "paste with embedded newline should yield single Paste, got: {:?}",
            key
        );
    }

    #[tokio::test]
    async fn read_key_backspace() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\x7f").await;
        let key = read_key(&stdin).await;
        assert_eq!(key, Some(Key::Backspace));
    }

    #[tokio::test]
    async fn read_key_arrow_up() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, &[0x1b, b'[', b'A']).await;
        let key = read_key(&stdin).await;
        assert_eq!(key, Some(Key::Up));
    }

    #[tokio::test]
    async fn read_key_char() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"x").await;
        let key = read_key(&stdin).await;
        assert_eq!(key, Some(Key::Char('x')));
    }
}
