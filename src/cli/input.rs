

use crate::cli::render::*;

// ── Async stdin wrapper ───────────────────────────────────────────────────────

/// Non-owning handle to fd 0 used with `AsyncFd`.  Does not close the fd on
/// drop — closing stdin would break the process.
struct StdinRawFd;

impl std::os::unix::io::AsRawFd for StdinRawFd {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd { libc::STDIN_FILENO }
}

/// Single async reader over stdin (fd 0), shared by the main input loop and
/// tool-call approval prompts.  Supports raw-mode byte-at-a-time reading
/// (for the interactive line editor) and cooked-mode line reading (for simple
/// y/n prompts) through the same `AsyncFd` registration.
pub struct AsyncStdin(tokio::io::unix::AsyncFd<StdinRawFd>);

impl AsyncStdin {
    pub fn new() -> anyhow::Result<Self> {
        // AsyncFd requires the fd to be in O_NONBLOCK mode.
        unsafe {
            let flags = libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL, 0);
            libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        Ok(Self(tokio::io::unix::AsyncFd::new(StdinRawFd)?))
    }

    /// Read one raw byte from stdin asynchronously.
    pub async fn read_byte(&self) -> Option<u8> {
        let mut buf = [0u8; 1];
        loop {
            let mut guard = self.0.readable().await.ok()?;
            let n = unsafe {
                libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1)
            };
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
                b              => line.push(b as char),
            }
        }
    }
}

// ── Interactive line editor ───────────────────────────────────────────────────

/// A single editable line: a character buffer and a cursor position.
pub struct InputLine {
    buf:    Vec<char>,
    cursor: usize, // character index, 0 ..= buf.len()
}

impl InputLine {
    pub fn new() -> Self { Self { buf: Vec::new(), cursor: 0 } }

    fn from_str(s: &str) -> Self {
        let buf: Vec<char> = s.chars().collect();
        let cursor = buf.len();
        Self { buf, cursor }
    }

    fn insert(&mut self, c: char) { self.buf.insert(self.cursor, c); self.cursor += 1; }
    fn backspace(&mut self) {
        if self.cursor > 0 { self.buf.remove(self.cursor - 1); self.cursor -= 1; }
    }
    fn delete(&mut self) {
        if self.cursor < self.buf.len() { self.buf.remove(self.cursor); }
    }
    fn move_left(&mut self)  { if self.cursor > 0               { self.cursor -= 1; } }
    fn move_right(&mut self) { if self.cursor < self.buf.len()  { self.cursor += 1; } }
    fn move_home(&mut self)  { self.cursor = 0; }
    fn move_end(&mut self)   { self.cursor = self.buf.len(); }
    fn kill_to_end(&mut self)   { self.buf.truncate(self.cursor); }
    fn kill_to_start(&mut self) { self.buf.drain(..self.cursor); self.cursor = 0; }
    fn as_string(&self) -> String { self.buf.iter().collect() }
}

/// Session-wide input state: the current line plus the navigable history.
pub struct InputState {
    current:     InputLine,
    history:     Vec<String>,
    history_idx: Option<usize>,
    saved:       String, // current line stashed while browsing history
}

impl InputState {
    pub fn new() -> Self {
        Self { current: InputLine::new(), history: Vec::new(),
               history_idx: None, saved: String::new() }
    }

    /// Commit a query to history and reset the current line to empty.
    pub fn push_history(&mut self, s: String) {
        if !s.is_empty() && self.history.last().map(|l| l.as_str()) != Some(&s) {
            self.history.push(s);
        }
        self.history_idx = None;
        self.saved = String::new();
        self.current = InputLine::new();
    }

    fn history_up(&mut self) {
        if self.history.is_empty() { return; }
        let new_idx = match self.history_idx {
            None    => { self.saved = self.current.as_string(); self.history.len() - 1 }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_idx = Some(new_idx);
        self.current = InputLine::from_str(&self.history[new_idx].clone());
    }

    fn history_down(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_idx = None;
                let s = self.saved.clone();
                self.current = InputLine::from_str(&s);
            }
            Some(i) => {
                let new_idx = i + 1;
                self.history_idx = Some(new_idx);
                self.current = InputLine::from_str(&self.history[new_idx].clone());
            }
        }
    }
}

/// Parsed key event from raw-mode terminal input.
enum Key {
    Char(char),
    Backspace, Delete,
    Left, Right, Up, Down,
    Home, End,
    Enter,
    CtrlA, CtrlE, CtrlK, CtrlU,
    CtrlC, CtrlD,
}

