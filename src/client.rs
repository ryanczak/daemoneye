use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::Config;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};

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
struct AsyncStdin(tokio::io::unix::AsyncFd<StdinRawFd>);

impl AsyncStdin {
    fn new() -> anyhow::Result<Self> {
        // AsyncFd requires the fd to be in O_NONBLOCK mode.
        unsafe {
            let flags = libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL, 0);
            libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        Ok(Self(tokio::io::unix::AsyncFd::new(StdinRawFd)?))
    }

    /// Read one raw byte from stdin asynchronously.
    async fn read_byte(&self) -> Option<u8> {
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
    async fn read_line(&self) -> Option<String> {
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
struct InputLine {
    buf:    Vec<char>,
    cursor: usize, // character index, 0 ..= buf.len()
}

impl InputLine {
    fn new() -> Self { Self { buf: Vec::new(), cursor: 0 } }

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
struct InputState {
    current:     InputLine,
    history:     Vec<String>,
    history_idx: Option<usize>,
    saved:       String, // current line stashed while browsing history
}

impl InputState {
    fn new() -> Self {
        Self { current: InputLine::new(), history: Vec::new(),
               history_idx: None, saved: String::new() }
    }

    /// Commit a query to history and reset the current line to empty.
    fn push_history(&mut self, s: String) {
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
async fn read_input_line(
    state:       &mut InputState,
    stdin:       &AsyncStdin,
    sigwinch:    &mut tokio::signal::unix::Signal,
    chat_width:  &mut usize,
    chat_height: &mut usize,
    start_time:  std::time::Instant,
    session_id:  &str,
    status:      &str,
) -> anyhow::Result<Option<String>> {
    let old = set_raw_mode()?;
    let result = read_input_line_inner(
        state, stdin, sigwinch, chat_width, chat_height, start_time, session_id, status,
    ).await;
    restore_termios(old);
    result
}

async fn read_input_line_inner(
    state:       &mut InputState,
    stdin:       &AsyncStdin,
    sigwinch:    &mut tokio::signal::unix::Signal,
    chat_width:  &mut usize,
    chat_height: &mut usize,
    start_time:  std::time::Instant,
    session_id:  &str,
    status:      &str,
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
                draw_status_bar(*chat_height, *chat_width, session_id, status);
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

/// True if the command string contains `sudo` as a standalone word.
/// Mirrors the same check in daemon.rs so the client classifies commands
/// identically for session-level approval.
fn command_is_sudo(cmd: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:^|[;&|])\s*sudo\b").unwrap());
    re.is_match(cmd)
}

/// Per-session auto-approval flags for the two command classes.
/// Once set, the corresponding class is approved without prompting
/// for the rest of the chat session.
#[derive(Default)]
struct SessionApproval {
    regular: bool, // auto-approve non-sudo commands
    sudo: bool,    // auto-approve sudo commands
}

pub fn run_setup() -> Result<()> {
    // Write the systemd user service file.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let systemd_dir = PathBuf::from(&home).join(".config/systemd/user");
    let service_path = systemd_dir.join("daemoneye.service");

    let service_content = "\
[Unit]
Description=DaemonEye Tmux Daemon
After=network.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/daemoneye daemon
ExecStop=%h/.cargo/bin/daemoneye stop
Restart=on-failure
RestartSec=5
Environment=\"PATH=%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"

[Install]
WantedBy=default.target
";

    match std::fs::create_dir_all(&systemd_dir)
        .and_then(|_| std::fs::write(&service_path, service_content))
    {
        Ok(()) => {
            println!("Wrote {}", service_path.display());
            println!();
            println!("# Enable and start the daemon:");
            println!("systemctl --user daemon-reload");
            println!("systemctl --user enable --now daemoneye");
            println!();
            println!("# Check status and view logs:");
            println!("systemctl --user status daemoneye");
            println!("daemoneye logs");
        }
        Err(e) => {
            eprintln!("Warning: could not write service file: {}", e);
            eprintln!("You can install it manually:");
            eprintln!("  mkdir -p ~/.config/systemd/user");
            eprintln!("  cp daemoneye.service ~/.config/systemd/user/");
        }
    }

    let position = Config::load()
        .unwrap_or_default()
        .ai
        .position;
    let split_flag = match position.as_str() {
        "right"  => "-h",
        "left"   => "-bh",
        "top"    => "-bv",
        _        => "-v",   // "bottom" or any unrecognised value
    };

    // Use the absolute path to the running binary so the bind-key works even
    // when ~/.cargo/bin is not in the PATH inherited by the tmux session (a
    // common issue when the daemon created the session from a background
    // process or service with a minimal environment).
    let daemon_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!(
        "bind-key T split-window {} -e \"DAEMONEYE_SOURCE_PANE=#{{pane_id}}\" '{} chat'",
        split_flag, daemon_bin
    );
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");
    println!();
    println!("# If you already have a bind-key that uses the bare name 'daemoneye',");
    println!("# replace it with the full path above — the tmux session may not");
    println!("# inherit ~/.cargo/bin in its PATH.");

    Ok(())
}

pub fn run_logs(path: PathBuf) -> Result<()> {
    if !path.exists() {
        eprintln!("No log file found at {}.", path.display());
        eprintln!("The daemon writes logs there by default when started with: daemoneye daemon");
        std::process::exit(1);
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("tail")
        .args(["-f", path.to_str().unwrap_or("")])
        .exec();
    anyhow::bail!("Failed to exec tail: {}", err)
}

pub async fn run_stop() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Shutdown).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon stopped."),
                _ => {
                    println!("Daemon did not respond to shutdown.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ping() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Ping).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon is running."),
                _ => {
                    println!("Daemon is not responding.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ask(query: String) -> Result<()> {
    let stdin = AsyncStdin::new()?;
    let mut approval = SessionApproval::default(); // never persists; single-shot has no session
    ask_with_session(query, None, None, &stdin, Some(terminal_width()), &mut approval).await
}

/// List all available prompts from ~/.daemoneye/prompts/.
pub fn run_prompts() -> Result<()> {
    use crate::config::{load_named_prompt, prompts_dir};

    let dir = prompts_dir();
    let mut entries: Vec<(String, String)> = Vec::new();

    if dir.is_dir() {
        let mut paths: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |x| x == "toml"))
            .collect();
        paths.sort_by_key(|e| e.file_name());

        for entry in paths {
            let name = entry.path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let def = load_named_prompt(&name);
            entries.push((name, def.description));
        }
    }

    if entries.is_empty() {
        println!("No prompts found in {}", dir.display());
        println!("Create a prompt file: {}/my-prompt.toml", dir.display());
        return Ok(());
    }

    let name_width = entries.iter().map(|(n, _)| n.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mAvailable prompts\x1b[0m  ({})", dir.display());
    println!();
    for (name, desc) in &entries {
        println!("  \x1b[1m\x1b[96m{:<width$}\x1b[0m  {}", name, desc, width = name_width);
    }
    println!();
    println!("  Use \x1b[1m/prompt <name>\x1b[0m in chat to switch, or set \x1b[1mprompt = \"<name>\"\x1b[0m in config.toml.");
    Ok(())
}

/// List scripts in ~/.daemoneye/scripts/ (read directly, no daemon needed).
pub fn run_scripts() -> Result<()> {
    let scripts = crate::scripts::list_scripts()?;
    if scripts.is_empty() {
        let dir = crate::scripts::scripts_dir();
        println!("No scripts found in {}", dir.display());
        println!("Ask the AI to write a script, or place one there manually.");
        return Ok(());
    }
    let name_w = scripts.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mScripts\x1b[0m  ({})", crate::scripts::scripts_dir().display());
    println!();
    for s in &scripts {
        println!("  \x1b[1m\x1b[96m{:<width$}\x1b[0m  {} bytes", s.name, s.size, width = name_w);
    }
    println!();
    Ok(())
}

/// List scheduled jobs (reads schedules.json directly, no daemon needed).
pub fn run_sched_list() -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    let jobs = store.list();
    if jobs.is_empty() {
        println!("No scheduled jobs.");
        return Ok(());
    }
    let name_w = jobs.iter().map(|j| j.name.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mScheduled Jobs\x1b[0m");
    println!();
    println!("  {:<8}  {:<name_w$}  {:<16}  {:<12}  {}",
        "ID", "Name", "Schedule", "Status", "Next Run", name_w = name_w);
    println!("  {}  {}  {}  {}  {}",
        "─".repeat(8), "─".repeat(name_w), "─".repeat(16), "─".repeat(12), "─".repeat(24));
    for job in &jobs {
        let id_short = &job.id[..job.id.len().min(8)];
        let next = job.kind.next_run()
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".to_string());
        println!("  \x1b[96m{:<8}\x1b[0m  {:<name_w$}  {:<16}  {:<12}  {}",
            id_short, job.name, job.kind.describe(), job.status.describe(), next,
            name_w = name_w);
    }
    println!();
    Ok(())
}

/// Cancel a scheduled job by UUID prefix (reads/writes schedules.json directly).
pub fn run_sched_cancel(id: String) -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    // Support prefix matching
    let jobs = store.list();
    let matched: Vec<&crate::scheduler::ScheduledJob> = jobs.iter()
        .filter(|j| j.id.starts_with(&id))
        .collect();
    match matched.len() {
        0 => {
            eprintln!("No job found with ID starting with '{}'", id);
            std::process::exit(1);
        }
        1 => {
            let full_id = matched[0].id.clone();
            store.cancel(&full_id)?;
            println!("Cancelled job {} ({})", full_id, matched[0].name);
        }
        _ => {
            eprintln!("Ambiguous ID prefix '{}' — matches {} jobs. Use more characters.", id, matched.len());
            std::process::exit(1);
        }
    }
    Ok(())
}

/// List leftover de-* tmux windows (from failed scheduled jobs).
pub fn run_sched_windows() -> Result<()> {
    // Use tmux list-windows to find de-* windows
    let output = std::process::Command::new("tmux")
        .args(["list-windows", "-a", "-F", "#{session_name}:#{window_name}"])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let de_windows: Vec<&str> = text.lines()
                .filter(|l| {
                    let name = l.splitn(2, ':').nth(1).unwrap_or("");
                    name.starts_with("de-")
                })
                .collect();
            if de_windows.is_empty() {
                println!("No leftover de-* tmux windows found.");
            } else {
                println!("\x1b[1mLeftover scheduled job windows:\x1b[0m");
                println!();
                for w in &de_windows {
                    println!("  \x1b[96m{}\x1b[0m", w);
                }
                println!();
                println!("Kill a window:  tmux kill-window -t <session>:<window>");
            }
        }
        Err(e) => {
            eprintln!("Failed to list tmux windows: {}", e);
        }
    }
    Ok(())
}

pub async fn run_chat() -> Result<()> {
    let result = run_chat_inner().await;
    if let Err(ref e) = result {
        // AsyncStdin has been dropped by now; synchronous stdin is safe.
        use std::io::Write;
        eprintln!("\n\x1b[31m✗\x1b[0m daemoneye error: {}", e);
        eprint!("\x1b[2mPress Enter to close this pane…\x1b[0m");
        std::io::stderr().flush().ok();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    result
}

async fn run_chat_inner() -> Result<()> {
    let start_time = std::time::Instant::now();
    let mut session_id = new_session_id();
    // None = use daemon's configured default prompt; Some(name) = override.
    let mut current_prompt: Option<String> = None;
    let mut approval = SessionApproval::default();

    // Single AsyncStdin owns the fd 0 epoll registration for the whole session.
    // Shared by the interactive line editor (raw mode) and tool-call approval
    // prompts (cooked mode); they never run concurrently.
    let stdin = AsyncStdin::new()?;
    let mut input_state = InputState::new();

    // Register the SIGWINCH listener before doing anything that depends on
    // terminal size.  tokio queues signals from the moment the listener is
    // created, so no resize event can slip through the gap between process
    // start and our first poll.
    let mut sigwinch = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::window_change())?
    };

    // Initial pane dimensions — use the tmux query to set the 25%-width target
    // and read back the exact post-resize size.
    let pane_id_opt = std::env::var("TMUX_PANE").ok();
    let mut chat_width: usize;
    let mut chat_height: usize;
    if let Some(ref pane_id) = pane_id_opt {
        let target_w = crate::tmux::query_window_width(pane_id)
            .map(|w| (w * 25 / 100).max(20))
            .unwrap_or(100);
        let _ = crate::tmux::resize_pane_width(pane_id, target_w);
        chat_width  = crate::tmux::query_pane_width(pane_id).unwrap_or(target_w);
        chat_height = crate::tmux::query_pane_height(pane_id).unwrap_or_else(|_| terminal_height());
    } else {
        chat_width  = terminal_width();
        chat_height = terminal_height();
    }

    // When running inside tmux a new split pane triggers one or more SIGWINCH
    // signals as the layout is negotiated.  Wait here until no SIGWINCH has
    // arrived for SETTLE_MS milliseconds so we know the final dimensions before
    // printing anything.  Re-query on every signal so we always end up with
    // the correct settled size.
    if pane_id_opt.is_some() {
        const SETTLE_MS: u64 = 500;
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(SETTLE_MS),
                sigwinch.recv(),
            ).await {
                Ok(_) => {
                    // Another resize — update dims and restart the quiet timer.
                    chat_width  = terminal_width();
                    chat_height = terminal_height();
                }
                Err(_elapsed) => break, // stable for SETTLE_MS — proceed
            }
        }
    }

    // Install the scroll region.  The input frame and status bar are
    // intentionally NOT drawn yet — the greeting streams next and the
    // dimensions may still shift.  Drawing the frame now would show it in
    // the wrong place or have it visually overwritten by the greeting content.
    setup_scroll_region(chat_height);

    // ASCII logo — centered using the settled chat_width.
    {
        let logo_lines = [
            "████▄   ▄▄▄  ▄▄▄▄▄ ▄▄   ▄▄  ▄▄▄  ▄▄  ▄▄ ██████ ▄▄ ▄▄ ▄▄▄▄▄",
            "██  ██ ██▀██ ██▄▄  ██▀▄▀██ ██▀██ ███▄██ ██▄▄   ▀███▀ ██▄▄",
            "████▀  ██▀██ ██▄▄▄ ██   ██ ▀███▀ ██ ▀██ ██▄▄▄▄   █   ██▄▄▄",
        ];
        let subtitle = "                 AI POWERED OPERATOR";
        let logo_w = logo_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let pad = " ".repeat((chat_width.saturating_sub(logo_w)) / 2);
        println!();
        for line in &logo_lines {
            println!("{pad}\x1b[1m\x1b[96m{line}\x1b[0m");
        }
        println!("{pad}\x1b[2m{subtitle}\x1b[0m");
    }

    // One-time usage hints — stacked vertically, centered in the pane.
    {
        let center = |vis_len: usize| -> String {
            " ".repeat((chat_width.saturating_sub(vis_len)) / 2)
        };
        println!();
        // visible lengths (no ANSI): 22, 23, 26, 30
        println!("{}\x1b[93mexit\x1b[0m or \x1b[93mCtrl-C\x1b[0m to quit",           center(22));
        println!("{}\x1b[96m/clear\x1b[0m to reset session",                           center(23));
        println!("{}\x1b[96m/refresh\x1b[0m to resync context",                        center(26));
        println!("{}\x1b[2mcontext: panes · windows · env\x1b[0m",                    center(30));
        println!();
    }

    // Hold off on the AI greeting until a tmux client is attached to this
    // session.  When the daemon auto-opens the chat pane in a freshly-created
    // (detached) session, nobody is watching yet; firing the greeting
    // immediately would waste an API call and surface a stale response when
    // the user eventually attaches.
    //
    // In the normal keybinding workflow (user already inside an active tmux
    // session), #{session_attached} is already ≥ 1 so the loop exits on the
    // first check with no perceptible delay.
    let mut current_status = "ready";
    draw_status_bar(chat_height, chat_width, &session_id, current_status);

    loop {
        let attached = std::process::Command::new("tmux")
            .args(["display-message", "-p", "#{session_attached}"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1); // treat errors as attached (e.g. running outside tmux)
        if attached > 0 { break; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // A client is now attached — switch to "thinking…" and send the greeting.
    current_status = "thinking…";
    draw_status_bar(chat_height, chat_width, &session_id, current_status);

    if let Err(e) = ask_with_session("Hello!".to_string(), Some(&session_id), current_prompt.as_deref(), &stdin, Some(chat_width), &mut approval).await {
        eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
        eprintln!("  Make sure it is running:  \x1b[1mdaemoneye daemon --console\x1b[0m");
        eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
    }

    // Greeting is done.  Re-query dimensions in case the pane was resized
    // while it streamed, then draw the full chrome for the first time.
    chat_width  = terminal_width();
    chat_height = terminal_height();
    setup_scroll_region(chat_height);
    current_status = "ready";
    draw_input_frame(chat_height, chat_width, start_time);
    draw_status_bar(chat_height, chat_width, &session_id, current_status);

    loop {
        // read_input_line handles its own rendering and SIGWINCH internally.
        let line_opt = read_input_line(
            &mut input_state, &stdin, &mut sigwinch,
            &mut chat_width, &mut chat_height,
            start_time, &session_id, current_status,
        ).await?;

        let Some(line) = line_opt else { break }; // EOF or Ctrl+D on empty line

        // Clear the input row and anchor to the scroll region's bottom so
        // all subsequent output scrolls upward.
        {
            use std::io::Write;
            let input_row     = chat_height.saturating_sub(2).max(1);
            let scroll_bottom = chat_height.saturating_sub(4).max(1);
            print!("\x1b[{input_row};1H\x1b[2K");
            print!("\x1b[{scroll_bottom};1H");
            std::io::stdout().flush()?;
        }

        let query = line.trim().to_string();
        if query.is_empty() { continue; }

        // Push to history before processing so /clear etc. are also navigable.
        input_state.push_history(query.clone());

        if query == "exit" || query == "quit" { break; }
        if query == "/clear" {
            session_id = new_session_id();
            approval = SessionApproval::default();
            current_prompt = None;
            let label = format!(" session cleared · new session:{} ", &session_id[..8]);
            let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
            println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
            current_status = "ready";
            draw_input_frame(chat_height, chat_width, start_time);
            draw_status_bar(chat_height, chat_width, &session_id, current_status);
            continue;
        }
        if let Some(name) = query.strip_prefix("/prompt ").map(str::trim) {
            let name = name.to_string();
            let path = crate::config::prompts_dir().join(format!("{}.toml", name));
            if !path.exists() && name != "sre" {
                println!("\x1b[31m✗\x1b[0m  Unknown prompt \x1b[1m{}\x1b[0m — run \x1b[1mdaemoneye prompts\x1b[0m to list available prompts.", name);
            } else {
                session_id = new_session_id();
                approval = SessionApproval::default();
                current_prompt = Some(name.clone());
                let label = format!(" prompt: {}  ·  new session:{} ", name, &session_id[..8]);
                let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                current_status = "ready";
                draw_input_frame(chat_height, chat_width, start_time);
                draw_status_bar(chat_height, chat_width, &session_id, current_status);
            }
            continue;
        }
        if query == "/refresh" {
            match send_refresh().await {
                Ok(()) => {
                    session_id = new_session_id();
                    approval = SessionApproval::default();
                    let label = format!(" context refreshed  ·  new session:{} ", &session_id[..8]);
                    let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                    current_status = "ready";
                    draw_input_frame(chat_height, chat_width, start_time);
                    draw_status_bar(chat_height, chat_width, &session_id, current_status);
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  Refresh failed: {}", e),
            }
            continue;
        }
        // Echo the query at the bottom of the scroll region.
        println!("\x1b[92m❯\x1b[0m {}", query);
        current_status = "thinking…";
        draw_status_bar(chat_height, chat_width, &session_id, current_status);
        if let Err(e) = ask_with_session(query, Some(&session_id), current_prompt.as_deref(), &stdin, Some(chat_width), &mut approval).await {
            eprintln!("\n\x1b[31m✗\x1b[0m {}", e);
        }
        // Re-sync dimensions after the (potentially long) streaming response.
        chat_width  = terminal_width();
        chat_height = terminal_height();
        setup_scroll_region(chat_height);
        current_status = "ready";
        draw_input_frame(chat_height, chat_width, start_time);
        draw_status_bar(chat_height, chat_width, &session_id, current_status);
    }

    teardown_scroll_region(chat_height);
    println!("\n\x1b[2mGoodbye.\x1b[0m");
    Ok(())
}

/// Render a bright-cyan bordered panel at terminal width.
///
/// `title`    — label embedded in the top border
/// `body`     — lines of text to show inside; long lines are truncated with `…`
/// `dim_body` — if true the body text is rendered dim (for captured output)
fn print_tool_panel(title: &str, body: &[&str], dim_body: bool) {
    let w     = terminal_width().max(44);
    let inner = w - 2; // visible chars between corner glyphs

    // ── Top border: ╭─ title ────────────────────────────╮ ─────────────
    let tpart = format!("─ {} ", title);
    let fill  = inner.saturating_sub(visual_len(&tpart) + 1); // +1 for the ─ before ╮
    println!("\x1b[1m\x1b[96m╭{tpart}{}─╮\x1b[0m", "─".repeat(fill));

    // ── Body lines ──────────────────────────────────────────────────────
    let avail = inner.saturating_sub(2); // 2 for the "  " indent
    for line in body {
        let vis = visual_len(line);
        let (text, text_vis) = if vis > avail {
            // Truncate and append ellipsis.
            let t: String = line.chars().take(avail.saturating_sub(1)).collect();
            (t + "…", avail)
        } else {
            (line.to_string(), vis)
        };
        let pad = " ".repeat(inner.saturating_sub(2 + text_vis));
        if dim_body {
            println!("\x1b[1m\x1b[96m│\x1b[0m  \x1b[2m{text}\x1b[0m{pad}\x1b[1m\x1b[96m│\x1b[0m");
        } else {
            println!("\x1b[1m\x1b[96m│\x1b[0m  {text}{pad}\x1b[1m\x1b[96m│\x1b[0m");
        }
    }

    // ── Bottom border: ╰──────────────────────────────────╯ ─────────────
    println!("\x1b[1m\x1b[96m╰{}\x1b[22m╯\x1b[0m", "─".repeat(inner));
}

async fn ask_with_session(query: String, session_id: Option<&str>, prompt_override: Option<&str>, stdin: &AsyncStdin, chat_width: Option<usize>, approval: &mut SessionApproval) -> Result<()> {
    use std::io::Write;
    use std::time::Duration;

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    // DAEMONEYE_SOURCE_PANE is set by the recommended tmux bind-key:
    //   split-window -h -e "DAEMONEYE_SOURCE_PANE=#{pane_id}" 'daemoneye chat'
    // It records the user's working pane before the split so the daemon
    // captures context from — and injects commands into — the right pane.
    // Falls back to TMUX_PANE, which is correct when `daemoneye chat` or
    // `daemoneye ask` is run directly from the user's working pane.
    let tmux_pane = std::env::var("DAEMONEYE_SOURCE_PANE")
        .ok()
        .or_else(|| std::env::var("TMUX_PANE").ok());
    // The chat pane is this process's own pane ($TMUX_PANE).  The daemon uses
    // it to switch focus back to the AI interface after a foreground sudo
    // command hands control to the user's working pane.
    let chat_pane = std::env::var("TMUX_PANE").ok();
    send_request(&mut tx, Request::Ask {
        query,
        tmux_pane,
        session_id: session_id.map(|s| s.to_string()),
        chat_pane,
        prompt: prompt_override.map(|s| s.to_string()),
        chat_width,
    }).await?;

    // Braille-pattern spinner frames, updated every 80 ms while waiting for
    // the first response from the daemon.
    const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut spin = 0usize;
    let mut response_started = false;

    // Markdown renderer — parses inline markdown and block-level elements,
    // applies ANSI styling, and word-wraps prose at the current terminal width.
    // Shared across the whole response (including tool-call sub-turns) so that
    // column position and code-block state remain consistent throughout.
    let mut md = MarkdownRenderer::new();

    loop {
        // Phase 1 — waiting for the first content: poll recv() with a short
        // timeout so we can animate the spinner between each check.
        let msg = if !response_started {
            loop {
                match tokio::time::timeout(Duration::from_millis(80), recv(&mut rx)).await {
                    Err(_timeout) => {
                        print!("\r\x1b[36m{}\x1b[0m \x1b[2mThinking…\x1b[0m", SPINNER[spin]);
                        std::io::stdout().flush()?;
                        spin = (spin + 1) % SPINNER.len();
                    }
                    Ok(r) => break r?,
                }
            }
        } else {
            // Phase 2 — streaming: wait with a 60 s inter-token deadline so a
            // daemon that stops responding mid-stream produces a clear error.
            tokio::time::timeout(
                Duration::from_secs(60),
                recv(&mut rx),
            )
            .await
            .context("Daemon stopped responding (60 s inter-token timeout)")??
        };

        match msg {
            Response::Ok => {
                md.flush();
                print!("\x1b[0m"); // reset prose tint
                println!();
                break;
            }
            Response::Error(e) => {
                if !response_started {
                    print!("\r\x1b[K"); // erase spinner line
                }
                md.flush();
                eprintln!("\n\x1b[31m✗\x1b[0m {}", e);
                break;
            }
            Response::SessionInfo { message_count } => {
                // Print a subtle turn/context indicator, then let the spinner resume.
                let turn = (message_count / 2) + 1; // each turn = 1 user + 1 assistant msg
                let ctx_label = if message_count == 0 {
                    "new session".to_string()
                } else {
                    format!("{} message{} in context",
                        message_count,
                        if message_count == 1 { "" } else { "s" })
                };
                let w = terminal_width();
                let label = format!(" turn {} · {} ", turn, ctx_label);
                let dashes = w.min(72).saturating_sub(visual_len(&label) + 1);
                print!("\r\x1b[K"); // erase spinner
                println!("\x1b[2m─{}{}\x1b[0m",
                    label,
                    "─".repeat(dashes));
            }
            Response::Token(t) => {
                if !response_started {
                    print!("\r\x1b[K"); // erase spinner line
                    response_started = true;
                }
                md.feed(&t);
                std::io::stdout().flush()?;
            }
            Response::ToolCallPrompt { id, command, background } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!(); // blank line before panel
                let where_label = if background {
                    "daemon · runs silently"
                } else {
                    "terminal · visible to you"
                };
                let cmd_line = format!("$ {}", command);
                print_tool_panel(where_label, &[&cmd_line], false);

                let is_sudo = command_is_sudo(&command);
                let auto_approved = if is_sudo { approval.sudo } else { approval.regular };

                let approved = if auto_approved {
                    println!("  \x1b[32m✓\x1b[0m \x1b[2mauto-approved (session)\x1b[0m");
                    true
                } else {
                    let session_label = if is_sudo { "sudo session" } else { "session" };
                    print!(
                        "  \x1b[32mApprove?\x1b[0m \
                         [\x1b[1;92mY\x1b[0m]es  \
                         [\x1b[1;91mN\x1b[0m]o  \
                         [\x1b[1;93mA\x1b[0m]pprove for {session_label} \
                         \x1b[32m›\x1b[0m "
                    );
                    std::io::stdout().flush()?;
                    let input = stdin.read_line().await.unwrap_or_default();
                    let trimmed = input.trim();
                    let approve_session = trimmed.eq_ignore_ascii_case("a");
                    let approved_once = trimmed.eq_ignore_ascii_case("y") || approve_session;

                    if approve_session {
                        if is_sudo { approval.sudo = true; } else { approval.regular = true; }
                        println!("  \x1b[32m✓ approved — all {} commands auto-approved for this session\x1b[0m",
                                 if is_sudo { "sudo" } else { "regular" });
                    } else if approved_once {
                        println!("  \x1b[32m✓ approved\x1b[0m");
                    } else {
                        println!("  \x1b[2m✗ skipped\x1b[0m");
                    }
                    approved_once
                };

                md.reset();
                send_request(&mut tx, Request::ToolCallResponse { id, approved }).await?;
            }
            Response::SystemMsg(msg) => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!("\x1b[33m⚙\x1b[0m  \x1b[33m{}\x1b[0m", msg);
                md.reset();
            }
            Response::ToolResult(output) => {
                md.flush();
                const MAX_RESULT_LINES: usize = 10;
                let all_lines: Vec<&str> = output.lines().collect();
                let total = all_lines.len();
                // When overflow occurs the indicator itself occupies one row,
                // so only MAX_RESULT_LINES-1 content lines fit within the cap.
                let content_rows = if total > MAX_RESULT_LINES {
                    MAX_RESULT_LINES - 1
                } else {
                    total
                };
                let mut body: Vec<String> = all_lines[..content_rows]
                    .iter().map(|s| s.to_string()).collect();
                if total > MAX_RESULT_LINES {
                    body.push(format!("… {} more lines", total - content_rows));
                }
                if body.is_empty() {
                    body.push("(no output)".to_string());
                }
                let body_refs: Vec<&str> = body.iter().map(|s| s.as_str()).collect();
                print_tool_panel("output", &body_refs, true);
                md.reset();
            }
            Response::CredentialPrompt { id, prompt } => {
                md.flush();
                println!("\n\x1b[33m⚠\x1b[0m  \x1b[1m{}\x1b[0m", prompt);
                let credential = read_password_silent("   \x1b[33mPassword:\x1b[0m ").unwrap_or_default();
                md.reset();
                send_request(&mut tx, Request::CredentialResponse { id, credential }).await?;
            }
            Response::PaneSelectPrompt { id, panes } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mWhich pane should receive this command?\x1b[0m");
                println!();
                for (i, pane) in panes.iter().enumerate() {
                    println!("  \x1b[32m[{}]\x1b[0m  {} — {} — {}",
                        i + 1, pane.id, pane.current_cmd, pane.summary);
                }
                println!();
                print!("  Select pane \x1b[32m›\x1b[0m ");
                std::io::stdout().flush()?;
                let input = stdin.read_line().await.unwrap_or_default();
                let pane_id = input.trim().parse::<usize>()
                    .ok()
                    .and_then(|n| panes.get(n.saturating_sub(1)))
                    .map(|p| p.id.clone())
                    .unwrap_or_else(|| panes.first().map(|p| p.id.clone()).unwrap_or_default());
                md.reset();
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptWritePrompt { id, script_name, content } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write script:\x1b[0m \x1b[96m{}\x1b[0m", script_name);
                println!();
                // Show up to 40 lines of the script content
                let lines: Vec<&str> = content.lines().collect();
                let show = lines.len().min(40);
                for line in &lines[..show] {
                    println!("  \x1b[2m{}\x1b[0m", line);
                }
                if lines.len() > 40 {
                    println!("  \x1b[2m… ({} more lines)\x1b[0m", lines.len() - 40);
                }
                println!();
                print!("  Approve writing to ~/.daemoneye/scripts/{}? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ", script_name);
                std::io::stdout().flush()?;
                let input = stdin.read_line().await.unwrap_or_default();
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::ScriptWriteResponse { id, approved }).await?;
            }
            Response::ScheduleList { jobs } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                if jobs.is_empty() {
                    println!("  No scheduled jobs.");
                } else {
                    println!("  \x1b[1mScheduled Jobs\x1b[0m");
                    println!();
                    let id_w = jobs.iter().map(|j| j.id.len().min(8)).max().unwrap_or(8);
                    let name_w = jobs.iter().map(|j| j.name.len()).max().unwrap_or(4).max(4);
                    let kind_w = jobs.iter().map(|j| j.kind.len()).max().unwrap_or(8).max(8);
                    println!("  {:<id_w$}  {:<name_w$}  {:<kind_w$}  {:<12}  {}",
                        "ID", "Name", "Schedule", "Status", "Next Run",
                        id_w = id_w, name_w = name_w, kind_w = kind_w);
                    println!("  {}  {}  {}  {}  {}",
                        "─".repeat(id_w), "─".repeat(name_w), "─".repeat(kind_w),
                        "─".repeat(12), "─".repeat(24));
                    for job in &jobs {
                        let id_short = &job.id[..job.id.len().min(8)];
                        let next = job.next_run.as_deref().unwrap_or("—");
                        println!("  \x1b[96m{:<id_w$}\x1b[0m  {:<name_w$}  {:<kind_w$}  {:<12}  {}",
                            id_short, job.name, job.kind, job.status, next,
                            id_w = id_w, name_w = name_w, kind_w = kind_w);
                    }
                }
                println!();
                md.reset();
            }
            Response::ScriptList { scripts } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                if scripts.is_empty() {
                    println!("  No scripts in ~/.daemoneye/scripts/");
                } else {
                    println!("  \x1b[1mScripts\x1b[0m  (~/.daemoneye/scripts/)");
                    println!();
                    let name_w = scripts.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
                    for s in &scripts {
                        println!("  \x1b[96m{:<name_w$}\x1b[0m  {} bytes", s.name, s.size, name_w = name_w);
                    }
                }
                println!();
                md.reset();
            }
        }
    }

    Ok(())
}

/// Read a password from stdin with terminal echo disabled so it is not shown.
fn read_password_silent(prompt: &str) -> anyhow::Result<String> {
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

/// Count the visible (printable) characters in a string, skipping ANSI escape
/// sequences.  Used to measure word width correctly when the pending word
/// contains bold or colour codes injected by the markdown renderer.
fn visual_len(s: &str) -> usize {
    let mut count = 0usize;
    let mut in_esc = false;
    for ch in s.chars() {
        if in_esc {
            if ch.is_ascii_alphabetic() { in_esc = false; }
        } else if ch == '\x1b' {
            in_esc = true;
        } else {
            count += 1;
        }
    }
    count
}

/// Query the visible column width of the terminal on stdout.
/// Uses `ioctl(TIOCGWINSZ)` so the value is always live — pane resizes are
/// reflected automatically.  Falls back to `$COLUMNS`, then to 79.
fn terminal_width() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 1
        {
            // Leave a 1-char right margin so text never touches the very edge.
            return (ws.ws_col as usize) - 1;
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|w| w.saturating_sub(1))
        .unwrap_or(79)
}

/// Query the visible row height of the terminal on stdout.
/// Uses `ioctl(TIOCGWINSZ)` so the value is live; falls back to `$LINES` then 24.
fn terminal_height() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_row > 2
        {
            return ws.ws_row as usize;
        }
    }
    std::env::var("LINES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(24)
}

/// Install a terminal scroll region that reserves the bottom four rows:
///   rows 1..(height-4) — scrolling content area
///   row (height-3) — input box top border (app name + uptime)
///   row (height-2) — input prompt
///   row (height-1) — input box bottom border
///   row  height    — status bar
fn setup_scroll_region(height: usize) {
    use std::io::Write;
    let scroll_bottom = height.saturating_sub(4).max(1);
    // DECSTBM — set scrolling region (1-indexed).
    print!("\x1b[1;{scroll_bottom}r");
    // Position cursor at the bottom of the scroll region so the first output
    // starts at the correct row.
    print!("\x1b[{scroll_bottom};1H");
    std::io::stdout().flush().ok();
}

/// Reset the terminal to full-screen scrolling and clear the four reserved rows.
fn teardown_scroll_region(height: usize) {
    use std::io::Write;
    // \x1b[r resets the scroll region to the full screen.
    print!("\x1b[r");
    for row in [
        height.saturating_sub(3).max(1),
        height.saturating_sub(2).max(1),
        height.saturating_sub(1).max(1),
        height,
    ] {
        print!("\x1b[{row};1H\x1b[2K");
    }
    // Leave cursor near the bottom of the now-full-screen terminal.
    print!("\x1b[{};1H", height.saturating_sub(4).max(1));
    std::io::stdout().flush().ok();
}

/// Format an elapsed duration as a compact uptime string.
fn fmt_uptime(elapsed: std::time::Duration) -> String {
    let s = elapsed.as_secs();
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

/// Draw (or redraw) the input box borders.
///
/// The top border carries the app name and current uptime; the bottom border
/// is plain.  Uses DEC save/restore cursor so it is safe to call at any point
/// without disturbing the scroll-region cursor position.
///   row (height-3): ╭─ DaemonEye ─────────────────────── up 4m 12s ─╮
///   row (height-1): ╰────────────────────────────────────────────╯
fn draw_input_frame(height: usize, width: usize, start: std::time::Instant) {
    use std::io::Write;
    let border_top    = height.saturating_sub(3).max(1);
    let border_bottom = height.saturating_sub(1).max(1);
    let inner = width.saturating_sub(2);

    let title_left  = "─ DaemonEye ───────────";
    let title_right = format!(" up {} ─", fmt_uptime(start.elapsed()));
    let anchors     = visual_len(title_left) + visual_len(&title_right);
    let top = if inner >= anchors {
        let mid = "─".repeat(inner - anchors);
        format!("\x1b[1m\x1b[96m╭{title_left}{mid}\x1b[2m{title_right}\x1b[22m╮\x1b[0m")
    } else {
        let dashes = "─".repeat(inner.saturating_sub(visual_len(title_left)));
        format!("\x1b[1m\x1b[96m╭{title_left}{dashes}╮\x1b[0m")
    };

    print!("\x1b7");
    print!("\x1b[{border_top};1H\x1b[2K{top}");
    print!("\x1b[{border_bottom};1H\x1b[2K\x1b[1m\x1b[96m╰{}╯\x1b[0m", "─".repeat(inner));
    print!("\x1b8");
    std::io::stdout().flush().ok();
}

/// Render (or refresh) the status bar in the bottom row.
/// Uses DEC save/restore cursor (\x1b7 / \x1b8) so this is safe to call
/// at any point without disturbing the scroll-region cursor position.
fn draw_status_bar(height: usize, width: usize, session_id: &str, status: &str) {
    use std::io::Write;
    let left = format!(
        " ⬡ daemoneye  ·  session:{}  ·  {}",
        &session_id[..8.min(session_id.len())],
        status,
    );
    let vis = visual_len(&left);
    let pad = " ".repeat(width.saturating_sub(vis));
    print!("\x1b7");                    // DEC save cursor
    print!("\x1b[{height};1H");        // move to status bar row
    print!("\x1b[2m{}{}\x1b[0m", left, pad);
    print!("\x1b8");                    // DEC restore cursor
    std::io::stdout().flush().ok();
}

/// Streaming word-wrap writer.
///
/// Characters are accumulated in `pending` until a word boundary (space or
/// newline) is reached.  At that point the buffered word is either appended to
/// the current line (with a leading space if needed) or wrapped to the next
/// line.  Terminal width is sampled on every word boundary, so output adapts
/// automatically when the user resizes the pane while a response streams.
struct WrapWriter {
    /// Current visual column (number of chars printed since the last newline).
    col: usize,
    /// Characters accumulated since the last word boundary.
    pending: String,
    /// A space was consumed after the last word; it becomes a leading space
    /// before the next word (or is dropped when we wrap).
    space_before: bool,
    /// When true, prefix each emitted word with the prose tint color so that
    /// AI prose is visually distinct from other terminal output.
    tint: bool,
}

impl WrapWriter {
    fn new() -> Self {
        Self { col: 0, pending: String::new(), space_before: false, tint: false }
    }

    /// Feed a streaming token into the writer.
    fn feed(&mut self, token: &str) {
        for ch in token.chars() {
            match ch {
                '\n' => {
                    self.emit_word();
                    print!("\n");
                    self.col = 0;
                    self.space_before = false;
                }
                '\r' => {} // ignore bare carriage returns in AI output
                ' ' | '\t' => {
                    if !self.pending.is_empty() {
                        self.emit_word();
                        self.space_before = true;
                    } else if self.col > 0 {
                        self.space_before = true;
                    }
                }
                _ => self.pending.push(ch),
            }
        }
    }

    /// Flush any buffered word to stdout without resetting the column counter.
    /// Call this before printing your own output to ensure the pending word
    /// is visible first.
    fn flush(&mut self) {
        self.emit_word();
        self.space_before = false;
    }

    /// Flush any buffered word AND reset the column counter to zero.
    /// Call this after printing your own newline-terminated output so the
    /// writer knows the cursor is back at column zero.
    fn reset(&mut self) {
        self.emit_word();
        self.col = 0;
        self.space_before = false;
    }

    /// Directly set the column counter after printing a leader (bullet symbol,
    /// list number, blockquote bar, etc.) that bypasses the writer.
    fn set_col(&mut self, col: usize) {
        self.col = col;
    }

    /// Emit the pending word, wrapping first if it would overflow the line.
    fn emit_word(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        // Use visual length (strips ANSI codes) so bold/coloured words don't
        // appear wider than they actually are on screen.
        let word_len = visual_len(&self.pending);
        let w = terminal_width();
        // Soft-white tint wraps each word; the word's own ANSI codes (bold,
        // inline code colour, etc.) take precedence, then \x1b[0m resets
        // everything — the tint is re-applied on the next word.
        let (tint_on, tint_off) = if self.tint {
            ("\x1b[97m", "\x1b[0m")
        } else {
            ("", "")
        };
        if self.col == 0 {
            print!("{}{}{}", tint_on, self.pending, tint_off);
            self.col = word_len;
        } else if self.col + 1 + word_len <= w {
            let prefix = if self.space_before { " " } else { "" };
            print!("{}{}{}{}", prefix, tint_on, self.pending, tint_off);
            self.col += prefix.len() + word_len;
        } else {
            print!("\n{}{}{}", tint_on, self.pending, tint_off);
            self.col = word_len;
        }
        self.space_before = false;
        self.pending.clear();
    }
}

/// Convert inline markdown syntax in `input` to ANSI escape sequences.
/// Handles: `backtick code` (yellow), **bold**, *italic*.
/// Single underscores inside words are left as-is to avoid false positives
/// with filenames and identifiers.
fn render_inline(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 32);
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut in_bold   = false;
    let mut in_italic = false;
    let mut in_code   = false;

    while i < n {
        if in_code {
            if chars[i] == '`' {
                out.push_str("\x1b[0m");
                in_code = false;
            } else {
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }

        match chars[i] {
            '`' => {
                out.push_str("\x1b[33m"); // yellow for inline code
                in_code = true;
                i += 1;
            }
            '*' if i + 1 < n && chars[i + 1] == '*' => {
                if in_bold {
                    out.push_str("\x1b[22m");
                    in_bold = false;
                } else {
                    out.push_str("\x1b[1m");
                    in_bold = true;
                }
                i += 2;
            }
            '*' => {
                // Open italic only at a word boundary (preceded by space or
                // start-of-string and followed by a non-space character).
                let at_start    = i == 0 || chars[i - 1] == ' ';
                let next_is_txt = i + 1 < n && chars[i + 1] != ' ';
                if in_italic {
                    out.push_str("\x1b[23m");
                    in_italic = false;
                } else if at_start && next_is_txt {
                    out.push_str("\x1b[3m");
                    in_italic = true;
                } else {
                    out.push('*');
                }
                i += 1;
            }
            c => { out.push(c); i += 1; }
        }
    }

    if in_bold || in_italic || in_code {
        out.push_str("\x1b[0m");
    }
    out
}

// ── Syntax highlighting ──────────────────────────────────────────────────────

#[derive(Copy, Clone)]
enum CommentStyle {
    Hash,         // #  (bash, python, yaml, ruby, dockerfile)
    DoubleSlash,  // // (rust, js, go, java, c, c++)
    DoubleDash,   // -- (sql, lua, haskell)
    Semicolon,    // ;  (lisp, asm)
    None,
}

fn lang_keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "bash" | "sh" | "shell" | "zsh" | "fish" => &[
            "if", "then", "else", "elif", "fi", "for", "in", "do", "done",
            "while", "until", "case", "esac", "function", "return", "local",
            "export", "readonly", "declare", "unset", "source", "echo", "printf",
            "cd", "exit", "break", "continue", "shift", "set", "unsetopt",
        ],
        "python" | "py" => &[
            "False", "None", "True", "and", "as", "assert", "async", "await",
            "break", "class", "continue", "def", "del", "elif", "else", "except",
            "finally", "for", "from", "global", "if", "import", "in", "is",
            "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
            "while", "with", "yield",
        ],
        "rust" | "rs" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn",
            "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
            "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
            "self", "Self", "static", "struct", "super", "trait", "true", "type",
            "union", "unsafe", "use", "where", "while",
        ],
        "javascript" | "js" | "typescript" | "ts" | "jsx" | "tsx" => &[
            "break", "case", "catch", "class", "const", "continue", "debugger",
            "default", "delete", "do", "else", "export", "extends", "false",
            "finally", "for", "function", "if", "import", "in", "instanceof",
            "let", "new", "null", "return", "static", "super", "switch", "this",
            "throw", "true", "try", "typeof", "undefined", "var", "void", "while",
            "with", "yield", "async", "await", "of", "from", "type", "interface",
            "enum", "implements", "readonly",
        ],
        "go" | "golang" => &[
            "break", "case", "chan", "const", "continue", "default", "defer",
            "else", "fallthrough", "for", "func", "go", "goto", "if", "import",
            "interface", "map", "package", "range", "return", "select", "struct",
            "switch", "type", "var", "true", "false", "nil",
        ],
        "java" => &[
            "abstract", "assert", "boolean", "break", "byte", "case", "catch",
            "char", "class", "const", "continue", "default", "do", "double",
            "else", "enum", "extends", "false", "final", "finally", "float",
            "for", "goto", "if", "implements", "import", "instanceof", "int",
            "interface", "long", "native", "new", "null", "package", "private",
            "protected", "public", "return", "short", "static", "strictfp",
            "super", "switch", "synchronized", "this", "throw", "throws",
            "transient", "true", "try", "void", "volatile", "while",
        ],
        "sql" => &[
            "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "INSERT", "INTO",
            "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "DROP",
            "ALTER", "ADD", "COLUMN", "INDEX", "PRIMARY", "KEY", "FOREIGN",
            "REFERENCES", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON",
            "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "DISTINCT",
            "AS", "IN", "IS", "NULL", "NOT", "EXISTS", "UNION", "ALL",
            "CASE", "WHEN", "THEN", "ELSE", "END", "WITH", "RETURNING",
            "CONSTRAINT", "UNIQUE", "DEFAULT", "AUTO_INCREMENT", "SERIAL",
        ],
        _ => &[],
    }
}

fn lang_comment_style(lang: &str) -> CommentStyle {
    match lang {
        "bash" | "sh" | "shell" | "zsh" | "fish"
        | "python" | "py"
        | "ruby" | "rb"
        | "yaml" | "yml"
        | "toml"
        | "dockerfile" | "docker" => CommentStyle::Hash,

        "rust" | "rs"
        | "javascript" | "js" | "typescript" | "ts" | "jsx" | "tsx"
        | "go" | "golang"
        | "java"
        | "c" | "cpp" | "c++" | "cc" | "h" | "hpp"
        | "css" | "scss" | "sass"
        | "swift" | "kotlin" | "scala" => CommentStyle::DoubleSlash,

        "sql" | "lua" | "haskell" | "hs" => CommentStyle::DoubleDash,

        "lisp" | "scheme" | "clojure" | "asm" | "nasm" => CommentStyle::Semicolon,

        _ => CommentStyle::None,
    }
}

/// Colorize a single word if it matches the keyword list.
fn emit_word_token(out: &mut String, word: &str, keywords: &[&str], is_sql: bool) {
    if word.is_empty() {
        return;
    }
    let matched = if is_sql {
        keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
    } else {
        keywords.contains(&word)
    };
    if matched {
        out.push_str("\x1b[1m\x1b[94m"); // bold bright-blue
        out.push_str(word);
        out.push_str("\x1b[0m");
    } else {
        out.push_str(word);
    }
}

/// Apply syntax highlighting to a single code line.
///
/// For known languages, scans character-by-character tracking string and
/// comment state.  For unknown or missing languages, falls back to plain cyan.
fn highlight_code(line: &str, lang: Option<&str>) -> String {
    let lang_lower = lang.map(|l| l.to_lowercase());
    let lang_str = lang_lower.as_deref().unwrap_or("");
    let keywords = lang_keywords(lang_str);
    let comment_style = lang_comment_style(lang_str);
    let is_sql = matches!(lang_str, "sql");

    // Unknown / plain language: just emit in cyan.
    if keywords.is_empty() && matches!(comment_style, CommentStyle::None) {
        return format!("\x1b[36m{}\x1b[0m", line);
    }

    let mut out = String::with_capacity(line.len() * 2);
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    // Detect single-line comments that start at column 0 or after whitespace.
    // We check for comment prefix at the start of each "token" boundary.
    let comment_prefix: Option<&str> = match comment_style {
        CommentStyle::Hash        => Some("#"),
        CommentStyle::DoubleSlash => Some("//"),
        CommentStyle::DoubleDash  => Some("--"),
        CommentStyle::Semicolon   => Some(";"),
        CommentStyle::None        => None,
    };

    // String quote char currently open (None = not in a string).
    let mut in_string: Option<char> = None;
    // Current non-string word accumulator.
    let mut word = String::new();

    macro_rules! flush_word {
        () => {
            if !word.is_empty() {
                let w = std::mem::take(&mut word);
                emit_word_token(&mut out, &w, keywords, is_sql);
            }
        };
    }

    while i < len {
        // ── Inside a string literal ──────────────────────────────────────
        if let Some(q) = in_string {
            out.push(chars[i]);
            if chars[i] == '\\' && i + 1 < len {
                i += 1;
                out.push(chars[i]);
            } else if chars[i] == q {
                out.push_str("\x1b[0m");
                in_string = None;
            }
            i += 1;
            continue;
        }

        // ── Check for comment start ──────────────────────────────────────
        if let Some(prefix) = comment_prefix {
            let remaining: String = chars[i..].iter().collect();
            if remaining.starts_with(prefix) {
                flush_word!();
                out.push_str("\x1b[2m\x1b[3m"); // dim italic
                // Emit the rest of the line as comment
                for &c in &chars[i..] { out.push(c); }
                out.push_str("\x1b[0m");
                return out;
            }
        }

        // ── String open ─────────────────────────────────────────────────
        if chars[i] == '"' || chars[i] == '\'' {
            flush_word!();
            let q = chars[i];
            out.push_str("\x1b[32m"); // green
            out.push(q);
            in_string = Some(q);
            i += 1;
            continue;
        }

        // ── Word boundary (identifier / keyword chars) ───────────────────
        if chars[i].is_alphanumeric() || chars[i] == '_' {
            word.push(chars[i]);
            i += 1;
            continue;
        }

        // ── Number literal ───────────────────────────────────────────────
        if word.is_empty() && chars[i].is_ascii_digit() {
            // Collect the whole number token
            let mut num = String::new();
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                num.push(chars[i]);
                i += 1;
            }
            out.push_str("\x1b[33m"); // yellow
            out.push_str(&num);
            out.push_str("\x1b[0m");
            continue;
        }

        // ── Non-word, non-string, non-comment punctuation / space ────────
        flush_word!();
        out.push(chars[i]);
        i += 1;
    }

    flush_word!();

    // Close any unclosed string (shouldn't happen for well-formed code)
    if in_string.is_some() {
        out.push_str("\x1b[0m");
    }

    out
}

// ── Markdown rendering ───────────────────────────────────────────────────────

/// Line-buffered markdown renderer.
///
/// Tokens arrive one at a time; characters are accumulated in `line_buf` until
/// a newline is received, at which point the complete line is classified and
/// rendered with appropriate ANSI styling.  Prose lines flow through a
/// `WrapWriter` for word-wrapping; block elements (headings, code blocks,
/// rules, lists) are printed directly.
struct MarkdownRenderer {
    /// Characters since the last newline.
    line_buf: String,
    /// True while inside a fenced code block.
    in_code_block: bool,
    /// Language tag from the opening fence, if any.
    code_lang: Option<String>,
    /// Word-wrap writer for prose content.
    wrap: WrapWriter,
}

impl MarkdownRenderer {
    fn new() -> Self {
        let mut wrap = WrapWriter::new();
        wrap.tint = true; // soft-white tint for AI prose
        Self {
            line_buf:      String::new(),
            in_code_block: false,
            code_lang:     None,
            wrap,
        }
    }

    /// Feed a streaming token into the renderer.
    fn feed(&mut self, token: &str) {
        for ch in token.chars() {
            match ch {
                '\n' => { self.process_line(); self.line_buf.clear(); }
                '\r' => {}
                _    => self.line_buf.push(ch),
            }
        }
    }

    /// Flush any buffered content without resetting the column counter.
    fn flush(&mut self) {
        if !self.line_buf.is_empty() {
            let text = std::mem::take(&mut self.line_buf);
            if self.in_code_block {
                print!("{}", highlight_code(&text, self.code_lang.as_deref()));
            } else {
                self.wrap.feed(&render_inline(&text));
            }
        }
        self.wrap.flush();
    }

    /// Flush buffered content and reset the column counter to zero.
    fn reset(&mut self) {
        self.flush();
        self.wrap.reset();
    }

    /// Classify and render the accumulated line.
    fn process_line(&mut self) {
        let line = self.line_buf.clone();

        // ── Fenced code block toggle ─────────────────────────────────────
        if line.starts_with("```") {
            if self.in_code_block {
                self.in_code_block = false;
                self.code_lang = None;
                let w = terminal_width();
                println!("\x1b[2m{}\x1b[0m", "─".repeat(w.min(72)));
                self.wrap.reset();
            } else {
                self.wrap.flush();
                self.wrap.reset();
                self.in_code_block = true;
                let lang = line[3..].trim().to_string();
                let w = terminal_width();
                let border = w.min(72);
                if lang.is_empty() {
                    println!("\x1b[2m{}\x1b[0m", "─".repeat(border));
                } else {
                    let label = format!(" {} ", lang);
                    let dashes = border.saturating_sub(2 + label.len());
                    println!("\x1b[2m──\x1b[0m\x1b[33m{}\x1b[2m{}\x1b[0m",
                             label, "─".repeat(dashes));
                }
                self.code_lang = if lang.is_empty() { None } else { Some(lang) };
            }
            return;
        }

        // ── Code block body ───────────────────────────────────────────────
        if self.in_code_block {
            println!("{}", highlight_code(&line, self.code_lang.as_deref()));
            return;
        }

        // ── ATX headings ─────────────────────────────────────────────────
        if let Some(rest) = line.strip_prefix("### ") {
            self.wrap.flush();
            println!("\n\x1b[1m\x1b[94m{}\x1b[0m", render_inline(rest)); // bold blue
            self.wrap.reset();
            return;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            self.wrap.flush();
            println!("\n\x1b[1m\x1b[96m{}\x1b[0m", render_inline(rest)); // bold bright-cyan
            self.wrap.reset();
            return;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            self.wrap.flush();
            println!("\n\x1b[1m\x1b[95m{}\x1b[0m", render_inline(rest)); // bold magenta
            self.wrap.reset();
            return;
        }

        // ── Horizontal rule (--- / *** / ___ of 3+ chars) ─────────────────
        {
            let t = line.trim();
            if t.len() >= 3
                && (t.chars().all(|c| c == '-')
                    || t.chars().all(|c| c == '*')
                    || t.chars().all(|c| c == '_'))
            {
                self.wrap.flush();
                let w = terminal_width();
                println!("\n\x1b[2m{}\x1b[0m\n", "─".repeat(w.min(72)));
                self.wrap.reset();
                return;
            }
        }

        // ── Bullet list (top-level and one level of indent) ───────────────
        let bullet = if line.starts_with("- ")
                     || line.starts_with("* ")
                     || line.starts_with("+ ")
        {
            Some((2usize, "\x1b[33m•\x1b[0m"))
        } else if line.starts_with("  - ") || line.starts_with("  * ") {
            Some((4usize, "  \x1b[2m◦\x1b[0m"))
        } else {
            None
        };
        if let Some((skip, sym)) = bullet {
            self.wrap.flush();
            print!("{} ", sym);
            // "• " or "  ◦ " — set col to the visual width of the leader.
            self.wrap.set_col(visual_len(sym) + 1);
            self.wrap.feed(&render_inline(&line[skip..]));
            self.wrap.flush();
            println!();
            self.wrap.reset();
            return;
        }

        // ── Numbered list (digits followed by ". ") ───────────────────────
        {
            let bytes = line.as_bytes();
            let mut j = 0;
            while j < bytes.len() && bytes[j].is_ascii_digit() { j += 1; }
            if j > 0 && j + 1 < bytes.len() && bytes[j] == b'.' && bytes[j + 1] == b' ' {
                self.wrap.flush();
                let num = &line[..j];
                print!("\x1b[33m{}.\x1b[0m ", num);
                self.wrap.set_col(num.len() + 2); // "N. "
                self.wrap.feed(&render_inline(&line[j + 2..]));
                self.wrap.flush();
                println!();
                self.wrap.reset();
                return;
            }
        }

        // ── Blockquote ────────────────────────────────────────────────────
        if let Some(rest) = line.strip_prefix("> ").or_else(|| line.strip_prefix(">")) {
            self.wrap.flush();
            print!("\x1b[2m│\x1b[0m ");
            self.wrap.set_col(2);
            self.wrap.feed(&render_inline(rest));
            self.wrap.flush();
            println!();
            self.wrap.reset();
            return;
        }

        // ── Empty line ────────────────────────────────────────────────────
        if line.trim().is_empty() {
            self.wrap.flush();
            println!();
            self.wrap.reset();
            return;
        }

        // ── Regular prose ─────────────────────────────────────────────────
        self.wrap.feed(&render_inline(&line));
        self.wrap.flush();
        println!();
        self.wrap.reset();
    }
}

/// Generate a random session ID from /dev/urandom.
/// Falls back to timestamp+PID entropy if /dev/urandom is unavailable,
/// avoiding the predictable all-zeros key produced by the old code.
fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{:02x}", b)).collect();
        }
    }
    // /dev/urandom unavailable — mix nanosecond timestamp with PID.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{:08x}{:08x}", nanos ^ pid, pid.wrapping_mul(2_654_435_761))
}

