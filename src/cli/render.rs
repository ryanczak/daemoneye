/// Render a bordered panel at terminal width.
///
/// `title`    — label embedded in the top border
/// `body`     — lines of text to show inside; long lines are truncated with `…`
/// `dim_body` — if true the body text is rendered dim (for captured output)
pub fn print_tool_panel(title: &str, body: &[&str], dim_body: bool) {
    let w = terminal_width().max(44);
    let inner = w - 2; // visible chars between corner glyphs

    // ── Top border: ╭─ title ────────────────────────────╮ ─────────────
    // Content between ╭ and ╮: "─ " (2) + title + " " (1) + fill×"─" + "─" (1) = inner
    let fill = inner.saturating_sub(visual_len(title) + 4);
    println!(
        "\x1b[38;5;88m\x1b[1m╭─ \x1b[38;5;136m{title}\x1b[38;5;88m {}─╮\x1b[0m",
        "─".repeat(fill)
    );

    // ── Body lines ──────────────────────────────────────────────────────
    let avail = inner.saturating_sub(2); // 2 for the "  " indent
    for line in body {
        for wrapped_line in wrap_line_hard(line, avail) {
            if dim_body {
                println!("  \x1b[2m{wrapped_line}\x1b[0m");
            } else {
                println!("  {wrapped_line}");
            }
        }
    }

    // ── Bottom border: ╰──────────────────────────────────╯ ─────────────
    println!("\x1b[38;5;88m\x1b[1m╰{}╯\x1b[0m", "─".repeat(inner));
}

/// Return `user@hostname` for the local machine, used as the label in the
/// user query border.  Reads `$USER` and `$HOSTNAME` from the environment
/// (bash sets both automatically; daemoneye inherits them from its shell).
/// Falls back gracefully if either is missing.
fn local_user_host() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "you".to_string());
    let host = std::env::var("HOSTNAME").unwrap_or_default();
    // Strip domain suffix: `scrappy.local` → `scrappy`
    let host = host.split('.').next().unwrap_or("").to_string();
    if host.is_empty() {
        user
    } else {
        format!("{}@{}", user, host)
    }
}

/// Print a one-line `▸ tool(summary)` entry when a silent tool call starts.
/// The caller should set `response_started = false` afterward so the Phase-1
/// spinner takes over to animate the elapsed time while the tool runs.
pub fn print_tool_started(tool: &str, summary: &str) {
    use std::io::Write;
    let args = if summary.is_empty() {
        String::new()
    } else {
        format!("({})", summary)
    };
    println!("  \x1b[2m\x1b[36m▸\x1b[0m \x1b[2m{tool}{args}\x1b[0m");
    std::io::stdout().flush().ok();
}

/// Print a one-line `⎿ detail · elapsed` result entry when a silent tool call finishes.
/// Should be called after clearing the spinner line with `\r\x1b[K`.
pub fn print_tool_finished(ok: bool, elapsed_ms: u64, detail: Option<&str>) {
    use std::io::Write;
    let mark = if ok {
        "\x1b[32m⎿\x1b[0m"
    } else {
        "\x1b[31m⎿\x1b[0m"
    };
    let secs = elapsed_ms as f64 / 1000.0;
    let status = detail.unwrap_or(if ok { "ok" } else { "failed" });
    println!("    {mark} \x1b[2m{status} · {secs:.1}s\x1b[0m");
    std::io::stdout().flush().ok();
}

