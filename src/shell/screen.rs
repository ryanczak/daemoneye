//! Live screen model for a shell: a `vt100::Parser` wrapper that turns the
//! raw PTY byte stream into what a human at that terminal would see.
//!
//! The viewport/transcript split this module rests on: the screen is the
//! *viewport* (what is on the terminal right now) and the phase-03 cast log
//! is the *transcript* (the byte-exact record of everything that happened).
//! Only the former lives here.

/// Semantic colour tag produced from grid cells by [`ShellScreen::annotated`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellColor {
    Red,
    Yellow,
    Green,
}

/// Map a grid cell's foreground colour to a semantic tag, per F1.
///
/// `Idx(1)`/`Idx(9)` are the `31`/`91` codes → ERROR-red, `Idx(3)`/`Idx(11)`
/// the `33`/`93` → WARN-yellow, `Idx(2)`/`Idx(10)` the `32`/`92` → OK-green.
/// Everything else — `Default`, any other `Idx(n)`, every `Rgb(..)` — is
/// unlabelled text.
fn cell_color(c: vt100::Color) -> Option<CellColor> {
    match c {
        vt100::Color::Idx(1) | vt100::Color::Idx(9) => Some(CellColor::Red),
        vt100::Color::Idx(3) | vt100::Color::Idx(11) => Some(CellColor::Yellow),
        vt100::Color::Idx(2) | vt100::Color::Idx(10) => Some(CellColor::Green),
        _ => None,
    }
}

fn marker_label(color: CellColor) -> &'static str {
    match color {
        CellColor::Red => "ERROR",
        CellColor::Yellow => "WARN",
        CellColor::Green => "OK",
    }
}

/// A live view of one shell's terminal: what a human at that terminal would
/// see right now. The transcript of record is the cast log, not this.
pub struct ShellScreen {
    parser: vt100::Parser,
}

impl ShellScreen {
    /// Create an empty screen `rows` high by `cols` wide; `scrollback` sizes
    /// the scrollback buffer and is passed through to `vt100::Parser::new`.
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, scrollback),
        }
    }

    /// Feed raw bytes from the PTY read. Takes bytes, not a `String`, because
    /// a PTY read is bytes and may split an escape sequence; `vt100` buffers
    /// partial sequences across calls, so feeding in arbitrary chunks is safe.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// The visible screen as plain text (F2) — what the terminal shows now.
    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// Escape codes that redraw this screen exactly on a fresh terminal of
    /// the same size: contents, colours, cursor and input modes
    /// (`vt100::Screen::state_formatted`). An alt-screen app is re-entered
    /// first, since `state_formatted` does not carry that mode; the primary
    /// screen underneath it is not sent, so a client sees a blank screen if
    /// the app later exits.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.parser.screen().alternate_screen() {
            out.extend_from_slice(b"\x1b[?1049h");
        }
        out.extend(self.parser.screen().state_formatted());
        out
    }

    /// `(rows, cols)` of the terminal.
    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// `(row, col)` of the cursor, 0-indexed.
    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// True while a full-screen program owns the terminal (F4), i.e. between
    /// the `ESC[?1049h` / `ESC[?1049l` pair that `less` and `vim` emit.
    pub fn is_alt_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// The visible screen with semantically-coloured runs wrapped in markers:
    /// red → `[ERROR: …]`, yellow → `[WARN: …]`, green → `[OK: …]`.
    pub fn annotated(&self) -> String {
        self.marked(true)
    }

    /// The visible screen with selection markers: every run of cells in
    /// inverse video is wrapped `[SEL: …]`, and — only when `annotated` —
    /// the semantically-coloured runs `annotated()` wraps today are wrapped
    /// on top (`red → [ERROR: …]`, yellow → `[WARN: …]`, green → `[OK: …]`).
    pub fn marked(&self, annotated: bool) -> String {
        let (rows, cols) = self.size();
        let mut out: Vec<String> = Vec::with_capacity(usize::from(rows));
        for row in 0..rows {
            out.push(self.marked_row(row, cols, annotated));
        }
        // A trailing run of entirely-empty rows yields empty strings; drop them
        // but keep interior empty lines (they separate legitimately empty rows).
        while out.last().is_some_and(|r| r.is_empty()) {
            out.pop();
        }
        out.join("\n")
    }

    /// The last `n` rows that scrolled off the top of the screen, as plain
    /// text — vt100's scrollback, oldest line first. `n` larger than what is
    /// kept returns what is kept. The parser's scrollback offset is moved and
    /// restored under the same borrow, so the viewport is untouched.
    pub fn scrollback_rows(&mut self, n: usize) -> String {
        if n == 0 {
            return String::new();
        }
        let (_, cols) = self.size();
        let screen = self.parser.screen_mut();
        // At scrollback offset `o`, `rows()`'s first item is the oldest
        // scrollback row still above the viewport (`o` rows up from the
        // bottom); walking `o` from `usize::MAX` (clamped to the buffer) down
        // to 1 therefore yields the kept scrollback rows oldest first. Each
        // step re-borrows the screen; the offset is restored to 0 at the end,
        // so the viewport this function leaves behind is the live one.
        screen.set_scrollback(usize::MAX);
        let kept = screen.scrollback();
        let mut lines: Vec<String> = Vec::with_capacity(kept);
        for offset in (1..=kept).rev() {
            screen.set_scrollback(offset);
            if let Some(line) = screen
                .rows(0, cols)
                .map(|l| l.trim_end().to_string())
                .next()
            {
                lines.push(line);
            }
        }
        screen.set_scrollback(0);
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    }

    fn marked_row(&self, row: u16, cols: u16, annotated: bool) -> String {
        let mut text = String::new();
        let mut color: Option<CellColor> = None;
        let mut inverse = false;
        let mut out = String::new();
        for col in 0..cols {
            let cell = match self.parser.screen().cell(row, col) {
                Some(cell) => cell,
                None => break,
            };
            if !cell.has_contents() {
                // Interior blanks are part of the row's visible layout (a
                // program may position columns with ESC[nC rather than write
                // spaces); trailing blanks are removed by flush_span's trim.
                text.push(' ');
                continue;
            }
            let ccolor = cell_color(cell.fgcolor());
            let cinv = cell.inverse();
            if ccolor != color || cinv != inverse {
                flush_span(&mut out, &mut text, color.take(), inverse, annotated);
                color = ccolor;
                inverse = cinv;
            }
            text.push_str(cell.contents());
        }
        flush_span(&mut out, &mut text, color.take(), inverse, annotated);
        out
    }
}