/// Ask the daemon to re-collect system context (OS info, memory, processes, history).
async fn send_refresh() -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut data = serde_json::to_vec(&crate::ipc::Request::Refresh)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    let mut rx = tokio::io::BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    Ok(())
}

async fn connect() -> Result<UnixStream> {
    let socket_path = Path::new(DEFAULT_SOCKET_PATH);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        UnixStream::connect(socket_path),
    )
    .await
    .with_context(|| format!("Timed out connecting to daemon at {} (is it running?)", DEFAULT_SOCKET_PATH))?
    .with_context(|| format!("Failed to connect to daemon at {}", DEFAULT_SOCKET_PATH))
}

async fn send_request(tx: &mut OwnedWriteHalf, req: Request) -> Result<()> {
    let mut data = serde_json::to_vec(&req)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

async fn recv(rx: &mut BufReader<OwnedReadHalf>) -> Result<Response> {
    let mut line = String::new();
    let n = rx.read_line(&mut line).await?;
    if n == 0 {
        anyhow::bail!("Daemon closed connection unexpectedly.");
    }
    let response: Response = serde_json::from_str(line.trim())?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── command_is_sudo ───────────────────────────────────────────────────────

    #[test]
    fn command_is_sudo_simple() {
        assert!(command_is_sudo("sudo apt install vim"));
    }

    #[test]
    fn command_is_sudo_in_pipeline() {
        assert!(command_is_sudo("echo hi | sudo tee /etc/hosts"));
    }

    #[test]
    fn command_is_sudo_after_semicolon() {
        assert!(command_is_sudo("cd /tmp; sudo rm -rf foo"));
    }

    #[test]
    fn command_is_sudo_false_positive_guard() {
        // "sudoers" is not "sudo" — word-boundary must hold.
        assert!(!command_is_sudo("cat /etc/sudoers"));
    }

    #[test]
    fn command_is_sudo_no_sudo() {
        assert!(!command_is_sudo("ls -la /home"));
    }

    // ── visual_len ────────────────────────────────────────────────────────────

    #[test]
    fn visual_len_plain_ascii() {
        assert_eq!(visual_len("hello"), 5);
    }

    #[test]
    fn visual_len_empty_string() {
        assert_eq!(visual_len(""), 0);
    }

    #[test]
    fn visual_len_strips_ansi_reset() {
        // "\x1b[0m" is an ANSI reset — it contributes 0 visual columns.
        assert_eq!(visual_len("\x1b[0mhello"), 5);
    }

    #[test]
    fn visual_len_strips_ansi_colour() {
        assert_eq!(visual_len("\x1b[31mred\x1b[0m"), 3);
    }

    #[test]
    fn visual_len_strips_bold() {
        assert_eq!(visual_len("\x1b[1mbold text\x1b[0m"), 9);
    }

    #[test]
    fn visual_len_nested_escape_sequences() {
        // Two different ANSI sequences around some text.
        let s = "\x1b[1m\x1b[32mgreen bold\x1b[0m\x1b[0m";
        assert_eq!(visual_len(s), 10);
    }

    #[test]
    fn visual_len_no_escape_inside_word() {
        // "DaemonEye" has no escapes — all 9 chars count.
        assert_eq!(visual_len("DaemonEye"), 9);
    }

    // ── fmt_uptime ────────────────────────────────────────────────────────────

    #[test]
    fn fmt_uptime_seconds_only() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(0)),  "0s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(42)), "42s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(59)), "59s");
    }

    #[test]
    fn fmt_uptime_minutes_and_seconds() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(60)),  "1m 0s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(90)),  "1m 30s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3599)), "59m 59s");
    }

    #[test]
    fn fmt_uptime_hours_and_minutes() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3600)),  "1h 0m");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3660)),  "1h 1m");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(7322)),  "2h 2m");
    }

    #[test]
    fn fmt_uptime_exact_hour_boundary() {
        // 3600s == 1h 0m, not shown as minutes
        let out = fmt_uptime(std::time::Duration::from_secs(3600));
        assert!(out.contains('h'), "should show hours: {out}");
        assert!(!out.contains('s'), "should not show seconds: {out}");
    }
}
