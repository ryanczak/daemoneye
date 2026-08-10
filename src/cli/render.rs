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