/// Switch stdin to raw (non-canonical, no-echo) mode.
/// Returns the saved termios for later restoration via `restore_termios`.
fn set_raw_mode() -> anyhow::Result<libc::termios> {
    unsafe {
        let mut old = std::mem::MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(libc::STDIN_FILENO, old.as_mut_ptr()) != 0 {
            return Err(anyhow::anyhow!("tcgetattr: {}", std::io::Error::last_os_error()));
        }
        let old = old.assume_init();
        let mut raw = old;
        // Disable: CR→NL, flow control, parity, strip, break-int.
        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        // Disable output processing (so \n doesn't become \r\n).
        raw.c_oflag &= !libc::OPOST;
        // 8-bit characters.
        raw.c_cflag = (raw.c_cflag & !libc::CSIZE) | libc::CS8;
        // Disable: echo, canonical mode, extended processing, signal generation.
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        // Return after each byte, no timeout.
        raw.c_cc[libc::VMIN  as usize] = 1;
        raw.c_cc[libc::VTIME as usize] = 0;
        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw) != 0 {
            return Err(anyhow::anyhow!("tcsetattr: {}", std::io::Error::last_os_error()));
        }
        Ok(old)
    }
}

fn restore_termios(old: libc::termios) {
    unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &old); }
}

/// Redraw the input row to reflect `line`'s current buffer and cursor.
///
/// The visible window scrolls horizontally to keep the cursor in view.
/// Layout: `│ ❯ <text>│`  — text occupies columns 5 … (chat_width − 1).
fn render_input_row(line: &InputLine, input_row: usize, chat_width: usize) {
    use std::io::Write;
    let avail  = chat_width.saturating_sub(5); // chars available for text
    let len    = line.buf.len();
    let cursor = line.cursor;

    // Viewport start: keep cursor centred, clamped so we never show past the end.
    let view_start = if len <= avail || cursor <= avail / 2 {
        0
    } else if len.saturating_sub(cursor) < avail / 2 {
        len.saturating_sub(avail)
    } else {
        cursor.saturating_sub(avail / 2)
    };
    let view_end   = (view_start + avail).min(len);
    let visible: String = line.buf[view_start..view_end].iter().collect();
    let cursor_col = 5 + (cursor - view_start); // 1-indexed terminal column

    print!("\x1b[{input_row};1H\x1b[2K");
    print!("\x1b[1m\x1b[96m│\x1b[0m \x1b[92m❯\x1b[0m {}", visible);
    print!("\x1b7");
    print!("\x1b[{input_row};{chat_width}H\x1b[1m\x1b[96m│\x1b[0m");
    print!("\x1b8");
    print!("\x1b[{input_row};{}H", cursor_col);
    std::io::stdout().flush().ok();
}

/// Read and parse one key event from raw-mode stdin.
///
/// Arrow keys and other escape sequences are consumed with a 30 ms inter-byte
/// timeout so a lone Escape is distinguishable from a CSI sequence.
async fn read_key(stdin: &AsyncStdin) -> Option<Key> {
    use tokio::time::{Duration, timeout};

    let b = stdin.read_byte().await?;
    Some(match b {
        b'\r' | b'\n'      => Key::Enter,
        b'\x7f' | b'\x08' => Key::Backspace,
        b'\x01'            => Key::CtrlA,
        b'\x03'            => Key::CtrlC,
        b'\x04'            => Key::CtrlD,
        b'\x05'            => Key::CtrlE,
        b'\x0b'            => Key::CtrlK,
        b'\x15'            => Key::CtrlU,
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
                        Ok(Some(b'3')) => { // \x1b[3~ = Delete
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::Delete
                        }
                        Ok(Some(b'1')) | Ok(Some(b'7')) => { // \x1b[1~ / \x1b[7~ = Home
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::Home
                        }
                        Ok(Some(b'4')) | Ok(Some(b'8')) => { // \x1b[4~ / \x1b[8~ = End
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
                        _              => Key::Char('\x1b'),
                    }
                }
                _ => Key::Char('\x1b'), // bare Escape
            }
        }
        c if c < 0x20 => Key::Char('\0'), // ignore other control chars
        c if c < 0x80 => Key::Char(c as char),
        c => {
            // Multi-byte UTF-8: accumulate continuation bytes.
            let extra = if c >= 0xF0 { 3 } else if c >= 0xE0 { 2 } else { 1 };
            let mut utf8 = vec![c];
            for _ in 0..extra {
                match tokio::time::timeout(
                    tokio::time::Duration::from_millis(30),
                    stdin.read_byte(),
                ).await {
                    Ok(Some(b)) => utf8.push(b),
                    _           => break,
                }
            }
            match std::str::from_utf8(&utf8) {
                Ok(s)  => s.chars().next().map_or(Key::Char('\0'), Key::Char),
                Err(_) => Key::Char('\0'),
            }
        }
    })
}

