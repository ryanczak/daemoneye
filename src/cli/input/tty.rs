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
    CtrlA,
    CtrlE,
    CtrlK,
    CtrlU,
    CtrlC,
    CtrlD,
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
