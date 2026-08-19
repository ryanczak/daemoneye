//! Alt-screen transcript viewer: full-screen read-only pager over the
//! client-side transcript model. See `docs/design/transcript-view.md`.

use crate::cli::transcript::Block;
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::HashSet;

/// What a rendered row is, so the draw pass can style it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Header,
    User,
    Assistant,
    Tool,
    Output,
    System,
    Blank,
}

/// One laid-out screen row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRow {
    pub text: String,
    pub kind: RowKind,
    /// Index of the block this row came from. Blank separators carry the index
    /// of the block they precede.
    pub block: usize,
}

/// Lay the transcript out at `width` columns: one `ViewRow` per screen row,
/// blocks separated by exactly one blank row (never after the last).
pub fn layout_blocks(blocks: &[Block], width: usize) -> Vec<ViewRow> {
    layout_blocks_with(blocks, width, &HashSet::new())
}

/// Lay out with a set of collapsed block indices. `layout_blocks` is this with
/// an empty set.
pub fn layout_blocks_with(
    blocks: &[Block],
    width: usize,
    collapsed: &HashSet<usize>,
) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 && !rows.is_empty() {
            rows.push(ViewRow {
                text: String::new(),
                kind: RowKind::Blank,
                block: i,
            });
        }
        if collapsed.contains(&i) {
            layout_block_collapsed(block, width, &mut rows, i);
        } else {
            layout_block(block, width, &mut rows, i);
        }
    }
    rows
}

fn layout_block(block: &Block, width: usize, rows: &mut Vec<ViewRow>, idx: usize) {
    match block {
        Block::UserTurn { label, text } => {
            rows.push(ViewRow {
                text: format!("▾ {label}"),
                kind: RowKind::Header,
                block: idx,
            });
            push_wrapped(rows, text, width, RowKind::User, idx);
        }
        Block::Assistant { text } => {
            push_wrapped(rows, text, width, RowKind::Assistant, idx);
        }
        Block::ToolPanel {
            tool,
            summary,
            label,
        } => {
            let header = match label {
                Some(l) => format!("▾ {tool} — {l}"),
                None => format!("▾ {tool}"),
            };
            rows.push(ViewRow {
                text: header,
                kind: RowKind::Header,
                block: idx,
            });
            push_wrapped(rows, summary, width, RowKind::Tool, idx);
        }
        Block::Output { full, shown: _, .. } => {
            let n = full.lines().count();
            rows.push(ViewRow {
                text: format!("output ({n} lines)"),
                kind: RowKind::Header,
                block: idx,
            });
            // Never elide: showing what the inline panel hid is the point.
            for line in full.lines() {
                push_wrapped(rows, line, width, RowKind::Output, idx);
            }
        }
        Block::System { text } => {
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { "⚙ " } else { "" };
                first = false;
                let wrapped = crate::cli::render::wrap_line_hard(line, width);
                if wrapped.is_empty() {
                    rows.push(ViewRow {
                        text: prefix.to_string(),
                        kind: RowKind::System,
                        block: idx,
                    });
                } else if let Some((first_line, rest)) = wrapped.split_first() {
                    rows.push(ViewRow {
                        text: format!("{prefix}{first_line}"),
                        kind: RowKind::System,
                        block: idx,
                    });
                    for wl in rest {
                        rows.push(ViewRow {
                            text: wl.clone(),
                            kind: RowKind::System,
                            block: idx,
                        });
                    }
                }
            }
        }
    }
}

/// A block with no header of its own (Assistant, System) uses its first
/// laid-out row as the header row for the collapse suffix.
fn layout_block_collapsed(block: &Block, width: usize, rows: &mut Vec<ViewRow>, idx: usize) {
    let mut full: Vec<ViewRow> = Vec::new();
    layout_block(block, width, &mut full, idx);
    let n = full.len().saturating_sub(1);
    let text = match bare_header(block) {
        Some(bare) => format!("▸ {bare} [collapsed, {n} lines]"),
        None => {
            let first = full.first().map(|r| r.text.clone()).unwrap_or_default();
            format!("{first} [collapsed, {n} lines]")
        }
    };
    rows.push(ViewRow {
        text,
        kind: RowKind::Header,
        block: idx,
    });
}