/// Run the interactive line-editor loop in raw mode.
///
/// Handles history navigation (↑/↓), cursor movement (←/→, Ctrl+A/E),
/// and kill shortcuts (Ctrl+K/U).  Integrates the SIGWINCH resize handler
/// so the input row repaints correctly after a terminal resize.
/// Returns `None` on EOF or Ctrl+D with an empty buffer.
pub async fn read_input_line(
    state:          &mut InputState,
    stdin:          &AsyncStdin,
    sigwinch:       &mut tokio::signal::unix::Signal,
    chat_width:     &mut usize,
    chat_height:    &mut usize,
    start_time:     std::time::Instant,
    session_id:     &str,
    status:         &str,
    approval_hint:  &str,
) -> anyhow::Result<Option<String>> {
    let old = set_raw_mode()?;
    let result = read_input_line_inner(
        state, stdin, sigwinch, chat_width, chat_height, start_time, session_id, status, approval_hint,
    ).await;
    restore_termios(old);
    result
}

async fn read_input_line_inner(
    state:          &mut InputState,
    stdin:          &AsyncStdin,
    sigwinch:       &mut tokio::signal::unix::Signal,
    chat_width:     &mut usize,
    chat_height:    &mut usize,
    start_time:     std::time::Instant,
    session_id:     &str,
    status:         &str,
    approval_hint:  &str,
) -> anyhow::Result<Option<String>> {
    // Initial render of the (empty or restored) input row.
    render_input_row(&state.current, chat_height.saturating_sub(2).max(1), *chat_width);

    loop {
        let row = chat_height.saturating_sub(2).max(1);
        tokio::select! {
            _ = sigwinch.recv() => {
                *chat_width  = terminal_width();
                *chat_height = terminal_height();
                setup_scroll_region(*chat_height);
                draw_input_frame(*chat_height, *chat_width, start_time);
                draw_status_bar(*chat_height, *chat_width, session_id, status, approval_hint);
                render_input_row(&state.current, chat_height.saturating_sub(2).max(1), *chat_width);
            }
            key = read_key(stdin) => {
                let Some(key) = key else { return Ok(None); };
                match key {
                    Key::Enter  => return Ok(Some(state.current.as_string())),
                    Key::CtrlD  => {
                        if state.current.buf.is_empty() { return Ok(None); }
                        state.current.delete();
                        render_input_row(&state.current, row, *chat_width);
                    }
                    Key::CtrlC  => {
                        // Clear the current line; history nav position is reset.
                        state.current = InputLine::new();
                        state.history_idx = None;
                        render_input_row(&state.current, row, *chat_width);
                    }
                    Key::Char(c) if c != '\0' => {
                        state.current.insert(c);
                        render_input_row(&state.current, row, *chat_width);
                    }
                    Key::Backspace               => { state.current.backspace();    render_input_row(&state.current, row, *chat_width); }
                    Key::Delete                  => { state.current.delete();       render_input_row(&state.current, row, *chat_width); }
                    Key::Left                    => { state.current.move_left();    render_input_row(&state.current, row, *chat_width); }
                    Key::Right                   => { state.current.move_right();   render_input_row(&state.current, row, *chat_width); }
                    Key::Up                      => { state.history_up();           render_input_row(&state.current, row, *chat_width); }
                    Key::Down                    => { state.history_down();          render_input_row(&state.current, row, *chat_width); }
                    Key::Home   | Key::CtrlA     => { state.current.move_home();    render_input_row(&state.current, row, *chat_width); }
                    Key::End    | Key::CtrlE     => { state.current.move_end();     render_input_row(&state.current, row, *chat_width); }
                    Key::CtrlK                   => { state.current.kill_to_end();  render_input_row(&state.current, row, *chat_width); }
                    Key::CtrlU                   => { state.current.kill_to_start();render_input_row(&state.current, row, *chat_width); }
                    _ => {}
                }
            }
        }
    }
}


/// Read a password from stdin with terminal echo disabled so it is not shown.
pub fn read_password_silent(prompt: &str) -> anyhow::Result<String> {
    use std::io::{BufRead, Write};
    print!("{}", prompt);
    std::io::stdout().flush()?;

    let fd = libc::STDIN_FILENO;

    // AsyncStdin sets O_NONBLOCK on fd 0 so the async reader can work with epoll.
    // Synchronous read_line returns EAGAIN immediately when O_NONBLOCK is set, so
    // clear it here and restore it after the read.
    let saved_flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if saved_flags >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, saved_flags & !libc::O_NONBLOCK) };
    }

    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    let termios_ok = unsafe { libc::tcgetattr(fd, &mut old) } == 0;

    if termios_ok {
        let mut new = old;
        new.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &new) };
    }

    let mut input = String::new();
    let result = std::io::stdin().lock().read_line(&mut input);

    if termios_ok {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
    }

    // Restore O_NONBLOCK so the async stdin reader continues to work.
    if saved_flags >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, saved_flags) };
    }

    println!(); // newline after silent input
    result?;
    Ok(input.trim_end_matches('\n').trim_end_matches('\r').to_string())
}

