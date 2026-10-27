use super::tests::make_test_renderer;
use super::*;
use ratatui::backend::TestBackend;

// ── Helpers for spinner-row tests ──────────────────────────

/// Find the y-coordinate of the row containing the first '┌' (top-left
/// corner of the input box).
fn corner_row(buf: &ratatui::buffer::Buffer) -> u16 {
    let cols = buf.area.width;
    for y in 0..buf.area.height {
        for x in 0..cols {
            if buf[(x, y)].symbol() == "┌" {
                return y;
            }
        }
    }
    panic!("no '┌' found in buffer");
}

/// Collect an entire row's symbols into a single String.
fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    let cols = buf.area.width;
    let mut s = String::new();
    for x in 0..cols {
        s.push_str(buf[(x, y)].symbol());
    }
    s
}

#[test]
fn spinner_renders_above_input_box_not_inside_it() {
    let mut renderer = make_test_renderer();

    let status = StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };

    renderer.draw_spinner("(◉)", "scrying", 3, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let corner = corner_row(buf);

    // The spinner row is at corner_row - 1.
    let spinner_row = corner - 1;
    let spinner_text = row_text(buf, spinner_row);
    assert!(
        spinner_text.contains("scrying"),
        "spinner row should contain verb 'scrying', got: {:?}",
        spinner_text
    );
    assert!(
        spinner_text.contains("..."),
        "spinner row should contain dots '...', got: {:?}",
        spinner_text
    );
    assert!(
        spinner_text.contains('◉'),
        "spinner row should contain frame glyph '◉', got: {:?}",
        spinner_text
    );

    // Negative: the rows at and below the box corner must NOT contain
    // the verb — the spinner is outside the box.
    for y in corner..buf.area.height {
        let r = row_text(buf, y);
        assert!(
            !r.contains("scrying"),
            "row {} (at or below box) must not contain 'scrying', got: {:?}",
            y,
            r
        );
    }
}

#[test]
fn spinner_glyph_renders_at_column_zero() {
    let mut renderer = make_test_renderer();

    let status = StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };

    renderer.draw_spinner("(◉)", "scrying", 3, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let corner = corner_row(buf);
    let spinner_row = corner - 1;

    // The spinner glyph must start at column 0, not indented.
    let first_cell = &buf[(0, spinner_row)];
    assert_eq!(
        first_cell.symbol(),
        "(",
        "spinner glyph '(' must be at column 0, got: {:?}",
        first_cell.symbol()
    );
}

#[test]
fn input_box_row_is_stable_across_draw_modes() {
    let mut renderer = make_test_renderer();

    let status = StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };

    let mut input = InputLine::new();
    input.insert('H');
    input.insert('i');

    // Draw in normal mode.
    renderer.draw(&input, &status).unwrap();
    let buf1 = renderer.terminal.backend().buffer();
    let corner_draw = corner_row(buf1);

    // Draw in spinner mode.
    renderer.draw_spinner("(◉)", "scrying", 1, &status).unwrap();
    let buf2 = renderer.terminal.backend().buffer();
    let corner_spinner = corner_row(buf2);

    // Draw in prompt mode.
    renderer.draw_prompt("password:", &input, &status).unwrap();
    let buf3 = renderer.terminal.backend().buffer();
    let corner_prompt = corner_row(buf3);

    assert_eq!(
        corner_draw, corner_spinner,
        "box corner row must be the same in draw and spinner modes"
    );
    assert_eq!(
        corner_draw, corner_prompt,
        "box corner row must be the same in draw and prompt modes"
    );
}

#[test]
fn spinner_row_is_blank_when_idle() {
    let mut renderer = make_test_renderer();

    let mut input = InputLine::new();
    input.insert('H');
    input.insert('e');
    input.insert('l');
    input.insert('l');
    input.insert('o');

    let status = StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };

    renderer.draw(&input, &status).unwrap();

    let buf = renderer.terminal.backend().buffer();
    let corner = corner_row(buf);
    let spinner_row = corner - 1;
    let text = row_text(buf, spinner_row);

    assert!(
        text.chars().all(|c| c.is_whitespace()),
        "spinner row above box should be all whitespace when idle, got: {:?}",
        text
    );
}

