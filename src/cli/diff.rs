use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};

const CONTEXT_LINES: usize = 3;
const NEW_FILE_LINE_CAP: usize = 80;

/// The prompt's indent: two spaces before every diff row.
const INDENT: &str = "  ";

/// One diff view: the styled rows the approval prompt commits inline.
///
/// `rows` is capped for a new file (80 content rows + a `ctrl+o` footer);
/// `full` is the same view uncapped, for the Ctrl+O transcript viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffView {
    pub rows: Vec<Line<'static>>,
    pub full: Vec<Line<'static>>,
}

/// Renders the approval diff as styled rows (no ANSI escapes — the styles are
/// real `Style`s, so `commit_styled` renders them as cell attributes).
///
/// - `existing_content = None`  → new file: all lines shown as green `+`
/// - `existing_content = Some`  → modified file: standard unified diff
///
/// The header carries the counts a reader needs to judge the change: a new
/// file's line count, a modification's added/removed totals.
pub(crate) fn render_diff(
    name: &str,
    existing_content: Option<&str>,
    new_content: &str,
) -> DiffView {
    match existing_content {
        None => render_new_file(name, new_content),
        Some(old) => render_unified(name, old, new_content),
    }
}

fn header(name: &str, text: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(
            format!("{text}{name}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ])
}

fn dim_indent(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(
            text.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])
}

fn content_row(prefix: &str, text: &str, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(format!("{prefix}{text}"), style),
    ])
}

fn render_new_file(name: &str, content: &str) -> DiffView {
    let green = Style::default().fg(Color::Green);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let shown = total.min(NEW_FILE_LINE_CAP);

    let mut full = vec![header(&format!("{name} ({total} lines)"), "new file: ")];
    for line in &lines {
        full.push(content_row("+", line, green));
    }

    let mut rows = Vec::with_capacity(shown + 2);
    rows.push(full[0].clone());
    rows.extend(full[1..=shown].iter().cloned());
    if total > shown {
        rows.push(dim_indent(&format!(
            "… ({more} more lines — ctrl+o to view all)",
            more = total - shown
        )));
    }

    DiffView { rows, full }
}

