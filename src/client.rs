use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::Config;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};

/// A persistent async stdin reader owned for the lifetime of a chat session.
/// Using one reader for both the main query loop and tool-call approvals
/// guarantees a single internal buffer — no bytes get lost between the two
/// call sites.
type ChatStdin = tokio::io::Lines<tokio::io::BufReader<tokio::io::Stdin>>;

pub fn run_setup() -> Result<()> {
    // Write the systemd user service file.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let systemd_dir = PathBuf::from(&home).join(".config/systemd/user");
    let service_path = systemd_dir.join("t1000.service");

    let service_content = "\
[Unit]
Description=T1000 AI Tmux Daemon
After=network.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/t1000 daemon
ExecStop=%h/.cargo/bin/t1000 stop
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
            println!("systemctl --user enable --now t1000");
            println!();
            println!("# Check status and view logs:");
            println!("systemctl --user status t1000");
            println!("t1000 logs");
        }
        Err(e) => {
            eprintln!("Warning: could not write service file: {}", e);
            eprintln!("You can install it manually:");
            eprintln!("  mkdir -p ~/.config/systemd/user");
            eprintln!("  cp t1000.service ~/.config/systemd/user/");
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
    let t1000_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "t1000".to_string());

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!(
        "bind-key T split-window {} -e \"T1000_SOURCE_PANE=#{{pane_id}}\" '{} chat'",
        split_flag, t1000_bin
    );
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");
    println!();
    println!("# If you already have a bind-key that uses the bare name 't1000',");
    println!("# replace it with the full path above — the tmux session may not");
    println!("# inherit ~/.cargo/bin in its PATH.");

    Ok(())
}

