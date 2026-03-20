

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
pub fn set_raw_mode() -> anyhow::Result<libc::termios> {
    unsafe {
        let mut old = std::mem::MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(libc::STDIN_FILENO, old.as_mut_ptr()) != 0 {
            return Err(anyhow::anyhow!("tcgetattr: {}", std::io::Error::last_os_error()));
        }
        let old = old.assume_init();
        let mut raw = old;
        // Disable: echo, canonical mode, extended processing, signal generation.
        // This ensures Ctrl+C is read as 0x03 instead of generating SIGINT.
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

pub fn restore_termios(old: libc::termios) {
    unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &old); }
}

/// Maximum number of rows the input area can grow to.
const MAX_INPUT_ROWS: usize = 20;

/// How many display rows `line` needs at the given terminal width.
fn input_rows_needed(line: &InputLine, chat_width: usize, chat_height: usize) -> usize {
    let avail = chat_width.saturating_sub(5).max(1);
    let len = line.buf.len();
    let cap = MAX_INPUT_ROWS.min(chat_height / 3).max(1);
    if len == 0 { 1 } else { ((len + avail - 1) / avail).min(cap).max(1) }
}

/// Render the word-wrapped multi-row input area.
///
/// The last input row is always at `height − 2`.  Earlier rows sit immediately
/// above it.  The first row shows `│ ❯ text│`; continuation rows show `│   text│`.
/// The terminal cursor is placed at the correct row/column for the current
/// buffer cursor position.
fn render_input_multiline(line: &InputLine, height: usize, chat_width: usize, rows: usize) {
    use std::io::Write;
    let avail    = chat_width.saturating_sub(5).max(1);
    let n_chars  = line.buf.len();
    let last_row = height.saturating_sub(2).max(1);
    let first_row = last_row.saturating_sub(rows.saturating_sub(1));

    for i in 0..rows {
        let row = first_row + i;
        let start_char = i * avail;
        let end_char   = (start_char + avail).min(n_chars);
        let visible: String = if start_char < n_chars {
            line.buf[start_char..end_char].iter().collect()
        } else {
            String::new()
        };
        let prefix = if i == 0 {
            "\x1b[38;5;88m\x1b[1m│\x1b[0m \x1b[92m❯\x1b[0m "
        } else {
            "\x1b[38;5;88m\x1b[1m│\x1b[0m   "
        };
        print!("\x1b[{row};1H\x1b[2K{}{}", prefix, visible);
        // Right border — save/restore cursor so we don't lose our position.
        print!("\x1b7\x1b[{row};{chat_width}H\x1b[38;5;88m\x1b[1m│\x1b[0m\x1b8");
    }

    // Place the terminal cursor at the character position in the buffer.
    let cursor_row_idx   = line.cursor / avail;
    let cursor_col_in_row = line.cursor % avail;
    let cursor_row = (first_row + cursor_row_idx).min(last_row);
    let cursor_col = 5 + cursor_col_in_row; // 1-indexed
    print!("\x1b[{cursor_row};{cursor_col}H");
    std::io::stdout().flush().ok();
}

