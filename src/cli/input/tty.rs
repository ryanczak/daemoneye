/// Single async reader over the controlling terminal, shared by the main input
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
        let fd =
            unsafe { tokio::io::unix::AsyncFd::register(file) }.map_err(std::io::Error::from)?;
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
    /// Ctrl+O — open the alt-screen transcript viewer.
    CtrlO,
    PageUp,
    PageDown,
    /// Terminal focus-in report (`ESC [ I`) — this tmux pane regained focus.
    FocusGained,
    /// Terminal focus-out report (`ESC [ O`) — this tmux pane lost focus.
    FocusLost,
    /// A pasted block of text (bracketed paste).
    Paste(String),
    /// A mouse report from inside the transcript viewer (SGR 1006).
    /// `col`/`row` are 0-based, converted from the wire's 1-based values.
    Mouse {
        button: u8,
        col: u16,
        row: u16,
        pressed: bool,
    },
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
        b'\x0f' => Key::CtrlO,
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
                        // SGR mouse report (1006): ESC[<Cb;Cx;CyM|m.
                        Ok(Some(b'<')) => match read_sgr_mouse(stdin).await {
                            Some(key) => key,
                            None => Key::Char('\0'),
                        },
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
                        Ok(Some(b'5')) => {
                            // \x1b[5~ = PageUp
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::PageUp
                        }
                        Ok(Some(b'6')) => {
                            // \x1b[6~ = PageDown
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::PageDown
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

/// Read the rest of an SGR mouse report after `ESC[<` (already consumed):
/// decimal fields `Cb;Cx;Cy`, terminated by `M` (press) or `m` (release).
/// Returns `None` on a malformed or truncated sequence.
async fn read_sgr_mouse(stdin: &AsyncStdin) -> Option<Key> {
    use tokio::time::{Duration, timeout};

    let first = timeout(Duration::from_millis(30), stdin.read_byte())
        .await
        .ok()
        .flatten()?;
    let mut cb: u16 = match first {
        c @ b'0'..=b'9' => (c - b'0') as u16,
        _ => return None,
    };
    loop {
        match timeout(Duration::from_millis(30), stdin.read_byte()).await {
            Ok(Some(c @ b'0'..=b'9')) => cb = cb * 10 + (c - b'0') as u16,
            Ok(Some(b';')) => break,
            _ => return None,
        }
    }

    let mut cx: u16 = 0;
    loop {
        match timeout(Duration::from_millis(30), stdin.read_byte()).await {
            Ok(Some(c @ b'0'..=b'9')) => cx = cx * 10 + (c - b'0') as u16,
            Ok(Some(b';')) => break,
            _ => return None,
        }
    }

    let mut cy: u16 = 0;
    loop {
        match timeout(Duration::from_millis(30), stdin.read_byte()).await {
            Ok(Some(c @ b'0'..=b'9')) => cy = cy * 10 + (c - b'0') as u16,
            Ok(Some(b'M')) => {
                return Some(Key::Mouse {
                    button: cb as u8,
                    col: cx.saturating_sub(1),
                    row: cy.saturating_sub(1),
                    pressed: true,
                });
            }
            Ok(Some(b'm')) => {
                return Some(Key::Mouse {
                    button: cb as u8,
                    col: cx.saturating_sub(1),
                    row: cy.saturating_sub(1),
                    pressed: false,
                });
            }
            _ => return None,
        }
    }
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

    /// Write bytes into the write file. Returns once every byte is in the pipe
    /// buffer; the reader sees them immediately, so no wait is needed.
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
                // A short write on this pipe can only mean EAGAIN; anything else is
                // a real bug and must fail loudly rather than spin forever.
                let err = std::io::Error::last_os_error();
                assert_eq!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock,
                    "write to test pipe failed: {err}"
                );
                tokio::task::yield_now().await;
            }
        }
    }

    /// How long a test will wait for `read_key` before declaring the read starved.
    ///
    /// Generous on purpose: it is only ever paid when something is already broken,
    /// so a slow machine must not trip it.
    const KEY_READ_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

    /// `read_key`, but a starved read panics instead of hanging the suite.
    ///
    /// `read_key`'s first `read_byte()` is deliberately unbounded — production
    /// awaits it in a `select!` while the user thinks. That is correct there and
    /// fatal here: a regression that stops bytes reaching it would hang CI rather
    /// than fail it.
    async fn read_key_bounded(stdin: &AsyncStdin) -> Option<Key> {
        read_key_within(stdin, KEY_READ_BOUND).await
    }

    /// `read_key_bounded` with an explicit bound, so the guard itself is testable.
    async fn read_key_within(stdin: &AsyncStdin, bound: std::time::Duration) -> Option<Key> {
        match tokio::time::timeout(bound, read_key(stdin)).await {
            Ok(key) => key,
            Err(_) => panic!("read_key did not return within {bound:?} — no byte reached it"),
        }
    }

    /// The second byte arrives after the first was drained: the read sees
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
        let (stdin, write_file) = make_pipe_stdin();
        // Write a bare CR
        write_bytes(&write_file, b"\r").await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(key, Some(Key::Enter), "bare CR should yield Enter");
    }

    #[tokio::test]
    async fn read_key_bare_lf_yields_enter() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\n").await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(key, Some(Key::Enter), "bare LF should yield Enter");
    }

    #[tokio::test]
    async fn read_key_alt_enter_yields_ctrlj() {
        let (stdin, write_file) = make_pipe_stdin();
        // Alt+Enter: ESC followed by CR
        write_bytes(&write_file, &[0x1b, b'\r']).await;
        let key = read_key_bounded(&stdin).await;
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
        let key = read_key_bounded(&stdin).await;
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
        let key = read_key_bounded(&stdin).await;
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
        let key = read_key_bounded(&stdin).await;
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
        let key = read_key_bounded(&stdin).await;
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
        let key = read_key_bounded(&stdin).await;
        assert_eq!(key, Some(Key::Backspace));
    }

    #[tokio::test]
    async fn read_key_arrow_up() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, &[0x1b, b'[', b'A']).await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(key, Some(Key::Up));
    }

    #[tokio::test]
    async fn read_key_char() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"x").await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(key, Some(Key::Char('x')));
    }

    #[tokio::test]
    #[should_panic(expected = "read_key did not return within")]
    async fn read_key_within_panics_when_no_byte_ever_arrives() {
        // `_write_file` MUST stay bound: holding the pipe's write end open is what
        // makes the read block. Dropping it closes the pipe and `read_key` returns
        // `None` at once (EOF), which would pass this test for the wrong reason.
        let (stdin, _write_file) = make_pipe_stdin();
        let _ = read_key_within(&stdin, std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn read_key_parses_sgr_wheel_up() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\x1b[<64;10;5M").await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(
            key,
            Some(Key::Mouse {
                button: 64,
                col: 9,
                row: 4,
                pressed: true
            }),
            "SGR wheel-up must convert 1-based coords to 0-based"
        );
    }

    #[tokio::test]
    async fn read_key_parses_multi_digit_mouse_coords() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\x1b[<0;137;42M").await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(
            key,
            Some(Key::Mouse {
                button: 0,
                col: 136,
                row: 41,
                pressed: true
            }),
            "multi-digit columns (137) must parse as a single field"
        );
    }

    #[tokio::test]
    async fn read_key_mouse_release_is_not_pressed() {
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\x1b[<0;10;5m").await;
        let key = read_key_bounded(&stdin).await;
        assert_eq!(
            key,
            Some(Key::Mouse {
                button: 0,
                col: 9,
                row: 4,
                pressed: false
            }),
            "lowercase-m terminator must yield pressed: false"
        );
    }

    #[tokio::test]
    async fn read_key_malformed_mouse_sequence_is_ignored() {
        // Truncated sequence: ESC[<64;10 with nothing after — must not hang and
        // must degrade to the parser's ignore value.
        let (stdin, write_file) = make_pipe_stdin();
        write_bytes(&write_file, b"\x1b[<64;10").await;
        let key = read_key_within(&stdin, std::time::Duration::from_millis(300)).await;
        assert_eq!(
            key,
            Some(Key::Char('\0')),
            "truncated SGR mouse sequence must yield the ignore value"
        );
    }
}