#[test]
fn short_region_collapses_spinner_row() {
    // Build a renderer with a viewport shorter than MIN_HEIGHT_FOR_SPINNER_ROW.
    let backend = TestBackend::new(60, 10);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(4),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let status = StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };

    let input = InputLine::new();

    // Neither call should panic.
    renderer.draw(&input, &status).unwrap();
    renderer.draw_spinner("(◉)", "scrying", 1, &status).unwrap();

    // A box border should still be present.
    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    assert!(
        all_text.contains('┌'),
        "short region should still render the input box border, got: {}",
        all_text
    );
}

/// Bug-01-2 regression test: at region height 4, the spinner row collapses
/// to zero height, so the prompt must be rendered into a one-row strip
/// carved from the top of the body instead.
#[test]
fn prompt_region_at_height_four_does_not_lose_prompt() {
    // Build a renderer with a viewport shorter than MIN_HEIGHT_FOR_SPINNER_ROW.
    let backend = TestBackend::new(60, 10);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(4),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let status = StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };

    let input = InputLine::new();

    renderer.draw_prompt("password:", &input, &status).unwrap();

    // The prompt text must be visible in the buffer.
    let buf = renderer.terminal.backend().buffer();
    let all_text: String = buf
        .content
        .iter()
        .flat_map(|c| c.symbol().chars())
        .collect();
    assert!(
        all_text.contains("password:"),
        "prompt region at height 4 must render the prompt text, got: {}",
        all_text
    );
}

/// Helper: make a default StatusBarState for tests.
fn default_status() -> StatusBarState<'static> {
    StatusBarState {
        session_id: "abcdef12-3456",
        approval_hint: "cmds: auto",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    }
}

/// Cursor sits one cell right of the last glyph on a short (non-wrapped)
/// input, on the first visual row inside the input box.
#[test]
fn cursor_sits_on_next_free_cell_of_short_input() {
    let mut renderer = make_test_renderer();

    let mut input = InputLine::new();
    input.insert_str("hello"); // cursor at index 5

    renderer.draw(&input, &default_status()).unwrap();

    // Read cursor before borrowing buffer immutably
    let cursor = renderer
        .terminal
        .backend_mut()
        .get_cursor_position()
        .unwrap();

    let buf = renderer.terminal.backend().buffer();
    let box_top = corner_row(buf);

    // Cursor should be at (x: 1+5, y: box_top+1) — one cell right of 'o'
    assert_eq!(
        cursor.x,
        1 + 5,
        "cursor x should be at col 6 (after 'hello')"
    );
    assert_eq!(
        cursor.y,
        box_top + 1,
        "cursor y should be one row below box top"
    );

    // The cell at (1+4, box_top+1) should hold 'o' — the last glyph
    assert_eq!(
        buf[(1 + 4, box_top + 1)].symbol(),
        "o",
        "cell at col 5 should hold 'o'"
    );
}

