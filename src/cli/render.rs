


// ── Async stdin wrapper ───────────────────────────────────────────────────────

/// Non-owning handle to fd 0 used with `AsyncFd`.  Does not close the fd on
/// drop — closing stdin would break the process.
/// Render a bright-cyan bordered panel at terminal width.
///
/// `title`    — label embedded in the top border
/// `body`     — lines of text to show inside; long lines are truncated with `…`
/// `dim_body` — if true the body text is rendered dim (for captured output)
pub fn print_tool_panel(title: &str, body: &[&str], dim_body: bool) {
    let w     = terminal_width().max(44);
    let inner = w - 2; // visible chars between corner glyphs

    // ── Top border: ╭─ title ────────────────────────────╮ ─────────────
    let tpart = format!("─ {} ", title);
    let fill  = inner.saturating_sub(visual_len(&tpart) + 1); // +1 for the ─ before ╮
    println!("\x1b[1m\x1b[96m╭{tpart}{}─╮\x1b[0m", "─".repeat(fill));

    // ── Body lines ──────────────────────────────────────────────────────
    let avail = inner.saturating_sub(2); // 2 for the "  " indent
    for line in body {
        for wrapped_line in wrap_line_hard(line, avail) {
            let vis = visual_len(&wrapped_line);
            let pad = " ".repeat(inner.saturating_sub(2 + vis));
            if dim_body {
                println!("\x1b[1m\x1b[96m│\x1b[0m  \x1b[2m{wrapped_line}\x1b[0m{pad}\x1b[1m\x1b[96m│\x1b[0m");
            } else {
                println!("\x1b[1m\x1b[96m│\x1b[0m  {wrapped_line}{pad}\x1b[1m\x1b[96m│\x1b[0m");
            }
        }
    }

    // ── Bottom border: ╰──────────────────────────────────╯ ─────────────
    println!("\x1b[1m\x1b[96m╰{}\x1b[22m╯\x1b[0m", "─".repeat(inner));
}

/// Render a user query as a bordered box in the chat history scroll region.
///
/// The box uses the same bold-cyan `╭╮╰╯` style as the input frame and the
/// tool panel.  Long lines are word-wrapped.  The turn/context info is
/// right-justified into the bottom border, mirroring where `SessionInfo`
/// was previously printed as a leading horizontal rule.
///
/// `query`         — raw user text (may contain newlines and special chars)
/// `turn`          — 1-based turn number
/// `message_count` — number of messages in context before this query
pub fn print_user_query(query: &str, turn: usize, message_count: usize) {
    use std::io::Write;
    let w     = terminal_width().max(44);
    let inner = w - 2; // visible chars between corner glyphs

    // ── Top border: ╭─ You ──────────────────────────╮ ─────────────
    let tpart = "─ You ";
    let fill  = inner.saturating_sub(visual_len(tpart) + 1); // +1 for ─ before ╮
    println!("\x1b[1m\x1b[96m╭{tpart}{}─╮\x1b[0m", "─".repeat(fill));

    // ── Body lines (word-wrap aware) ──────────────────────────────────
    let avail = inner.saturating_sub(2); // 2 for the "  " indent
    for raw_line in query.lines() {
        // Escape every \ so wrap_line_hard sees literal characters, not ANSI
        // codes. The query text is plain UTF-8, no escape sequences to consider.
        for wrapped in wrap_line_hard(raw_line, avail) {
            let vis = visual_len(&wrapped);
            let pad = " ".repeat(inner.saturating_sub(2 + vis));
            println!("\x1b[1m\x1b[96m│\x1b[0m  {wrapped}{pad}\x1b[1m\x1b[96m│\x1b[0m");
        }
    }

    // ── Bottom border with right-justified turn/context label ──────────
    let ctx_label = if message_count == 0 {
        "new session".to_string()
    } else {
        format!("{} message{} in context",
            message_count,
            if message_count == 1 { "" } else { "s" })
    };
    let label     = format!(" turn {} · {} ", turn, ctx_label);
    let label_vis = visual_len(&label);
    // Fill the dashes: total inner width minus label minus 1 for the ─ prefix on label side
    let dashes = inner.saturating_sub(label_vis + 1);
    println!("\x1b[1m\x1b[96m╰{}\x1b[2m{label}\x1b[0m\x1b[1m\x1b[96m─╯\x1b[0m",
        "─".repeat(dashes));
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
pub fn terminal_width() -> usize {
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
pub fn terminal_height() -> usize {
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
pub fn setup_scroll_region(height: usize) {
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
pub fn teardown_scroll_region(height: usize) {
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
pub fn fmt_uptime(elapsed: std::time::Duration) -> String {
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
pub fn draw_input_frame(height: usize, width: usize, start: std::time::Instant) {
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
pub fn draw_status_bar(height: usize, width: usize, session_id: &str, status: &str) {
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
pub struct MarkdownRenderer {
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
    pub fn new() -> Self {
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
    pub fn feed(&mut self, token: &str) {
        for ch in token.chars() {
            match ch {
                '\n' => { self.process_line(); self.line_buf.clear(); }
                '\r' => {}
                _    => self.line_buf.push(ch),
            }
        }
    }

    /// Flush any buffered content without resetting the column counter.
    pub fn flush(&mut self) {
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
    pub fn reset(&mut self) {
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

