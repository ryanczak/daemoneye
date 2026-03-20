/// Semantic color tag produced by [`annotate_ansi`].
#[derive(Clone, Copy, PartialEq)]
enum SpanColor {
    Red,
    Yellow,
    Green,
}

/// Classify an SGR parameter string (the content between `\x1b[` and `m`).
///
/// Returns `Some(color)` when a foreground colour code is present, `None`
/// when the sequence is a reset or a non-colour attribute (bold, italic, …).
fn classify_sgr(params: &str) -> Option<SpanColor> {
    let mut color: Option<SpanColor> = None;
    for part in params.split(';') {
        match part {
            "31" | "91" => color = Some(SpanColor::Red),
            "32" | "92" => color = Some(SpanColor::Green),
            "33" | "93" => color = Some(SpanColor::Yellow),
            _ => {}
        }
    }
    // An explicit reset in the same sequence (e.g. `\x1b[0;31m`) is treated
    // as a colour-change rather than a reset so colour wins.
    color
}

/// Flush an accumulated colour span to `out` with the appropriate label.
fn flush_span(out: &mut String, span_buf: &mut String, color: SpanColor) {
    let text = span_buf.trim();
    if !text.is_empty() {
        let label = match color {
            SpanColor::Red => "ERROR",
            SpanColor::Yellow => "WARN",
            SpanColor::Green => "OK",
        };
        out.push_str(&format!("[{}: {}]", label, text));
    }
    span_buf.clear();
}

/// Convert ANSI SGR colour escapes in terminal output to semantic markers (R2).
///
/// * Red foreground (31, 91)    → `[ERROR: text]`
/// * Yellow foreground (33, 93) → `[WARN: text]`
/// * Green foreground (32, 92)  → `[OK: text]`
///
/// All other CSI / OSC sequences (cursor movement, bold, underline, …) are
/// stripped.  `\r\n` and lone `\r` are normalised to `\n`.
pub(super) fn annotate_ansi(s: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    // First branch captures SGR params (ESC [ <digits/semicolons> m).
    // Remaining branches match other escape sequences that should be stripped.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"\x1b\[([0-9;]*)m",                    // group 1: SGR
            r"|\x1b\[[0-9;?<=>!]*[A-Za-z]",         // other CSI
            r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?", // OSC
            r"|\x1b[PX\^_][^\x1b]*\x1b\\",          // DCS/SOS/PM/APC
            r"|\x1b[()][A-Za-z0-9]",                // Charset
            r"|\x1b.",                              // lone ESC + 1 byte
        ))
        .expect("annotate_ansi regex is valid")
    });

    let mut result = String::with_capacity(s.len());
    let mut current_color: Option<SpanColor> = None;
    let mut span_buf = String::new();
    let mut last_end = 0usize;

    for cap in re.captures_iter(s) {
        let m = cap.get(0).unwrap();
        let plain = &s[last_end..m.start()];
        if !plain.is_empty() {
            match current_color {
                Some(_) => span_buf.push_str(plain),
                None => result.push_str(plain),
            }
        }
        last_end = m.end();

        if let Some(sgr_params) = cap.get(1) {
            // This is an SGR sequence.
            match classify_sgr(sgr_params.as_str()) {
                Some(new_color) => {
                    if current_color.is_some() && current_color != Some(new_color) {
                        // Colour change mid-span: flush old span first.
                        flush_span(&mut result, &mut span_buf, current_color.unwrap());
                    }
                    current_color = Some(new_color);
                }
                None => {
                    // Reset or non-colour SGR: close any open span.
                    if let Some(c) = current_color {
                        flush_span(&mut result, &mut span_buf, c);
                        current_color = None;
                    }
                }
            }
        }
        // Non-SGR escapes: stripped (not added to output).
    }

    // Flush any remaining plain text after the last escape.
    let tail = &s[last_end..];
    if !tail.is_empty() {
        match current_color {
            Some(_) => span_buf.push_str(tail),
            None => result.push_str(tail),
        }
    }
    // Close an open span that wasn't terminated with a reset.
    if let Some(c) = current_color {
        flush_span(&mut result, &mut span_buf, c);
    }

    result.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::annotate_ansi;

    #[test]
    fn annotate_ansi_plain_text_unchanged() {
        assert_eq!(annotate_ansi("hello world"), "hello world");
    }

    #[test]
    fn annotate_ansi_red_becomes_error() {
        // ESC[31m = red fg, ESC[0m = reset
        assert_eq!(
            annotate_ansi("\x1b[31mfailed to connect\x1b[0m"),
            "[ERROR: failed to connect]"
        );
    }

    #[test]
    fn annotate_ansi_bright_red_becomes_error() {
        assert_eq!(annotate_ansi("\x1b[91mERROR\x1b[0m"), "[ERROR: ERROR]");
    }

    #[test]
    fn annotate_ansi_yellow_becomes_warn() {
        assert_eq!(
            annotate_ansi("\x1b[33mDeprecated API\x1b[0m"),
            "[WARN: Deprecated API]"
        );
    }

    #[test]
    fn annotate_ansi_bright_yellow_becomes_warn() {
        assert_eq!(
            annotate_ansi("\x1b[93mwarning: unused variable\x1b[0m"),
            "[WARN: warning: unused variable]"
        );
    }

    #[test]
    fn annotate_ansi_green_becomes_ok() {
        assert_eq!(
            annotate_ansi("\x1b[32mAll tests passed\x1b[0m"),
            "[OK: All tests passed]"
        );
    }

    #[test]
    fn annotate_ansi_mixed_colours() {
        let input = "\x1b[32mOK\x1b[0m some text \x1b[31mERR\x1b[0m";
        assert_eq!(annotate_ansi(input), "[OK: OK] some text [ERROR: ERR]");
    }

    #[test]
    fn annotate_ansi_bold_attribute_stripped() {
        // Bold (1) is not a colour — span stays open if colour was already set.
        // Bold alone: just strip the escape.
        assert_eq!(annotate_ansi("\x1b[1mBold text\x1b[0m"), "Bold text");
    }

    #[test]
    fn annotate_ansi_bold_plus_color() {
        // \x1b[1;31m = bold red — should annotate as ERROR
        assert_eq!(
            annotate_ansi("\x1b[1;31mCRITICAL\x1b[0m"),
            "[ERROR: CRITICAL]"
        );
    }

    #[test]
    fn annotate_ansi_no_reset_at_eof() {
        // Span not closed with explicit reset — should still emit marker
        assert_eq!(annotate_ansi("\x1b[31mno reset"), "[ERROR: no reset]");
    }

    #[test]
    fn annotate_ansi_cursor_movement_stripped() {
        // CSI cursor-up stripped; \r\n normalised to \n
        assert_eq!(annotate_ansi("a\x1b[1Ab\r\nc"), "ab\nc");
    }

    #[test]
    fn annotate_ansi_osc_title_stripped() {
        assert_eq!(annotate_ansi("\x1b]0;user@host\x07hello"), "hello");
    }

    #[test]
    fn annotate_ansi_plain_text_between_spans() {
        let input = "prefix \x1b[31merror msg\x1b[0m suffix";
        assert_eq!(annotate_ansi(input), "prefix [ERROR: error msg] suffix");
    }
}
