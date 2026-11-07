use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Show `ch` as visible text: control and format characters become their
/// Rust-style escape form, so a reader sees the byte that would otherwise act
/// invisibly on the terminal. `ch` of width 0 (zero-width format characters)
/// is shown so it is not silently dropped.
fn visible_control(ch: char) -> Option<String> {
    let is_control = ch.is_control() || ('\u{80}'..='\u{9f}').contains(&ch);
    let is_format = matches!(ch,
        '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}'
        | '\u{feff}'
    );
    if !is_control && !is_format {
        return None;
    }
    Some(match ch {
        '\t' => "\\t".to_string(),
        '\r' => "\\r".to_string(),
        '\0' => "\\0".to_string(),
        '\u{1b}' => "\\x1b".to_string(),
        '\u{7f}' => "\\x7f".to_string(),
        '\u{9b}' => "\\u{9b}".to_string(),
        '\u{85}' => "\\u{85}".to_string(),
        '\u{feff}' => "\\u{feff}".to_string(),
        _ => {
            let mut out = String::new();
            for b in ch.escape_default() {
                out.push(b);
            }
            out
        }
    })
}

/// Hard-wrap `text` at `width` display columns (measured with
/// `unicode-width`), one row per hard line at least. A character that does
/// not fit in the rest of the row starts the next row: a wide character is
/// never split across the edge and no character is dropped. Every control or
/// invisible format character except `\n` is shown in its visible escape form
/// (`visible_control`), which counts toward the row's width like any other
/// text and is never split across rows. Joining the rows of a hard line gives
/// back that line with the escapes undone.
pub fn wrap_display(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for hard_line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0usize;
        for ch in hard_line.chars() {
            let shown = visible_control(ch).unwrap_or_else(|| ch.to_string());
            let w = UnicodeWidthStr::width(shown.as_str());
            if !row.is_empty() && used + w > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push_str(&shown);
            used += w;
        }
        rows.push(row);
    }
    rows
}

/// The display width of `s` in terminal columns: the ANSI sequences the
/// renderer inserts itself are skipped, every other character counts what
/// `unicode-width` says (CJK 2, combining 0). Control characters count 0 —
/// they are measured only where a wrapper shows them in escape form.
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
            count += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    count
}

/// Map every control or invisible-format character except `\n` through
/// [`visible_control`], so text printed with `println!` (`top --once`) can
/// never carry a raw escape byte to the terminal. Plain text, spaces and
/// `\n` are unchanged. Pure.
pub fn escape_controls(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch == '\n' {
                ch.to_string()
            } else {
                visible_control(ch).unwrap_or_else(|| ch.to_string())
            }
        })
        .collect()
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

/// Split a styled line into rows of at most `width` display columns, so the
/// markdown committer never returns a row wider than its width. Styles are
/// preserved row by row: a continuation row carries the style of the text it
/// holds (a heading's second row is still bold, a code row keeps its
/// highlighting). A wide character never splits across the edge and no
/// character is dropped. Width is measured with `unicode-width`; spans are
/// already parsed, so the visible-escape rule of `wrap_display` does not
/// apply here.
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line.width() <= width {
        return vec![line];
    }
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in line.spans {
        let style = span.style;
        let mut current = String::new();
        for ch in span.content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if !current.is_empty() && used + w > width {
                spans.push(Span::styled(std::mem::take(&mut current), style));
                rows.push(Line::from(std::mem::take(&mut spans)));
                used = 0;
            }
            current.push(ch);
            used += w;
        }
        if !current.is_empty() {
            spans.push(Span::styled(current, style));
        }
    }
    if !spans.is_empty() {
        rows.push(Line::from(spans));
    }
    rows
}

/// The status-bar fields passed as a single reference to every rendering function.
pub struct StatusBarState<'a> {
    pub session_id: &'a str,
    pub approval_hint: &'a str,
    pub model: &'a str,
    /// The chat's turn counter: 0 before the first turn, the resumed count
    /// after `--resume`.
    pub turn: usize,
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