/// When input wraps via visual_lines, the cursor and glyphs agree on the
/// wrap point. If visual_lines were called with a different width than
/// cursor_visual_pos, the cursor would land on a different row than the
/// glyphs display. 57 'a's + " b" (59 chars): at width 58 the " b" wraps
/// to row 1; at width 59 everything fits on row 0.
#[test]
fn cursor_matches_glyph_on_word_wrapped_input() {
    let mut renderer = make_test_renderer();

    // 57 'a's, a space, then 'b' = 59 chars total.
    // With inner width 58, visual_lines wraps:
    //   row 0 = 57 'a's
    //   row 1 = " b" (2 chars)
    // cursor_visual_pos(58) places cursor at (row 1, col 2).
    // If visual_lines were called with 59 instead, everything fits on row 0
    // but cursor_visual_pos(58) still says row 1 — cursor and glyphs disagree.
    let mut input = InputLine::new();
    input.insert_str(&"a".repeat(57));
    input.insert(' ');
    input.insert('b');

    renderer.draw(&input, &default_status()).unwrap();

    // Read cursor before borrowing buffer immutably
    let cursor = renderer
        .terminal
        .backend_mut()
        .get_cursor_position()
        .unwrap();

    let buf = renderer.terminal.backend().buffer();
    let box_top = corner_row(buf);

    // Cursor should be on visual row 1 (box_top + 2), col 2
    assert_eq!(
        cursor.y,
        box_top + 2,
        "cursor y should be on visual row 1 (box_top + 2), got {}",
        cursor.y
    );
    assert_eq!(
        cursor.x,
        1 + 2,
        "cursor x should be at col 3 (after ' b'), got {}",
        cursor.x
    );

    // The cell at (2, box_top+2) should hold 'b' — the 'b' on row 1
    // This is the key assertion: if visual_lines were called with a
    // different width, the 'b' would be on a different row.
    assert_eq!(
        buf[(2, box_top + 2)].symbol(),
        "b",
        "cell at col 2 on visual row 1 should hold 'b'"
    );
}

/// The cursor clamp must never place the cursor on the border column.
/// With a 58-char unbroken word and cursor at end, the clamped x must be
/// <= 1 + 56 (max content column), strictly less than 59 (border).
#[test]
fn cursor_clamp_never_reaches_border() {
    let mut renderer = make_test_renderer();

    // One unbroken 58-char word; cursor at end (col 58).
    // The clamp `min(width - 3)` = min(57) limits col to 57.
    // So x = 1 + 57 = 58, which is < 59 (the right border column).
    let mut input = InputLine::new();
    input.insert_str(&"x".repeat(58));

    renderer.draw(&input, &default_status()).unwrap();

    // Read cursor before borrowing buffer immutably
    let cursor = renderer
        .terminal
        .backend_mut()
        .get_cursor_position()
        .unwrap();

    let buf = renderer.terminal.backend().buffer();

    // cursor.x must be <= 1 + 57 (max content column is width-2 = 58,
    // so offset within box is 0..57, absolute x is 1..58)
    assert!(
        cursor.x <= 1 + 57,
        "cursor x={} must not exceed max content column (1+57=58)",
        cursor.x
    );

    // The border column (x=59) at the cursor's row must be '│' or similar,
    // not the cursor
    assert!(
        cursor.x < 59,
        "cursor x={} must be strictly less than border column 59",
        cursor.x
    );

    // Verify the border column is actually a border character
    let border_cell = buf[(59, cursor.y)].symbol();
    assert!(
        border_cell == "│" || border_cell == " ",
        "border column at (59, {}) should be a border char or space, got '{}'",
        cursor.y,
        border_cell
    );
}

/// `input_content_width()` returns the same value that `draw` uses for
/// wrapping — inner content width = terminal width - 2.
#[test]
fn input_content_width_matches_draw_width() {
    let renderer = make_test_renderer();
    // TestBackend is 60 cols wide; inner content = 60 - 2 = 58
    assert_eq!(
        renderer.input_content_width(),
        58,
        "input_content_width should be 58 on a 60-wide backend"
    );
}

#[test]
fn repin_rows_parks_at_viewport_top() {
    // Park is `height − VIEWPORT_ROWS`, never `height − 1`.
    // content_end = 18 (park), so the new min-term is not binding.
    assert_eq!(repin_rows(10, 18, 24), (10, 18));
}

#[test]
fn scrolled_repaint_plan_full_screen() {
    // Whole region is committed content: clear it all and repaint a full
    // screenful of history, bottom-anchored at the park row.
    assert_eq!(scrolled_repaint_plan(0, 100, 39), (0, 0, 39));
}