pub fn run_logs(path: PathBuf) -> Result<()> {
    if !path.exists() {
        eprintln!("No log file found at {}.", path.display());
        eprintln!("The daemon writes logs there by default when started with: t1000 daemon");
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
    // Single-shot: create a fresh stdin reader just for any tool-call approvals.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    ask_with_session(query, None, &mut stdin).await
}

pub async fn run_chat() -> Result<()> {
    let result = run_chat_inner().await;
    if let Err(ref e) = result {
        // The ChatStdin async reader has been dropped by now, so using the
        // synchronous stdin here is safe.
        use std::io::Write;
        eprintln!("\n\x1b[31m✗\x1b[0m t1000 error: {}", e);
        eprint!("\x1b[2mPress Enter to close this pane…\x1b[0m");
        std::io::stderr().flush().ok();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    result
}

async fn run_chat_inner() -> Result<()> {
    use std::io::Write;

    let mut session_id = new_session_id();

    // One async stdin reader shared by the main query loop AND any tool-call
    // approval prompts inside ask_with_session. Sharing a single BufReader
    // means there is only one internal buffer — no bytes can silently disappear
    // between the two readers.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();

    // If running inside tmux, resize the chat pane to 25% of the window width
    // (minimum 20 cols) so the header has a comfortable width, then query the
    // exact post-resize width directly from tmux.  TIOCGWINSZ can lag behind
    // the actual pane size due to the SIGWINCH race at pane creation time,
    // which would cause the header border to fall short of the right edge.
    let chat_width: usize = if let Ok(pane_id) = std::env::var("TMUX_PANE") {
        let target = crate::tmux::query_window_width(&pane_id)
            .map(|w| (w * 25 / 100).max(20))
            .unwrap_or(100);
        let _ = crate::tmux::resize_pane_width(&pane_id, target);
        crate::tmux::query_pane_width(&pane_id).unwrap_or(target)
    } else {
        terminal_width()
    };

    // Header box — round corners, bright cyan, width-adaptive.
    // Stretches to fill the measured chat pane width.
    {
        let w     = chat_width.max(70);
        let inner = w - 2; // chars between the corner glyphs

        // Title row anchors — each 19 visible chars.
        let title_left  = "─ T1000  AI  Agent ";
        let title_right = format!(" session:{} ─", &session_id[..8]);
        // Use visual_len (char-count) not .len() (byte-count): ─ and · are
        // multi-byte UTF-8 but each occupies exactly one terminal column.
        let anchors = visual_len(title_left) + visual_len(&title_right);

        let top = if inner >= anchors {
            let mid = "─".repeat(inner - anchors);
            format!("\x1b[1m\x1b[96m╭{title_left}{mid}\x1b[2m{title_right}\x1b[22m╮\x1b[0m")
        } else {
            // Terminal too narrow to fit the session tag — just fill with dashes.
            let dashes = "─".repeat(inner.saturating_sub(visual_len(title_left)));
            format!("\x1b[1m\x1b[96m╭{title_left}{dashes}╮\x1b[0m")
        };
        println!("{top}");

        // Hint row — pad to inner width.
        let hint     = "  Type '\x1b[1mexit\x1b[0m' or \x1b[1mCtrl-C\x1b[0m to close  ·  \x1b[1m/clear\x1b[0m to reset session";
        let hint_vis = visual_len(hint);
        let pad      = " ".repeat(inner.saturating_sub(hint_vis));
        println!("\x1b[1m\x1b[96m│\x1b[0m{hint}{pad}\x1b[1m\x1b[96m│\x1b[0m");

        // Bottom row.
        let bot = "─".repeat(inner);
        println!("\x1b[1m\x1b[96m╰{bot}╯\x1b[0m");
    }
    println!();

    // Send an automatic opening message so the AI greets the user immediately
    // rather than waiting for them to type first.
    if let Err(e) = ask_with_session("Hello!".to_string(), Some(&session_id), &mut stdin).await {
        eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
        eprintln!("  Make sure it is running:  \x1b[1mt1000 daemon --console\x1b[0m");
        eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
    }

    loop {
        print!("\n\x1b[92m❯\x1b[0m ");
        std::io::stdout().flush()?;

        match stdin.next_line().await? {
            None => break, // EOF / pane closed
            Some(line) => {
                let query = line.trim().to_string();
                if query.is_empty() { continue; }
                if query == "exit" || query == "quit" { break; }
                if query == "/clear" {
                    session_id = new_session_id();
                    let w = terminal_width();
                    let label = format!(" session cleared · new session:{} ", &session_id[..8]);
                    let dashes = w.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                    continue;
                }
                if let Err(e) = ask_with_session(query, Some(&session_id), &mut stdin).await {
                    eprintln!("\n\x1b[31m✗\x1b[0m {}", e);
                }
            }
        }
    }

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

async fn ask_with_session(query: String, session_id: Option<&str>, stdin: &mut ChatStdin) -> Result<()> {
    use std::io::Write;
    use std::time::Duration;

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    // T1000_SOURCE_PANE is set by the recommended tmux bind-key:
    //   split-window -h -e "T1000_SOURCE_PANE=#{pane_id}" 't1000 chat'
    // It records the user's working pane before the split so the daemon
    // captures context from — and injects commands into — the right pane.
    // Falls back to TMUX_PANE, which is correct when `t1000 chat` or
    // `t1000 ask` is run directly from the user's working pane.
    let tmux_pane = std::env::var("T1000_SOURCE_PANE")
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
            // Phase 2 — streaming: wait directly without spinning.
            recv(&mut rx).await?
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
                print!("  \x1b[32mApprove?\x1b[0m [y/N] \x1b[32m›\x1b[0m ");
                std::io::stdout().flush()?;
                // Use the shared async stdin reader so the buffer stays
                // consistent with the main query loop — no bytes dropped.
                let input = stdin.next_line().await?.unwrap_or_default();
                let approved = input.trim().eq_ignore_ascii_case("y");
                if approved {
                    println!("  \x1b[32m✓ approved\x1b[0m");
                } else {
                    println!("  \x1b[2m✗ skipped\x1b[0m");
                }
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
            Response::SudoPrompt { id, command } => {
                md.flush();
                println!("\n\x1b[33m⚠\x1b[0m  \x1b[1msudo required\x1b[0m  \x1b[1m$\x1b[0m {}", command);
                // read_password_silent uses synchronous stdin with echo disabled.
                // The async BufReader hasn't consumed any bytes yet at this point
                // because the user hasn't typed anything since the last prompt.
                let password = read_password_silent("   \x1b[33mPassword:\x1b[0m ").unwrap_or_default();
                // read_password_silent ends with println(), so col is back to 0.
                md.reset();
                send_request(&mut tx, Request::SudoPassword { id, password }).await?;
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
fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

async fn connect() -> Result<UnixStream> {
    let socket_path = Path::new(DEFAULT_SOCKET_PATH);
    UnixStream::connect(socket_path)
        .await
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