/// Resize the input area from `old_rows` to `new_rows`, updating the scroll
/// region and redrawing the frame and status bar.  Clears any rows that are
/// switching between input and scroll-region territory.
fn resize_input_area(
    height: usize,
    width: usize,
    old_rows: usize,
    new_rows: usize,
    start_time: std::time::Instant,
    session_id: &str,
    approval_hint: &str,
    model: &str,
    prompt_tokens: u32,
    context_window: u32,
    daemon_up: bool,
) {
    use std::io::Write;
    if old_rows == new_rows { return; }

    // When shrinking, clear from the old border row up to (but not including)
    // the new border row so the old border glyph doesn't remain as an artifact.
    // draw_input_frame_n clears the new border row itself; render_input_multiline
    // clears each input row.  When expanding no explicit clearing is needed
    // because those functions handle it.
    if old_rows > new_rows {
        let old_border = height.saturating_sub(2 + old_rows);
        let new_border = height.saturating_sub(2 + new_rows);
        for r in old_border..new_border {
            print!("\x1b[{r};1H\x1b[2K");
        }
        std::io::stdout().flush().ok();
    }

    setup_scroll_region_n(height, new_rows);
    draw_input_frame_n(height, width, new_rows, start_time);
    draw_status_bar(height, width, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
}

/// Collapse the input area back to 1 row (called before returning from the
/// input loop so callers always see a clean 1-row layout).
fn collapse_input_area(
    height: usize,
    width: usize,
    input_rows: usize,
    start_time: std::time::Instant,
    session_id: &str,
    approval_hint: &str,
    model: &str,
    prompt_tokens: u32,
    context_window: u32,
    daemon_up: bool,
) {
    if input_rows <= 1 { return; }
    resize_input_area(height, width, input_rows, 1, start_time, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
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
    approval_hint:  &str,
    model:          &str,
    prompt_tokens:  u32,
    context_window: u32,
    last_ctrl_c:    &mut Option<std::time::Instant>,
    daemon_up:      bool,
) -> anyhow::Result<Option<String>> {
    read_input_line_inner(
        state, stdin, sigwinch, chat_width, chat_height, start_time,
        session_id, approval_hint, model, prompt_tokens, context_window, last_ctrl_c, daemon_up,
    ).await
}

async fn read_input_line_inner(
    state:          &mut InputState,
    stdin:          &AsyncStdin,
    sigwinch:       &mut tokio::signal::unix::Signal,
    chat_width:     &mut usize,
    chat_height:    &mut usize,
    start_time:     std::time::Instant,
    session_id:     &str,
    approval_hint:  &str,
    model:          &str,
    prompt_tokens:  u32,
    context_window: u32,
    last_ctrl_c:    &mut Option<std::time::Instant>,
    daemon_up:      bool,
) -> anyhow::Result<Option<String>> {
    let mut input_rows = input_rows_needed(&state.current, *chat_width, *chat_height);

    // Initial render.
    render_input_multiline(&state.current, *chat_height, *chat_width, input_rows);

    // Macro-style helper: recalculate needed rows, resize the input area if it
    // changed, then repaint the buffer.
    macro_rules! render {
        () => {{
            let needed = input_rows_needed(&state.current, *chat_width, *chat_height);
            if needed != input_rows {
                resize_input_area(*chat_height, *chat_width, input_rows, needed,
                                  start_time, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
                input_rows = needed;
            }
            render_input_multiline(&state.current, *chat_height, *chat_width, input_rows);
        }};
    }

    loop {
        tokio::select! {
            _ = sigwinch.recv() => {
                // Save previous frame geometry so we can erase the old rows.
                let old_height     = *chat_height;
                let old_input_rows = input_rows;

                *chat_width  = terminal_width();
                *chat_height = terminal_height();
                // Recalculate rows for new width — may change without input change.
                input_rows = input_rows_needed(&state.current, *chat_width, *chat_height);

                // On resize, the terminal emulator (or tmux) may reset DECSTBM,
                // causing the old fixed frame rows to appear as regular scrollable
                // content above the new frame.  Erase those rows explicitly before
                // re-establishing the scroll region to prevent border artifacts.
                {
                    use std::io::Write;
                    // Reset scroll region so CUP can reach any row.
                    print!("\x1b[r");
                    let old_frame_top = old_height.saturating_sub(2 + old_input_rows).max(1);
                    for r in old_frame_top..=old_height {
                        print!("\x1b[{r};1H\x1b[2K");
                    }
                    std::io::stdout().flush().ok();
                }

                setup_scroll_region_n(*chat_height, input_rows);
                draw_input_frame_n(*chat_height, *chat_width, input_rows, start_time);
                draw_status_bar(*chat_height, *chat_width, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
                render_input_multiline(&state.current, *chat_height, *chat_width, input_rows);
            }
            key = read_key(stdin) => {
                let Some(key) = key else {
                    collapse_input_area(*chat_height, *chat_width, input_rows,
                                        start_time, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
                    return Ok(None);
                };
                match key {
                    Key::Enter => {
                        let s = state.current.as_string();
                        collapse_input_area(*chat_height, *chat_width, input_rows,
                                            start_time, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
                        return Ok(Some(s));
                    }
                    Key::CtrlD => {
                        if state.current.buf.is_empty() {
                            collapse_input_area(*chat_height, *chat_width, input_rows,
                                                start_time, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
                            return Ok(None);
                        }
                        state.current.delete();
                        render!();
                    }
                    Key::CtrlC => {
                        if let Some(t) = last_ctrl_c {
                            if t.elapsed() < std::time::Duration::from_millis(1000) {
                                collapse_input_area(*chat_height, *chat_width, input_rows,
                                                    start_time, session_id, approval_hint, model, prompt_tokens, context_window, daemon_up);
                                return Ok(None); // Double Ctrl+C: exit chat
                            }
                        }
                        *last_ctrl_c = Some(std::time::Instant::now());
                        state.current = InputLine::new();
                        state.history_idx = None;
                        render!();
                        // Show a brief hint in the scroll area (above the frame) using
                        // DEC save/restore so the input cursor is not disturbed.
                        {
                            use std::io::Write;
                            let hint_row = (*chat_height).saturating_sub(3 + input_rows).max(1);
                            print!("\x1b7\x1b[{hint_row};1H\x1b[2m  (press Ctrl+C again to exit)\x1b[K\x1b[0m\x1b8");
                            std::io::stdout().flush().ok();
                        }
                    }
                    Key::Char(c) if c != '\0' => { state.current.insert(c);       render!(); }
                    Key::Backspace             => { state.current.backspace();     render!(); }
                    Key::Delete                => { state.current.delete();        render!(); }
                    Key::Left                  => { state.current.move_left();     render!(); }
                    Key::Right                 => { state.current.move_right();    render!(); }
                    Key::Up                    => { state.history_up();            render!(); }
                    Key::Down                  => { state.history_down();          render!(); }
                    Key::Home | Key::CtrlA     => { state.current.move_home();     render!(); }
                    Key::End  | Key::CtrlE     => { state.current.move_end();      render!(); }
                    Key::CtrlK                 => { state.current.kill_to_end();   render!(); }
                    Key::CtrlU                 => { state.current.kill_to_start(); render!(); }
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