#[test]
fn scrolled_repaint_plan_short_history_bottom_anchors() {
    // Less history than region: paint the tail directly above the park.
    assert_eq!(scrolled_repaint_plan(0, 10, 39), (0, 29, 10));
}

#[test]
fn scrolled_repaint_plan_preserves_shell_prefix() {
    // Content starts mid-screen: rows above content_top are untouched
    // pre-renderer output, and the repaint never rises above the clear.
    assert_eq!(scrolled_repaint_plan(20, 100, 39), (20, 20, 19));
}

#[test]
fn scrolled_repaint_plan_content_top_past_park_clamps() {
    assert_eq!(scrolled_repaint_plan(50, 5, 39), (39, 39, 0));
}

#[test]
fn commit_tracks_rows_scrolled_once_viewport_hits_bottom() {
    let mut renderer = make_test_renderer();
    // 60x10 backend, 6-row viewport anchored at row 0: the viewport can
    // move down 4 rows before the region starts scrolling.
    renderer.commit("a\nb\nc\nd").unwrap();
    assert_eq!(renderer.rows_scrolled, 0);
    renderer.commit("e\nf\ng").unwrap();
    assert_eq!(renderer.rows_scrolled, 3);
}

#[test]
fn commit_methods_record_history_rows() {
    let mut renderer = make_test_renderer();
    renderer.commit("one\ntwo").unwrap();
    renderer.commit_styled(&[]).unwrap(); // empty slice still inserts one row
    renderer
        .commit_panel("t", &["body".to_string()], false)
        .unwrap();
    // panel = top border + 1 body + bottom border + trailing blank = 4
    assert_eq!(renderer.history.len(), 2 + 1 + 4);
    assert_eq!(renderer.inserted_rows, 7);
}

#[test]
fn history_ring_caps_at_max() {
    let mut renderer = make_test_renderer();
    for i in 0..(MAX_REPAINT_HISTORY + 50) {
        renderer.commit(&format!("line {i}")).unwrap();
    }
    assert_eq!(renderer.history.len(), MAX_REPAINT_HISTORY);
    // The tail must hold the newest lines.
    let last = renderer.history.back().unwrap().to_string();
    assert_eq!(last, format!("line {}", MAX_REPAINT_HISTORY + 49));
}

#[test]
fn repin_rows_clears_from_old_top_when_higher() {
    // Old viewport higher → clear from it. content_end >= park so not binding.
    assert_eq!(repin_rows(3, 18, 24), (3, 18));
    // Old viewport below the new top → clear from the new top.
    assert_eq!(repin_rows(20, 18, 24), (18, 18));
}

#[test]
fn repin_rows_short_terminal_saturates() {
    // Terminal shorter than the viewport: park saturates to row 0.
    assert_eq!(repin_rows(0, 0, 4), (0, 0));
}

#[test]
fn repin_rows_clears_debris_between_content_and_park() {
    // Old viewport at the bottom (18), real content ends at row 10.
    // The wipe starts at 10, removing everything in the debris gap.
    assert_eq!(repin_rows(18, 10, 24), (10, 18));
}

#[test]
fn repin_rows_content_past_park_clamps() {
    // Full-scrolled session: content_end past park, old_top < park.
    assert_eq!(repin_rows(10, 30, 24), (10, 18));
    // Both old_top and content_end past park → clamped to park.
    assert_eq!(repin_rows(20, 30, 24), (18, 18));
}

#[test]
fn commit_methods_count_inserted_rows() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(6),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    // commit("a\nb\nc") → 3 lines
    renderer.commit("a\nb\nc").unwrap();
    assert_eq!(renderer.inserted_rows, 3);

    // commit_panel("t", &[one body line], false) → top border + body +
    // bottom border + spacer = 4
    renderer
        .commit_panel("t", &["body".to_string()], false)
        .unwrap();
    assert_eq!(renderer.inserted_rows, 7);

    // commit_styled(&[two lines]) → 2
    renderer
        .commit_styled(&[Line::from("line1"), Line::from("line2")])
        .unwrap();
    assert_eq!(renderer.inserted_rows, 9);
}