/// Render a user query as a bordered box in the chat history scroll region.
///
/// The box uses the same bold-cyan `╭╮╰╯` style as the input frame and the
/// tool panel.  Long lines are word-wrapped.  The turn/context info is
/// right-justified into the bottom border, mirroring where `SessionInfo`
/// was previously printed as a leading horizontal rule.
///
/// `query`          — raw user text (may contain newlines and special chars)
/// `turn`           — 1-based turn number
/// `prompt_tokens`  — prompt token count from the most recent AI turn (0 = new session)
/// `context_window` — model context window in tokens (used to show % remaining)
pub fn print_user_query(query: &str, turn: usize, prompt_tokens: u32, context_window: u32) {
    use std::io::Write;
    let w = terminal_width().max(44);
    let inner = w - 2; // visible chars between corner glyphs

    // ── Top border: ╭─ matt@scrappy ──────────────────╮ ─────────────
    let identity = local_user_host();
    let tpart = format!("─ {} ", identity); // plain for visual_len
    let fill = inner.saturating_sub(visual_len(&tpart) + 1); // +1 for ─ before ╮
    println!(
        "\x1b[38;5;88m\x1b[1m╭─ \x1b[38;5;136m{identity}\x1b[38;5;88m {}─╮\x1b[0m",
        "─".repeat(fill)
    );

    // ── Body lines (word-wrap aware) ──────────────────────────────────
    let avail = inner.saturating_sub(2); // 2 for the "  " indent
    for raw_line in query.lines() {
        for wrapped in wrap_line_hard(raw_line, avail) {
            println!("  {wrapped}");
        }
    }

    // ── Bottom border with right-justified context-budget label ───────
    // Show used/total tokens with % remaining.
    let budget_label = if prompt_tokens == 0 {
        "new session".to_string()
    } else {
        let pct_used = (prompt_tokens as f64 / context_window.max(1) as f64 * 100.0) as u32;
        format!("{} / {} ({}%)", prompt_tokens, context_window, pct_used)
    };
    let label = format!(" turn {} · {} ", turn, budget_label);
    let label_vis = visual_len(&label);
    let dashes = inner.saturating_sub(label_vis + 1);
    println!(
        "\x1b[38;5;88m\x1b[1m╰{}\x1b[0m\x1b[38;5;136m{label}\x1b[38;5;88m\x1b[1m─╯\x1b[0m",
        "─".repeat(dashes)
    );
    std::io::stdout().flush().ok();
}

/// Count the visible (printable) characters in a string, skipping ANSI escape
/// sequences.  Used to measure word width correctly when the pending word
/// contains bold or colour codes injected by the markdown renderer.
pub fn wrap_line_hard(s: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for hard_line in s.split('\n') {
        let mut current_line = String::new();
        let mut current_vis = 0;
        let mut in_esc = false;

        for ch in hard_line.chars() {
            current_line.push(ch);
            if in_esc {
                if ch.is_ascii_alphabetic() {
                    in_esc = false;
                }
            } else if ch == '\x1b' {
                in_esc = true;
            } else {
                current_vis += 1;
                if current_vis == width {
                    lines.push(current_line);
                    current_line = String::new();
                    current_vis = 0;
                }
            }
        }
        if !current_line.is_empty() || lines.is_empty() {
            lines.push(current_line);
        }
    }
    lines
}

pub fn visual_len(s: &str) -> usize {
    let mut count = 0usize;
    let mut in_esc = false;
    for ch in s.chars() {
        if in_esc {
            if ch.is_ascii_alphabetic() {
                in_esc = false;
            }
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
pub fn terminal_width() -> usize {
    // SAFETY: `ioctl(TIOCGWINSZ)` has no safe Rust alternative for querying live
    // terminal dimensions. `ws` is zeroed before the call and only read after
    // a successful return.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 1 {
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
pub fn terminal_height() -> usize {
    // SAFETY: `ioctl(TIOCGWINSZ)` has no safe Rust alternative for querying live
    // terminal dimensions. `ws` is zeroed before the call and only read after
    // a successful return.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_row > 2 {
            return ws.ws_row as usize;
        }
    }
    std::env::var("LINES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(24)
}

/// The status-bar fields passed as a single reference to every rendering function.
pub struct StatusBarState<'a> {
    pub session_id: &'a str,
    pub approval_hint: &'a str,
    pub model: &'a str,
    pub prompt_tokens: u32,
    pub context_window: u32,
    pub daemon_up: bool,
    /// Session-cumulative count of silent tool calls (incremented on ToolStarted).
    pub tools_total: u32,
    /// Cumulative cost of this session in USD.
    pub cost_usd: f64,
    /// Whether any AI call in this session had Unknown pricing.
    pub has_untracked: bool,
}