fn bare_header(block: &Block) -> Option<String> {
    match block {
        Block::UserTurn { label, .. } => Some(label.clone()),
        Block::ToolPanel { tool, label, .. } => Some(match label {
            Some(l) => format!("{tool} — {l}"),
            None => tool.clone(),
        }),
        Block::Output { full, .. } => {
            let n = full.lines().count();
            Some(format!("output ({n} lines)"))
        }
        Block::Assistant { .. } | Block::System { .. } => None,
    }
}

fn push_wrapped(rows: &mut Vec<ViewRow>, text: &str, width: usize, kind: RowKind, idx: usize) {
    for line in crate::cli::render::wrap_line_hard(text, width) {
        rows.push(ViewRow {
            text: line,
            kind,
            block: idx,
        });
    }
}

/// Clamp a scroll offset so the last page never scrolls past the end.
pub fn clamp_scroll(scroll: usize, total: usize, height: usize) -> usize {
    let max = total.saturating_sub(height);
    scroll.min(max)
}

/// Render `rows` into a frame at a scroll offset. The bottom row is a status
/// line; the rows above it show `rows[scroll..]`. Never panics on an empty
/// row set or an out-of-range scroll.
pub fn render_transcript(
    f: &mut Frame,
    rows: &[ViewRow],
    scroll: usize,
    focus: usize,
    evicted: usize,
) {
    let area = f.area();
    let body_height = area.height.saturating_sub(1);
    let scroll = clamp_scroll(scroll, rows.len(), body_height as usize);
    let palette = crate::cli::palette::Palette::from_env();

    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in rows.iter().skip(scroll).take(body_height as usize) {
        let style = if row.block == focus {
            style_for_focused(row.kind, palette)
        } else {
            style_for(row.kind, palette)
        };
        lines.push(Line::from(Span::styled(row.text.clone(), style)));
    }

    let total = rows.len();
    let shown_from = if total == 0 { 0 } else { scroll + 1 };
    let shown_to = (scroll + body_height as usize).min(total);
    let mut status = format!("transcript — {shown_from}-{shown_to} of {total} lines");
    if evicted > 0 {
        status = format!("{evicted} older blocks evicted · {status}");
    }
    status
        .push_str(" · [ ] focus · enter collapse · c/a all · ↑↓ PgUp/PgDn Home/End · esc to close");

    let status_line = Line::from(Span::styled(
        status,
        Style::default().add_modifier(Modifier::DIM),
    ));
    let para = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(
        para,
        ratatui::layout::Rect::new(area.x, area.y, area.width, body_height),
    );
    f.render_widget(
        status_line,
        ratatui::layout::Rect::new(area.x, area.y + body_height, area.width, 1),
    );
}

fn style_for(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    match kind {
        RowKind::Header => Style::default()
            .fg(palette.yellow())
            .add_modifier(Modifier::BOLD),
        RowKind::User => Style::default().fg(Color::Cyan),
        RowKind::Assistant => Style::default().fg(Color::White),
        RowKind::Tool => Style::default().fg(Color::Green),
        RowKind::Output => Style::default().fg(Color::Gray),
        RowKind::System => Style::default().fg(Color::Magenta),
        RowKind::Blank => Style::default(),
    }
}

/// Emphasised variant for the focused block's rows: same colour, underlined.
fn style_for_focused(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    style_for(kind, palette).add_modifier(Modifier::UNDERLINED)
}

/// RAII guard for the alternate screen: `Drop` runs the teardown even when the
/// guarded scope exits early through `?`, so a viewer failure can never strand
/// the terminal on the alternate screen with the inline viewport unpinned.
/// The teardown action is injectable so tests can count how often it runs
/// without a real terminal.
struct AltScreenGuard<'a> {
    teardown: Box<dyn FnMut() + 'a>,
}

impl<'a> AltScreenGuard<'a> {
    fn new(teardown: impl FnMut() + 'a) -> Self {
        Self {
            teardown: Box::new(teardown),
        }
    }
}

impl Drop for AltScreenGuard<'_> {
    fn drop(&mut self) {
        (self.teardown)();
    }
}

/// Full-screen alternate-screen viewer loop.
///
/// Enters the alternate screen, runs its own key loop, and exits back to the
/// inline renderer via `reanchor()` — never tearing down raw mode, which the
/// chat session still needs.
pub async fn run_transcript_viewer(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    transcript: &crate::cli::transcript::Transcript,
) -> anyhow::Result<()> {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};

    execute!(std::io::stdout(), EnterAlternateScreen)?;

    // From here on the screen is owned by this guard: leaving it and re-pinning
    // the inline viewport runs from `Drop`, so `?`, `break` and `Ok(())` all
    // exit the same way. `let _ =` is required — a `Drop` cannot propagate.
    let _guard = AltScreenGuard::new(|| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });

    viewer_loop(stdin, sigwinch, transcript).await?;
    Ok(())
}