/// Flush an accumulated run of cells to `out`: an inverse run is wrapped
/// `[SEL: …]` regardless of colour, then — when `annotated` — the label for
/// a semantically-coloured run is added on top (`red → [ERROR: …]`). Runs
/// that trim to nothing are dropped.
///
/// Nested markers wrap outside-in: the inverse span is the outer marker, so
/// a red *and* inverse run reads `[SEL: [ERROR: …]]`. `annotated()` only
/// sees colour runs, so its output is exactly what it always was.
fn flush_span(
    out: &mut String,
    span_buf: &mut String,
    color: Option<CellColor>,
    inverse: bool,
    annotated: bool,
) {
    let text = span_buf.trim().to_string();
    if !text.is_empty() {
        match (inverse, color) {
            (true, Some(color)) if annotated => {
                let label = marker_label(color);
                out.push_str(&format!("[SEL: [{label}: {text}]]"));
            }
            (true, _) => out.push_str(&format!("[SEL: {text}]")),
            (false, Some(color)) if annotated => {
                let label = marker_label(color);
                out.push_str(&format!("[{label}: {text}]"));
            }
            (false, _) => out.push_str(&text),
        }
    }
    span_buf.clear();
}

impl ShellScreen {
    /// `<status> — <last non-empty line>`, the same shape the pane summaries use.
    pub fn summary(
        &self,
        dead: bool,
        dead_status: Option<i32>,
        has_bell: bool,
        current_cmd: &str,
        last_activity: u64,
        now: u64,
    ) -> String {
        let status = crate::shell::status::classify(
            dead,
            dead_status,
            has_bell,
            current_cmd,
            last_activity,
            now,
        );
        crate::shell::status::summarize(status, &self.contents())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: u16, cols: u16) -> ShellScreen {
        ShellScreen::new(rows, cols, 100)
    }