fn render_unified(name: &str, old: &str, new: &str) -> DiffView {
    let green = Style::default().fg(Color::Green);
    let red = Style::default().fg(Color::Red);
    let cyan = Style::default().fg(Color::Cyan);

    let diff = TextDiff::from_lines(old, new);

    let has_changes = diff
        .ops()
        .iter()
        .any(|op| !matches!(op.tag(), similar::DiffTag::Equal));

    if !has_changes {
        let row = Line::from(vec![
            Span::raw(INDENT),
            Span::styled(
                format!("no changes: {name}"),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]);
        return DiffView {
            rows: vec![row.clone()],
            full: vec![row],
        };
    }

    let added = diff
        .iter_all_changes()
        .filter(|c| c.tag() == ChangeTag::Insert)
        .count();
    let removed = diff
        .iter_all_changes()
        .filter(|c| c.tag() == ChangeTag::Delete)
        .count();

    let head = header(&format!("{name} (+{added} -{removed})"), "modified: ");

    let mut rows: Vec<Line<'static>> = vec![head.clone()];
    let mut full: Vec<Line<'static>> = vec![head];
    let mut first_hunk = true;

    for group in diff.grouped_ops(CONTEXT_LINES) {
        if !first_hunk {
            rows.push(Line::from(Span::raw("")));
            full.push(Line::from(Span::raw("")));
        }
        first_hunk = false;

        let old_start = group
            .first()
            .map(|op| op.old_range().start + 1)
            .unwrap_or(1);
        let old_end = group
            .last()
            .map(|op| op.old_range().end)
            .unwrap_or(old_start);
        let new_start = group
            .first()
            .map(|op| op.new_range().start + 1)
            .unwrap_or(1);
        let new_end = group
            .last()
            .map(|op| op.new_range().end)
            .unwrap_or(new_start);

        let hunk = content_row(
            "",
            &format!(
                "@@ -{},{} +{},{} @@",
                old_start,
                old_end.saturating_sub(old_start - 1),
                new_start,
                new_end.saturating_sub(new_start - 1),
            ),
            cyan,
        );
        rows.push(hunk.clone());
        full.push(hunk);

        for op in &group {
            for change in diff.iter_changes(op) {
                let line = change.value().trim_end_matches('\n');
                let row = match change.tag() {
                    ChangeTag::Delete => content_row("-", line, red),
                    ChangeTag::Insert => content_row("+", line, green),
                    ChangeTag::Equal => content_row(" ", line, Style::default()),
                };
                rows.push(row.clone());
                full.push(row);
            }
        }
    }

    DiffView { rows, full }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn plain_rows(rows: &[Line<'static>]) -> Vec<String> {
        rows.iter().map(row_text).collect()
    }

    #[test]
    fn new_file_all_green() {
        let view = render_diff("foo.sh", None, "#!/bin/bash\necho hello\n");
        assert!(row_text(&view.rows[0]).contains("new file: foo.sh"));
        for row in &view.full[1..] {
            let text = row_text(row).trim_start_matches("  ").to_string();
            assert!(text.starts_with('+'), "a new file row is a + row: {text:?}");
        }
    }

    #[test]
    fn new_file_cap_at_80_lines() {
        let content: String = (0..100).map(|i| format!("line {}\n", i)).collect();
        let view = render_diff("big.sh", None, &content);
        // 1 header + 80 content rows + 1 footer
        assert_eq!(view.rows.len(), 82, "inline rows: {:?}", view.rows.len());
        let footer = row_text(view.rows.last().unwrap());
        assert!(footer.contains("20 more lines"), "{footer:?}");
        // Full form is uncapped: 1 header + 100 content rows
        assert_eq!(view.full.len(), 101);
    }

    #[test]
    fn identical_content_no_changes() {
        let content = "#!/bin/bash\necho hi\n";
        let view = render_diff("same.sh", Some(content), content);
        assert_eq!(view.rows.len(), 1);
        assert!(row_text(&view.rows[0]).contains("no changes"));
    }

    #[test]
    fn single_line_change() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nLINE2\nline3\n";
        let view = render_diff("f.sh", Some(old), new);
        let plain = plain_rows(&view.rows);
        assert!(plain.iter().any(|l| l.contains("@@")));
        assert!(plain.iter().any(|l| l.contains("-line2")));
        assert!(plain.iter().any(|l| l.contains("+LINE2")));
    }

    #[test]
    fn multi_hunk_diff() {
        let old = (0..20).map(|i| format!("line{}\n", i)).collect::<String>();
        let mut new_lines: Vec<String> = (0..20).map(|i| format!("line{}\n", i)).collect();
        new_lines[0] = "CHANGED\n".to_string();
        new_lines[19] = "ALSO_CHANGED\n".to_string();
        let new = new_lines.join("");
        let view = render_diff("m.sh", Some(&old), &new);
        let plain = plain_rows(&view.rows);
        let hunk_count = plain.iter().filter(|l| l.contains("@@")).count();
        assert_eq!(hunk_count, 2);
    }

    #[test]
    fn empty_to_nonempty() {
        let view = render_diff("new.sh", Some(""), "hello\n");
        let plain = plain_rows(&view.rows);
        assert!(plain.iter().any(|l| l.contains("+hello")));
    }

    #[test]
    fn nonempty_to_empty() {
        let view = render_diff("del.sh", Some("hello\n"), "");
        let plain = plain_rows(&view.rows);
        assert!(plain.iter().any(|l| l.contains("-hello")));
    }

    // ── approval_diff (M36 phase-06, B7) ────────────────────────────────

    #[test]
    fn approval_diff_rows_carry_styles_not_escapes() {
        let new_view = render_diff("new.sh", None, "#!/bin/sh\necho hi\n");
        let mod_view = render_diff("edit.sh", Some("a\nb\nc\n"), "a\nB\nc\n");
        for view in [&new_view, &mod_view] {
            for row in &view.rows {
                let text = row_text(row);
                assert!(!text.contains('\x1b'), "a row holds ESC: {text:?}");
                assert!(!text.contains("[3"), "a row holds SGR text: {text:?}");
            }
        }
        let header_style = new_view.rows[0].spans[1].style;
        assert!(
            header_style.add_modifier.contains(Modifier::BOLD),
            "the header is bold: {header_style:?}"
        );
        let plus = |rows: &[Line<'static>], needle: &str| {
            rows.iter()
                .find(|r| row_text(r).contains(needle))
                .unwrap_or_else(|| panic!("no row holds {needle:?}"))
                .spans[1]
                .style
        };
        assert_eq!(
            plus(&new_view.rows, "+echo hi").fg,
            Some(Color::Green),
            "+ rows are green"
        );
        assert_eq!(
            plus(&mod_view.rows, "-b").fg,
            Some(Color::Red),
            "- rows are red"
        );
        assert_eq!(
            plus(&mod_view.rows, "+B").fg,
            Some(Color::Green),
            "+ rows are green"
        );
        assert_eq!(
            plus(&mod_view.rows, "@@").fg,
            Some(Color::Cyan),
            "@@ headers are cyan"
        );
        assert_eq!(
            plus(&mod_view.rows, " a"),
            Style::default(),
            "context rows are unstyled"
        );
    }

    #[test]
    fn approval_diff_header_counts_lines() {
        let content: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let view = render_diff("x.sh", None, &content);
        assert!(
            row_text(&view.rows[0]).contains("new file: x.sh (120 lines)"),
            "a new file's header counts its lines: {:?}",
            row_text(&view.rows[0])
        );

        let view = render_diff(
            "x.sh",
            Some("one\ntwo\nthree\nfour\n"),
            "one\nTWO-a\nTWO-b\nthree\nfour\n",
        );
        assert!(
            row_text(&view.rows[0]).contains("modified: x.sh (+2 -1)"),
            "a modified file's header counts added and removed: {:?}",
            row_text(&view.rows[0])
        );
    }

    #[test]
    fn approval_diff_clipped_footer_names_key() {
        let content: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let view = render_diff("x.sh", None, &content);
        let plus_rows = view
            .rows
            .iter()
            .filter(|r| row_text(r).contains("+line "))
            .count();
        assert_eq!(plus_rows, 80, "the inline cap stays at 80 rows");
        let footer = row_text(view.rows.last().unwrap());
        assert!(
            footer.contains("40 more lines") && footer.contains("ctrl+o"),
            "the footer names the key: {footer:?}"
        );

        let small: String = (0..10).map(|i| format!("line {i}\n")).collect();
        let view = render_diff("y.sh", None, &small);
        assert!(
            view.rows
                .iter()
                .all(|r| !row_text(r).contains("more lines")),
            "a short file has no footer"
        );
    }

    /// The full, uncapped diff the Ctrl+O viewer shows: every content row.
    #[test]
    fn approval_diff_full_is_uncapped() {
        let content: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let view = render_diff("x.sh", None, &content);
        let content_rows = view
            .full
            .iter()
            .filter(|r| row_text(r).contains("+line "))
            .count();
        assert_eq!(
            content_rows, 120,
            "the viewer's diff holds all 120 lines, uncapped: {content_rows} rows"
        );
    }
}