/// Build the default test status bar state.
fn approval_test_status() -> StatusBarState<'static> {
    StatusBarState {
        session_id: "test-session",
        approval_hint: "",
        model: "test-model",
        prompt_tokens: 0,
        context_window: 200_000,
        daemon_up: false,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    }
}

/// Draw the approval panel on a fresh 80-col test renderer.
fn draw_approval_panel_test(
    renderer: &mut RatatuiRenderer<TestBackend>,
    session_label: &str,
    input: &InputLine,
) {
    renderer
        .draw_approval_panel(
            "approve command",
            session_label,
            input,
            &approval_test_status(),
        )
        .unwrap();
}

/// Collect the rendered rows of the live buffer as strings (80 cols wide).
fn buffer_rows(renderer: &RatatuiRenderer<TestBackend>) -> Vec<String> {
    let buf = renderer.terminal.backend().buffer();
    let symbols: Vec<String> = buf.content.iter().map(|c| c.symbol().to_string()).collect();
    (0..symbols.len())
        .step_by(80)
        .map(|i| symbols[i..std::cmp::min(i + 80, symbols.len())].join(""))
        .collect()
}

#[test]
fn approval_panel_options_multicolor() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    draw_approval_panel_test(&mut renderer, "session", &input);

    let rows = buffer_rows(&renderer);
    let options_row = rows
        .iter()
        .find(|r| r.contains("[Y]es"))
        .expect("options row with [Y]es should be rendered");
    assert!(
        options_row.contains("[A]pprove for session"),
        "options row should read [A]pprove for session, got: {}",
        options_row
    );
    assert!(
        options_row.contains("[N]o"),
        "options row should read [N]o, got: {}",
        options_row
    );

    // Key letters Y/A/N are bright yellow (bold); corners are blood red.
    let yellow = Color::Rgb(220, 160, 0);
    let red = Color::Rgb(180, 0, 0);
    let buf = renderer.terminal.backend().buffer();
    for (glyph, color) in [('Y', yellow), ('A', yellow), ('N', yellow)] {
        let cells: Vec<_> = buf
            .content
            .iter()
            .filter(|c| c.symbol() == glyph.to_string())
            .collect();
        assert!(
            !cells.is_empty(),
            "key letter '{}' should be rendered",
            glyph
        );
        for cell in &cells {
            assert_eq!(
                cell.style().fg,
                Some(color),
                "key letter '{}' should have yellow fg, got {:?}",
                glyph,
                cell.style().fg
            );
        }
    }
    for corner in ['╭', '╮', '╰', '╯'] {
        let cells: Vec<_> = buf
            .content
            .iter()
            .filter(|c| c.symbol() == corner.to_string())
            .collect();
        assert!(!cells.is_empty(), "corner '{}' should be rendered", corner);
        for cell in &cells {
            assert_eq!(
                cell.style().fg,
                Some(red),
                "corner '{}' should have blood-red fg, got {:?}",
                corner,
                cell.style().fg
            );
        }
    }
}

#[test]
fn approval_panel_sudo_session_label() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    draw_approval_panel_test(&mut renderer, "sudo session", &input);

    let rows = buffer_rows(&renderer);
    assert!(
        rows.iter()
            .any(|r| r.contains("[A]pprove for sudo session")),
        "options row should read [A]pprove for sudo session, got: {:?}",
        rows
    );
}

#[test]
fn approval_panel_has_no_command_row() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    draw_approval_panel_test(&mut renderer, "session", &input);

    let rows = buffer_rows(&renderer);
    assert!(
        !rows.iter().any(|r| r.contains("$ ")),
        "approval panel must not render a command row, got: {rows:?}"
    );
}