    #[test]
    fn snapshot_reproduces_the_screen() {
        let mut s = ShellScreen::new(24, 80, 100);
        s.feed(b"user@host:~$ ls\r\n\x1b[1;34mdir\x1b[0m  file.txt\r\nuser@host:~$ ");
        let mut p = vt100::Parser::new(24, 80, 100);
        p.process(&s.snapshot());
        assert_eq!(
            p.screen().contents_formatted(),
            s.parser.screen().contents_formatted(),
            "contents_formatted must round-trip"
        );
        assert_eq!(
            p.screen().cursor_position(),
            s.parser.screen().cursor_position(),
            "cursor position must round-trip"
        );
    }

    #[test]
    fn snapshot_keeps_an_alt_screen_app_in_alt_mode() {
        let mut s = ShellScreen::new(24, 80, 100);
        s.feed(b"before\r\n\x1b[?1049h\x1b[H\x1b[2J\x1b[7m top bar \x1b[0m\x1b[5;10Hin vim");
        assert!(s.parser.screen().alternate_screen());
        let mut p = vt100::Parser::new(24, 80, 100);
        p.process(&s.snapshot());
        assert!(
            p.screen().alternate_screen(),
            "snapshot must re-enter the alt screen"
        );
        assert_eq!(p.screen().contents(), s.parser.screen().contents());
    }

    #[test]
    fn screen_annotates_a_red_run_as_one_error_marker() {
        let mut s = screen(2, 60);
        s.feed(b"\x1b[31mdisk failure");
        let out = s.annotated();
        assert_eq!(out, "[ERROR: disk failure]");
        assert_eq!(out.matches("[ERROR:").count(), 1);
    }

    #[test]
    fn screen_maps_all_six_colour_codes() {
        let cases = [
            (b"\x1b[31mA".as_slice(), "[ERROR: A]"),
            (b"\x1b[91mA".as_slice(), "[ERROR: A]"),
            (b"\x1b[33mA".as_slice(), "[WARN: A]"),
            (b"\x1b[93mA".as_slice(), "[WARN: A]"),
            (b"\x1b[32mA".as_slice(), "[OK: A]"),
            (b"\x1b[92mA".as_slice(), "[OK: A]"),
        ];
        for (bytes, expected) in cases {
            let mut s = screen(2, 60);
            s.feed(bytes);
            assert_eq!(s.annotated(), *expected, "for input {:?}", bytes);
        }
    }

    #[test]
    fn screen_leaves_unmapped_colours_as_plain_text() {
        let mut s = screen(2, 200);
        s.feed(b"\x1b[34mblue \x1b[38;5;200mindexed \x1b[38;2;10;20;30mtruecolour \x1b[0mreset");
        let out = s.annotated();
        assert!(!out.contains("[ERROR:"), "output: {out:?}");
        assert!(!out.contains("[WARN:"), "output: {out:?}");
        assert!(!out.contains("[OK:]"), "output: {out:?}");
        assert!(out.contains("blue"), "output: {out:?}");
        assert!(out.contains("indexed"), "output: {out:?}");
        assert!(out.contains("truecolour"), "output: {out:?}");
        assert!(out.contains("reset"), "output: {out:?}");
    }

    #[test]
    fn screen_does_not_merge_a_colour_run_across_a_row_boundary() {
        // 4 columns: the same red run fills row 0 then continues at column 0
        // of row 1. Rows are separate, so this must be two markers, not one
        // spanning the newline — the colour does not change, only the row does.
        let mut s = screen(2, 4);
        s.feed(b"\x1b[31mabcdefgh");
        let out = s.annotated();
        assert_eq!(out, "[ERROR: abcd]\n[ERROR: efgh]", "rows must not merge");
    }

    #[test]
    fn screen_preserves_cursor_positioned_columns() {
        // Columns laid out with ESC[nC, not literal spaces — the grid holds
        // the gap and annotated() must render it the same way contents() does.
        let mut s = screen(1, 40);
        s.feed(b"NAME\x1b[6CSIZE\x1b[6CMODE");
        let annotated = s.annotated();
        let contents = s.contents();
        assert_eq!(
            annotated, contents,
            "annotated() must agree with contents() on column positions"
        );
        assert!(annotated.starts_with("NAME"), "annotated: {annotated:?}");
        assert!(annotated.contains("SIZE"), "annotated: {annotated:?}");
    }

    #[test]
    fn screen_trims_marker_text_and_drops_empty_runs() {
        let mut s = screen(2, 60);
        s.feed(b" \x1b[31m   \x1b[0m ");
        assert_eq!(s.annotated(), "");
    }

