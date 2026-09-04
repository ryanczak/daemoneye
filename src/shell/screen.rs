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
        let (rows, cols) = self.size();
        let mut out: Vec<String> = Vec::with_capacity(usize::from(rows));
        for row in 0..rows {
            out.push(self.annotated_row(row, cols));
        }
        // A trailing run of entirely-empty rows yields empty strings; drop them
        // but keep interior empty lines (they separate legitimately empty rows).
        while out.last().is_some_and(|r| r.is_empty()) {
            out.pop();
        }
        out.join("\n")
    }

    fn annotated_row(&self, row: u16, cols: u16) -> String {
        let mut text = String::new();
        let mut color: Option<CellColor> = None;
        let mut out = String::new();
        for col in 0..cols {
            let cell = match self.parser.screen().cell(row, col) {
                Some(cell) => cell,
                None => break,
            };
            if !cell.has_contents() {
                continue;
            }
            let ccolor = cell_color(cell.fgcolor());
            if ccolor != color {
                flush_span(&mut out, &mut text, color.take());
                color = ccolor;
            }
            text.push_str(cell.contents());
        }
        flush_span(&mut out, &mut text, color.take());
        out
    }
}

/// Flush an accumulated run of `color`-labelled cells to `out`, wrapping it
/// in the marker only when labelled; runs that trim to nothing are dropped.
fn flush_span(out: &mut String, span_buf: &mut String, color: Option<CellColor>) {
    let text = span_buf.trim().to_string();
    if !text.is_empty() {
        match color {
            Some(color) => out.push_str(&format!("[{}: {}]", marker_label(color), text)),
            None => out.push_str(&text),
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
        let status = crate::tmux::status::classify(
            dead,
            dead_status,
            has_bell,
            current_cmd,
            last_activity,
            now,
        );
        crate::tmux::status::summarize(status, &self.contents())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: u16, cols: u16) -> ShellScreen {
        ShellScreen::new(rows, cols, 100)
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
        // 4 columns: a red run fills row 0; a green run fills row 1. They must
        // NOT merge into one marker spanning the newline — rows are separate,
        // so this is two markers.
        let mut s = screen(2, 4);
        s.feed(b"\x1b[31mabcd\x1b[32mEFGH");
        let out = s.annotated();
        assert_eq!(out, "[ERROR: abcd]\n[OK: EFGH]", "rows must not merge");
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
        let status = crate::tmux::status::classify(false, None, false, "vim", 0, 0);
        let expected = crate::tmux::status::summarize(status, &contents);
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