#[test]
fn approval_panel_shows_typed_input() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let mut input = InputLine::new();
    input.insert_str("why");
    draw_approval_panel_test(&mut renderer, "session", &input);

    let rows = buffer_rows(&renderer);
    let input_row = rows
        .iter()
        .find(|r| r.contains('›'))
        .expect("input row with prompt glyph should be rendered");
    assert!(
        input_row.contains("why"),
        "input row should contain typed text 'why', got: {}",
        input_row
    );
}

#[test]
fn approval_panel_short_region_falls_back() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(3),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    // Must not panic — falls back to the plain prompt shape.
    renderer
        .draw_approval_panel(
            "approve command",
            "session",
            &input,
            &approval_test_status(),
        )
        .unwrap();
}

fn draw_credential_panel_test(
    renderer: &mut RatatuiRenderer<TestBackend>,
    title: &str,
    detail: &str,
    input: &InputLine,
) {
    renderer
        .draw_credential_panel(title, detail, input, &approval_test_status())
        .unwrap();
}

#[test]
fn credential_panel_title_and_hint() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(6),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    draw_credential_panel_test(
        &mut renderer,
        "sudo password",
        "[sudo] password required",
        &input,
    );

    let rows = buffer_rows(&renderer);
    let title_row = rows
        .iter()
        .find(|r| r.contains("sudo password"))
        .expect("title row should be rendered");
    assert!(
        title_row.contains('╭') && title_row.contains('╮'),
        "top border row should have rounded corners, got: {}",
        title_row
    );

    let buf = renderer.terminal.backend().buffer();
    let corner = buf
        .content
        .iter()
        .find(|c| c.symbol() == "╭")
        .expect("top-left corner cell should be rendered");
    assert_eq!(
        corner.fg,
        renderer.palette.red(),
        "corner should be palette red"
    );

    let hint_row = rows
        .iter()
        .find(|r| r.contains("[Enter] submit"))
        .expect("hint row with [Enter] submit should be rendered");
    assert!(
        hint_row.contains("[Esc] cancel"),
        "hint row should contain [Esc] cancel, got: {}",
        hint_row
    );
}

#[test]
fn credential_panel_shows_bullets() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(6),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let mut input = InputLine::new();
    input.insert('•');
    input.insert('•');
    input.insert('•');
    draw_credential_panel_test(
        &mut renderer,
        "sudo password",
        "[sudo] password required",
        &input,
    );

    let rows = buffer_rows(&renderer);
    let input_row = rows
        .iter()
        .find(|r| r.contains('›'))
        .expect("input row with prompt glyph should be rendered");
    assert!(
        input_row.contains("•••"),
        "input row should contain three bullets, got: {}",
        input_row
    );
}

#[test]
fn credential_panel_truncates_long_detail() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(6),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    let long_detail = format!("[sudo] password required {}", "x".repeat(300));
    draw_credential_panel_test(&mut renderer, "sudo password", &long_detail, &input);

    let rows = buffer_rows(&renderer);
    let detail_row = rows
        .iter()
        .find(|r| r.contains("password required"))
        .expect("detail row should be rendered");
    assert!(
        detail_row.ends_with("…│") || detail_row.trim_end().ends_with('…'),
        "detail row should be truncated with ellipsis before the right border, got: {}",
        detail_row
    );
}

#[test]
fn credential_panel_short_region_falls_back() {
    let backend = TestBackend::new(80, 24);
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(3),
        },
    )
    .unwrap();
    let mut renderer = RatatuiRenderer {
        terminal,
        start_time: std::time::Instant::now(),
        palette: crate::cli::palette::Palette::for_depth(
            crate::cli::palette::ColorDepth::Truecolor,
        ),
        origin_row: 0,
        inserted_rows: 0,
        rows_scrolled: 0,
        history: std::collections::VecDeque::new(),
        last_width: 0,
        last_height: 0,
    };

    let input = InputLine::new();
    // Must not panic — falls back to the plain prompt shape.
    renderer
        .draw_credential_panel(
            "sudo password",
            "[sudo] password required",
            &input,
            &approval_test_status(),
        )
        .unwrap();
}