    #[test]
    fn screen_trims_marker_text_and_drops_empty_runs2() {
        let mut s = screen(2, 60);
        s.feed(b"\x1b[31m  hi  ");
        assert_eq!(s.annotated(), "[ERROR: hi]");
    }

    #[test]
    fn screen_contents_is_the_visible_screen_not_the_scrollback() {
        let mut s = screen(3, 10);
        for i in 1..=8 {
            let line = format!("line{}\r\n", i);
            s.feed(line.as_bytes());
        }
        let contents = s.contents();
        assert!(!contents.contains("line1"), "the viewport must drop line1");
        assert!(
            contents.contains("line8"),
            "the viewport keeps the last line"
        );
        // 3 rows: 8 lines scrolled through it, so only the last two survive
        // (a CRLF on the last line still leaves the final row empty).
        assert_eq!(contents, "line7\nline8");
    }

    #[test]
    fn marked_wraps_an_inverse_run_as_sel() {
        let mut s = screen(2, 40);
        s.feed(b"\x1b[7mselected item\x1b[0m");
        assert_eq!(s.marked(false), "[SEL: selected item]");
        assert_eq!(s.marked(true), "[SEL: selected item]");
    }

    #[test]
    fn marked_leaves_plain_text_unmarked() {
        let mut s = screen(2, 40);
        s.feed(b"plain text only\r\nsecond");
        assert_eq!(s.marked(false), "plain text only\nsecond");
        assert_eq!(s.marked(false), s.contents());
    }

    #[test]
    fn marked_with_annotated_also_wraps_colour_runs() {
        let mut s = screen(2, 40);
        s.feed(b"\x1b[7mtop\x1b[0m \x1b[31mred tail\x1b[0m");
        let out = s.marked(true);
        assert_eq!(out, "[SEL: top][ERROR: red tail]", "annotated: {out:?}");
        let plain = s.marked(false);
        assert_eq!(plain, "[SEL: top]red tail", "plain: {plain:?}");
        let annotated = s.annotated();
        assert_eq!(annotated, "[SEL: top][ERROR: red tail]");
    }

    #[test]
    fn scrollback_rows_return_what_scrolled_off() {
        let mut s = ShellScreen::new(3, 20, 100);
        for i in 1..=8 {
            let line = format!("line{i}\r\n");
            s.feed(line.as_bytes());
        }
        // 3 visible rows with 8 lines pushed through: the scrollback keeps
        // the six rows that scrolled off (line1..line6); line7/line8/blank
        // are the viewport.
        let all = s.scrollback_rows(100);
        assert_eq!(
            all, "line1\nline2\nline3\nline4\nline5\nline6",
            "all: {all:?}"
        );
        let last_two = s.scrollback_rows(2);
        assert_eq!(last_two, "line5\nline6", "last two: {last_two:?}");
        assert_eq!(s.scrollback_rows(0), "", "no scrollback rows requested");
        assert_eq!(s.contents(), "line7\nline8", "viewport untouched");
        assert_eq!(s.contents(), "line7\nline8", "still untouched");
    }

    #[test]
    fn screen_reports_the_alternate_screen() {
        let mut s = screen(3, 20);
        assert!(!s.is_alt_screen());
        s.feed(b"\x1b[?1049h");
        assert!(s.is_alt_screen());
        s.feed(b"\x1b[?1049l");
        assert!(!s.is_alt_screen());
    }

    #[test]
    fn screen_summary_uses_the_shared_classifier() {
        let mut s = screen(3, 40);
        s.feed(b"cargo check\n\x1b[32mFinished\x1b[0m\r\n");
        let contents = s.contents();
        let status = crate::shell::status::classify(false, None, false, "vim", 0, 0);
        let expected = crate::shell::status::summarize(status, &contents);
        let actual = s.summary(false, None, false, "vim", 0, 0);
        assert_eq!(actual, expected);
        assert!(actual.ends_with("Finished"), "summary: {actual:?}");
    }

    #[test]
    fn screen_feeds_bytes_split_mid_sequence() {
        let mut whole = screen(2, 60);
        whole.feed(b"\x1b[31mred");
        let mut split = screen(2, 60);
        split.feed(b"\x1b[3");
        split.feed(b"1mred");
        assert_eq!(whole.annotated(), split.annotated());
        assert_eq!(split.annotated(), "[ERROR: red]");
    }
}
