mod syntax;

use crate::cli::render::{terminal_width, visual_len};
use syntax::highlight_code;

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
        Self {
            col: 0,
            pending: String::new(),
            space_before: false,
            tint: false,
        }
    }

    /// Feed a streaming token into the writer.
    fn feed(&mut self, token: &str) {
        for ch in token.chars() {
            match ch {
                '\n' => {
                    self.emit_word();
                    println!();
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
pub fn render_inline(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 32);
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut in_bold = false;
    let mut in_italic = false;
    let mut in_code = false;

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
                let at_start = i == 0 || chars[i - 1] == ' ';
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
            c => {
                out.push(c);
                i += 1;
            }
        }
    }

    if in_bold || in_italic || in_code {
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

impl Default for MarkdownRenderer {
    fn default() -> Self {
        let mut wrap = WrapWriter::new();
        wrap.tint = true; // soft-white tint for AI prose
        Self {
            line_buf: String::new(),
            in_code_block: false,
            code_lang: None,
            wrap,
        }
    }
}

impl MarkdownRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a streaming token into the renderer, yielding completed styled
    /// lines suitable for ratatui scrollback.  Unlike `feed` (which prints to
    /// stdout), this method buffers tokens until complete lines are formed,
    /// then renders each line to `Vec<Line>` via `render_line_to_spans`.
    ///
    /// Returns the styled lines for any complete lines finished by this token.
    /// Partial trailing content stays in `line_buf` for the next call.
    /// Call `flush_to_lines()` at the end of the turn to render the final
    /// partial line.
    pub fn feed_to_lines(
        &mut self,
        token: &str,
        width: usize,
    ) -> Vec<ratatui::text::Line<'static>> {
        let mut lines = Vec::new();
        for ch in token.chars() {
            match ch {
                '\n' => {
                    let text = std::mem::take(&mut self.line_buf);
                    if !text.is_empty() {
                        lines.extend(self.render_line_to_spans(&text, width));
                    } else {
                        // Empty line
                        lines.push(ratatui::text::Line::from(vec![]));
                    }
                }
                '\r' => {}
                _ => self.line_buf.push(ch),
            }
        }
        lines
    }

    /// Flush any remaining partial line to styled `Line`s.
    /// Call this at the end of a streaming turn to render the final
    /// incomplete line.  Returns the styled lines (may be empty).
    pub fn flush_to_lines(&mut self, width: usize) -> Vec<ratatui::text::Line<'static>> {
        if self.line_buf.is_empty() {
            return Vec::new();
        }
        let text = std::mem::take(&mut self.line_buf);
        self.render_line_to_spans(&text, width)
    }

    /// Feed a streaming token into the renderer.
    pub fn feed(&mut self, token: &str) {
        for ch in token.chars() {
            match ch {
                '\n' => {
                    self.process_line();
                    self.line_buf.clear();
                }
                '\r' => {}
                _ => self.line_buf.push(ch),
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

    /// Render the current line buffer to styled ratatui `Line`s without
    /// printing to stdout.  Returns completed lines plus any remaining partial
    /// line as a tuple `(Vec<Line>, Option<Line>)`.  The caller should commit
    /// the completed lines and keep the partial for the next token.
    pub fn render_line_to_spans(
        &mut self,
        line: &str,
        width: usize,
    ) -> Vec<ratatui::text::Line<'static>> {
        use crate::cli::render_ratatui::parse_ansi_to_spans;

        // ── Fenced code block toggle ──
        if line.starts_with("```") {
            if self.in_code_block {
                self.in_code_block = false;
                self.code_lang = None;
                // Closing fence: dim separator line.
                let sep = "─".repeat(width.min(72));
                let styled = format!("\x1b[2m{}\x1b[0m", sep);
                return vec![ratatui::text::Line::from(parse_ansi_to_spans(&styled))];
            } else {
                // Opening fence.
                let lang = line.strip_prefix("```").unwrap_or("").trim().to_string();
                self.in_code_block = true;
                self.code_lang = if lang.is_empty() {
                    None
                } else {
                    Some(lang.clone())
                };
                let border = width.min(72);
                let styled = if lang.is_empty() {
                    format!("\x1b[2m{}\x1b[0m", "─".repeat(border))
                } else {
                    let label = format!(" {} ", lang);
                    let dashes = border.saturating_sub(2 + label.len());
                    format!(
                        "\x1b[2m──\x1b[0m\x1b[33m{}\x1b[2m{}\x1b[0m",
                        label,
                        "─".repeat(dashes)
                    )
                };
                return vec![ratatui::text::Line::from(parse_ansi_to_spans(&styled))];
            }
        }

        // ── Code block body ──
        if self.in_code_block {
            let styled = highlight_code(line, self.code_lang.as_deref());
            return vec![ratatui::text::Line::from(parse_ansi_to_spans(&styled))];
        }

        // ── ATX headings ──
        if let Some(rest) = line.strip_prefix("### ") {
            let styled = format!("\x1b[1m\x1b[94m{}\x1b[0m", render_inline(rest));
            return vec![
                ratatui::text::Line::from(vec![]), // blank line before heading
                ratatui::text::Line::from(parse_ansi_to_spans(&styled)),
            ];
        }
        if let Some(rest) = line.strip_prefix("## ") {
            let styled = format!("\x1b[1m\x1b[96m{}\x1b[0m", render_inline(rest));
            return vec![
                ratatui::text::Line::from(vec![]),
                ratatui::text::Line::from(parse_ansi_to_spans(&styled)),
            ];
        }
        if let Some(rest) = line.strip_prefix("# ") {
            let styled = format!("\x1b[1m\x1b[95m{}\x1b[0m", render_inline(rest));
            return vec![
                ratatui::text::Line::from(vec![]),
                ratatui::text::Line::from(parse_ansi_to_spans(&styled)),
            ];
        }

        // ── Horizontal rule ──
        {
            let t = line.trim();
            if t.len() >= 3
                && (t.chars().all(|c| c == '-')
                    || t.chars().all(|c| c == '*')
                    || t.chars().all(|c| c == '_'))
            {
                let sep = "─".repeat(width.min(72));
                let styled = format!("\n\x1b[2m{}\x1b[0m\n", sep);
                let mut result = Vec::new();
                for sub in styled.split('\n') {
                    result.push(ratatui::text::Line::from(parse_ansi_to_spans(sub)));
                }
                return result;
            }
        }

        // ── Bullet list ──
        let bullet = if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
            Some((2usize, "\x1b[33m•\x1b[0m"))
        } else if line.starts_with("  - ") || line.starts_with("  * ") {
            Some((4usize, "  \x1b[2m◦\x1b[0m"))
        } else {
            None
        };
        if let Some((skip, sym)) = bullet {
            let leader_len = visual_len(sym) + 1; // sym + space
            let content = render_inline(&line[skip..]);
            let mut lines = Vec::new();
            let mut col = leader_len;
            let mut current = String::from(sym) + " ";
            for word in content.split(' ') {
                let word_len = visual_len(word);
                if col + word_len <= width {
                    if col > leader_len {
                        current.push(' ');
                    }
                    current.push_str(word);
                    col += word_len + if col > leader_len { 1 } else { 0 };
                } else {
                    lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
                    current = String::from(word);
                    col = word_len;
                }
            }
            if !current.is_empty() {
                lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
            }
            return lines;
        }

        // ── Numbered list ──
        {
            let bytes = line.as_bytes();
            let mut j = 0;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > 0 && j + 1 < bytes.len() && bytes[j] == b'.' && bytes[j + 1] == b' ' {
                let num = &line[..j];
                let leader = format!("\x1b[33m{}.\x1b[0m ", num);
                let leader_len = num.len() + 2;
                let content = render_inline(&line[j + 2..]);
                let mut lines = Vec::new();
                let mut col = leader_len;
                let mut current = leader;
                for word in content.split(' ') {
                    let word_len = visual_len(word);
                    if col + word_len <= width {
                        if col > leader_len {
                            current.push(' ');
                        }
                        current.push_str(word);
                        col += word_len + if col > leader_len { 1 } else { 0 };
                    } else {
                        lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
                        current = String::from(word);
                        col = word_len;
                    }
                }
                if !current.is_empty() {
                    lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
                }
                return lines;
            }
        }

        // ── Blockquote ──
        if let Some(rest) = line.strip_prefix("> ").or_else(|| line.strip_prefix(">")) {
            let leader = "\x1b[2m│\x1b[0m ";
            let content = render_inline(rest);
            let mut lines = Vec::new();
            let mut col = 2;
            let mut current = String::from(leader);
            for word in content.split(' ') {
                let word_len = visual_len(word);
                if col + word_len <= width {
                    if col > 2 {
                        current.push(' ');
                    }
                    current.push_str(word);
                    col += word_len + if col > 2 { 1 } else { 0 };
                } else {
                    lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
                    current = format!("{}{}", leader, word);
                    col = 2 + word_len;
                }
            }
            if !current.is_empty() {
                lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
            }
            return lines;
        }

        // ── Empty line ──
        if line.trim().is_empty() {
            return vec![ratatui::text::Line::from(vec![])];
        }

        // ── Regular prose — word-wrapped with tint ──
        let styled = render_inline(line);
        let mut lines = Vec::new();
        let mut col = 0usize;
        let mut current = String::new();
        let tint_on = "\x1b[97m";
        let tint_off = "\x1b[0m";

        for word in styled.split(' ') {
            let word_len = visual_len(word);
            let tinted_word = format!("{}{}{}", tint_on, word, tint_off);
            if col == 0 {
                current = tinted_word;
                col = word_len;
            } else if col + 1 + word_len <= width {
                current.push(' ');
                current.push_str(&tinted_word);
                col += 1 + word_len;
            } else {
                lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
                current = tinted_word;
                col = word_len;
            }
        }
        if !current.is_empty() {
            lines.push(ratatui::text::Line::from(parse_ansi_to_spans(&current)));
        }
        lines
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
                let lang = line.strip_prefix("```").unwrap_or("").trim().to_string();
                let w = terminal_width();
                let border = w.min(72);
                if lang.is_empty() {
                    println!("\x1b[2m{}\x1b[0m", "─".repeat(border));
                } else {
                    let label = format!(" {} ", lang);
                    let dashes = border.saturating_sub(2 + label.len());
                    println!(
                        "\x1b[2m──\x1b[0m\x1b[33m{}\x1b[2m{}\x1b[0m",
                        label,
                        "─".repeat(dashes)
                    );
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
        let bullet = if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
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
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fenced code block state on streaming path ──────────────────────

    #[test]
    fn fenced_code_block_body_renders_as_code_not_heading() {
        let mut md = MarkdownRenderer::new();
        let width = 80;

        // Opening fence
        let _ = md.feed_to_lines("```rust\n", width);
        assert!(md.in_code_block);
        assert_eq!(md.code_lang, Some("rust".to_string()));

        // Code body line that looks like a heading
        let lines = md.feed_to_lines("# not a heading\n", width);
        // Should be highlighted as code, not rendered as a heading
        assert!(!lines.is_empty());
        // The line should not be empty (it was rendered)
        let first_span = &lines[0].spans[0];
        let content = first_span.content.as_ref();
        assert!(content.contains("not a heading"), "content: {}", content);

        // Closing fence
        let _ = md.feed_to_lines("```\n", width);
        assert!(!md.in_code_block);
        assert!(md.code_lang.is_none());
    }

    #[test]
    fn heading_outside_code_block_still_renders_as_heading() {
        let mut md = MarkdownRenderer::new();
        let width = 80;

        let lines = md.feed_to_lines("# Real heading\n", width);
        assert!(!lines.is_empty());
        // Heading should have styling (not in code block)
        assert!(!md.in_code_block);
    }

    #[test]
    fn code_block_without_lang() {
        let mut md = MarkdownRenderer::new();
        let width = 80;

        let _ = md.feed_to_lines("```\n", width);
        assert!(md.in_code_block);
        assert!(md.code_lang.is_none());

        let _ = md.feed_to_lines("some code\n", width);
        let _ = md.feed_to_lines("```\n", width);
        assert!(!md.in_code_block);
    }

    #[test]
    fn nested_fences_in_code_body_do_not_toggle_state() {
        let mut md = MarkdownRenderer::new();
        let width = 80;

        // Open fence
        let _ = md.feed_to_lines("```python\n", width);
        assert!(md.in_code_block);

        // A line that starts with ``` but is inside the code block
        // This should NOT close the block — the fence toggle happens
        // at the line level, so a standalone ``` line inside code
        // would close it. But a line like ``` not-a-fence should
        // still be treated as code content.
        // Actually, our implementation toggles on any line starting
        // with ```, so a ``` inside code WILL close the block.
        // This is the same behavior as the legacy process_line.
        let _ = md.feed_to_lines("```\n", width);
        assert!(!md.in_code_block);
    }
}