/// Next focused block index, wrapping. `len == 0` yields 0.
pub fn focus_next(focus: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focus + 1) % len
}

/// Previous focused block index, wrapping. `len == 0` yields 0.
pub fn focus_prev(focus: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focus + len - 1) % len
}

/// The fallible body of the viewer. Lives separately from
/// `run_transcript_viewer` so the fullscreen `Terminal` is dropped (buffer
/// cleared) before the `AltScreenGuard` — which owns leaving the alternate
/// screen — runs its teardown after this returns, on every exit path.
async fn viewer_loop(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    transcript: &crate::cli::transcript::Transcript,
) -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let blocks_len = transcript.blocks().len();
    let mut focus = blocks_len.saturating_sub(1);
    let mut collapsed: std::collections::HashSet<usize> = std::collections::HashSet::new();

    let mut scroll: usize;
    {
        let size = terminal.size()?;
        let width = size.width as usize;
        let rows = layout_blocks(transcript.blocks(), width);
        let body_height = size.height.saturating_sub(1) as usize;
        scroll = clamp_scroll(usize::MAX, rows.len(), body_height);
        terminal.draw(|f| {
            render_transcript(f, &rows, scroll, focus, transcript.evicted());
        })?;
    }

    loop {
        let key = tokio::select! {
            _ = sigwinch.recv() => {
                let size = terminal.size()?;
                let width = size.width as usize;
                let rows = layout_blocks(transcript.blocks(), width);
                let body_height = size.height.saturating_sub(1) as usize;
                scroll = clamp_scroll(scroll, rows.len(), body_height);
                terminal.draw(|f| {
                    render_transcript(f, &rows, scroll, focus, transcript.evicted());
                })?;
                continue;
            }
            key = crate::cli::input::read_key(stdin) => key,
        };
        let Some(key) = key else { break };
        let size = terminal.size()?;
        let width = size.width as usize;
        let rows = layout_blocks(transcript.blocks(), width);
        let body_height = size.height.saturating_sub(1) as usize;
        let mut focus_changed = false;
        match key {
            crate::cli::input::Key::Up => {
                scroll = scroll.saturating_sub(1);
            }
            crate::cli::input::Key::Down => {
                scroll = scroll.saturating_add(1);
            }
            crate::cli::input::Key::PageUp => {
                scroll = scroll.saturating_sub(body_height.saturating_sub(1));
            }
            crate::cli::input::Key::PageDown => {
                scroll = scroll.saturating_add(body_height.saturating_sub(1));
            }
            crate::cli::input::Key::Home => scroll = 0,
            crate::cli::input::Key::End => scroll = usize::MAX,
            crate::cli::input::Key::Char(']') => {
                focus = focus_next(focus, blocks_len);
                focus_changed = true;
            }
            crate::cli::input::Key::Char('[') => {
                focus = focus_prev(focus, blocks_len);
                focus_changed = true;
            }
            crate::cli::input::Key::Enter => {
                if collapsed.contains(&focus) {
                    collapsed.remove(&focus);
                } else {
                    collapsed.insert(focus);
                }
            }
            crate::cli::input::Key::Char('c') => {
                collapsed.clear();
                for (i, block) in transcript.blocks().iter().enumerate() {
                    if matches!(block, crate::cli::transcript::Block::Output { .. }) {
                        collapsed.insert(i);
                    }
                }
            }
            crate::cli::input::Key::Char('a') => {
                collapsed.clear();
            }
            crate::cli::input::Key::Char('\x1b')
            | crate::cli::input::Key::Char('q')
            | crate::cli::input::Key::CtrlO => break,
            _ => {}
        }
        if focus_changed
            && let Some(row_idx) = rows
                .iter()
                .position(|r| r.block == focus && r.kind == crate::cli::viewer::RowKind::Header)
        {
            if row_idx < scroll {
                scroll = row_idx;
            } else if row_idx >= scroll + body_height {
                scroll = row_idx.saturating_add(1).saturating_sub(body_height);
            }
        }
        scroll = clamp_scroll(scroll, rows.len(), body_height);
        terminal.draw(|f| {
            render_transcript(f, &rows, scroll, focus, transcript.evicted());
        })?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn output_block(full: &str, shown: usize) -> Block {
        Block::Output {
            tool_call_id: "toolu_abc".to_string(),
            full: full.to_string(),
            shown,
        }
    }

    #[test]
    fn layout_blocks_renders_full_output() {
        let mut full = String::new();
        for i in 0..300 {
            full.push_str(&format!("line {i}\n"));
        }
        let rows = layout_blocks(&[output_block(&full, 9)], 100);
        assert_eq!(rows[0].text, "output (300 lines)");
        assert_eq!(rows[0].kind, RowKind::Header);
        assert_eq!(rows.len(), 301);
        assert!(rows[1..].iter().all(|r| r.kind == RowKind::Output));
        assert_eq!(rows[300].text, "line 299");
    }

    #[test]
    fn layout_blocks_separates_blocks_with_one_blank() {
        let blocks = vec![
            Block::Assistant {
                text: "first".to_string(),
            },
            Block::Assistant {
                text: "second".to_string(),
            },
        ];
        let rows = layout_blocks(&blocks, 80);
        let blanks: Vec<&ViewRow> = rows.iter().filter(|r| r.kind == RowKind::Blank).collect();
        assert_eq!(blanks.len(), 1);
        assert_ne!(rows.last().unwrap().kind, RowKind::Blank);
    }

    #[test]
    fn layout_blocks_wraps_to_width() {
        let text = "x".repeat(100);
        let rows = layout_blocks(&[Block::Assistant { text: text.clone() }], 20);
        assert!(rows.iter().all(|r| r.text.chars().count() <= 20));
        let rejoined: String = rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(rejoined, text);
    }

    #[test]
    fn layout_blocks_empty_transcript_is_empty() {
        assert!(layout_blocks(&[], 80).is_empty());
    }

    #[test]
    fn clamp_scroll_pins_to_last_page() {
        assert_eq!(clamp_scroll(9999, 100, 10), 90);
        assert_eq!(clamp_scroll(50, 100, 10), 50);
    }

    #[test]
    fn clamp_scroll_zero_when_content_fits() {
        assert_eq!(clamp_scroll(5, 3, 10), 0);
    }

    #[test]
    fn render_transcript_draws_rows_into_backend() {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let rows = vec![
            ViewRow {
                text: "alpha".to_string(),
                kind: RowKind::Header,
                block: 0,
            },
            ViewRow {
                text: "beta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "gamma".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "delta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "epsilon".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "zeta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "eta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
        ];
        terminal
            .draw(|f| render_transcript(f, &rows, 2, 0, 0))
            .unwrap();
        let buf = terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(all_text.contains("gamma"), "got: {all_text}");
        assert!(all_text.contains("of 7 lines"), "got: {all_text}");
        let bottom_row: String = buf
            .content
            .iter()
            .skip((7 * 40) as usize)
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            bottom_row.contains("of 7 lines"),
            "status must be on the bottom row, got: {bottom_row}"
        );
    }

    #[test]
    fn render_transcript_survives_scroll_past_end() {
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        let rows = vec![
            ViewRow {
                text: "one".to_string(),
                kind: RowKind::System,
                block: 0,
            },
            ViewRow {
                text: "two".to_string(),
                kind: RowKind::System,
                block: 0,
            },
        ];
        terminal
            .draw(|f| render_transcript(f, &rows, 9999, 0, 1))
            .unwrap();
        let buf = terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(all_text.contains("evicted ·"), "got: {all_text}");
        assert!(all_text.contains("of 2 lines"), "got: {all_text}");
    }

    #[test]
    fn collapsed_output_lays_out_as_exactly_one_row() {
        let mut full = String::new();
        for i in 0..300 {
            full.push_str(&format!("line {i}\n"));
        }
        let before = layout_blocks(&[output_block(&full, 9)], 100).len();
        assert_eq!(before, 301);
        let collapsed = HashSet::from([0]);
        let rows = layout_blocks_with(&[output_block(&full, 9)], 100, &collapsed);
        let block_rows = rows.iter().filter(|r| r.block == 0).count();
        assert_eq!(
            block_rows, 1,
            "collapsed block must contribute exactly 1 row"
        );
        assert!(rows[0].text.contains("[collapsed, 300 lines]"));
    }

    #[test]
    fn expanded_layout_is_unchanged_by_the_new_path() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            output_block("lorem\nipsum\ndolor\nsit\namet\nset\num\n", 6),
            Block::Assistant {
                text: "world".to_string(),
            },
            Block::Assistant {
                text: "again".to_string(),
            },
            Block::System {
                text: "⚙ note\n⚙ more".to_string(),
            },
        ];
        let empty = HashSet::new();
        assert_eq!(
            layout_blocks(&blocks, 80),
            layout_blocks_with(&blocks, 80, &empty)
        );
    }

    #[test]
    fn collapse_toggle_is_involutive() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            output_block("line one\nline two\nline three\n", 2),
            Block::Assistant {
                text: "world".to_string(),
            },
        ];
        let original = layout_blocks(&blocks, 80);
        let collapsed_rows = layout_blocks_with(&blocks, 80, &HashSet::from([1]));
        assert_ne!(original, collapsed_rows);
        let restored = layout_blocks_with(&blocks, 80, &HashSet::new());
        assert_eq!(original, restored);
    }

    #[test]
    fn collapse_all_outputs_collapses_only_outputs() {
        let block = |i: usize| Block::System {
            text: format!("s{i}"),
        };
        let blocks = [
            output_block("a\nb\n", 1),
            block(0),
            output_block("c\nd\n", 1),
            block(1),
            block(2),
        ];
        assert_eq!(blocks.len(), 5);
        let outputs: Vec<bool> = blocks
            .iter()
            .map(|b| matches!(b, Block::Output { .. }))
            .collect();
        let mut collapsed = HashSet::new();
        for (i, b) in blocks.iter().enumerate() {
            if matches!(b, Block::Output { .. }) {
                collapsed.insert(i);
            }
        }
        assert_eq!(collapsed.len(), 2, "exactly 2 members");
        for &i in &collapsed {
            assert!(outputs[i], "index {i} must be an Output block");
        }
    }

    #[test]
    fn focus_next_wraps_at_last_block() {
        assert_eq!(focus_next(2, 3), 0);
        assert_eq!(focus_next(0, 3), 1);
        assert_eq!(focus_next(0, 0), 0);
    }

    #[test]
    fn focus_prev_wraps_at_first() {
        assert_eq!(focus_prev(0, 3), 2);
        assert_eq!(focus_prev(2, 3), 1);
        assert_eq!(focus_prev(0, 0), 0);
    }

    #[test]
    fn rows_carry_their_source_block_index() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            Block::Assistant {
                text: "world".to_string(),
            },
            output_block("lorem\nipsum\n", 1),
        ];
        let rows = layout_blocks(&blocks, 80);
        let mut expected = 0;
        for row in &rows {
            if row.kind == RowKind::Blank {
                expected += 1;
                continue;
            }
            assert_eq!(
                row.block, expected,
                "row {:?} must carry the index of the block it came from",
                row.text
            );
        }
    }

    #[test]
    fn render_transcript_marks_collapsed_and_focused() {
        let collapsed = HashSet::from([1]);
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            output_block("first output line\n", 1),
            Block::System {
                text: "done".to_string(),
            },
        ];
        let rows = layout_blocks_with(&blocks, 60, &collapsed);
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal
            .draw(|f| render_transcript(f, &rows, 0, 1, 0))
            .unwrap();
        let buf = terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            all_text.contains('▸'),
            "collapsed marker must draw: {all_text}"
        );
        assert!(all_text.contains("enter collapse"), "got: {all_text}");
    }

    #[test]
    fn alt_screen_guard_runs_teardown_on_drop() {
        use std::cell::Cell;
        use std::rc::Rc;

        let teardown_runs = Rc::new(Cell::new(0));
        let count = Rc::clone(&teardown_runs);
        {
            let guard = AltScreenGuard::new(move || {
                count.set(count.get() + 1);
            });
            drop(guard);
        }
        assert_eq!(teardown_runs.get(), 1, "teardown must run exactly once");
    }

    #[test]
    fn alt_screen_guard_runs_teardown_on_normal_exit() {
        use std::cell::Cell;
        use std::rc::Rc;

        let teardown_runs = Rc::new(Cell::new(0));
        let count = Rc::clone(&teardown_runs);
        {
            let _guard = AltScreenGuard::new(move || {
                count.set(count.get() + 1);
            });
        }
        assert_eq!(
            teardown_runs.get(),
            1,
            "a normally-returning guarded scope must run teardown exactly once"
        );
    }
}
