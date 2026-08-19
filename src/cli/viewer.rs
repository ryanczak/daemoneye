//! Alt-screen transcript viewer: full-screen read-only pager over the
//! client-side transcript model. See `docs/design/transcript-view.md`.

use crate::cli::transcript::Block;
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

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
}

/// Lay the transcript out at `width` columns: one `ViewRow` per screen row,
/// blocks separated by exactly one blank row (never after the last).
pub fn layout_blocks(blocks: &[Block], width: usize) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 && !rows.is_empty() {
            rows.push(ViewRow {
                text: String::new(),
                kind: RowKind::Blank,
            });
        }
        layout_block(block, width, &mut rows);
    }
    rows
}

fn layout_block(block: &Block, width: usize, rows: &mut Vec<ViewRow>) {
    match block {
        Block::UserTurn { label, text } => {
            rows.push(ViewRow {
                text: format!("▸ {label}"),
                kind: RowKind::Header,
            });
            push_wrapped(rows, text, width, RowKind::User);
        }
        Block::Assistant { text } => {
            push_wrapped(rows, text, width, RowKind::Assistant);
        }
        Block::ToolPanel {
            tool,
            summary,
            label,
        } => {
            let header = match label {
                Some(l) => format!("▸ {tool} — {l}"),
                None => format!("▸ {tool}"),
            };
            rows.push(ViewRow {
                text: header,
                kind: RowKind::Header,
            });
            push_wrapped(rows, summary, width, RowKind::Tool);
        }
        Block::Output { full, shown: _, .. } => {
            let n = full.lines().count();
            rows.push(ViewRow {
                text: format!("output ({n} lines)"),
                kind: RowKind::Header,
            });
            // Never elide: showing what the inline panel hid is the point.
            for line in full.lines() {
                push_wrapped(rows, line, width, RowKind::Output);
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
                    });
                } else if let Some((first_line, rest)) = wrapped.split_first() {
                    rows.push(ViewRow {
                        text: format!("{prefix}{first_line}"),
                        kind: RowKind::System,
                    });
                    for wl in rest {
                        rows.push(ViewRow {
                            text: wl.clone(),
                            kind: RowKind::System,
                        });
                    }
                }
            }
        }
    }
}

fn push_wrapped(rows: &mut Vec<ViewRow>, text: &str, width: usize, kind: RowKind) {
    for line in crate::cli::render::wrap_line_hard(text, width) {
        rows.push(ViewRow { text: line, kind });
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
pub fn render_transcript(f: &mut Frame, rows: &[ViewRow], scroll: usize, evicted: usize) {
    let area = f.area();
    let body_height = area.height.saturating_sub(1);
    let scroll = clamp_scroll(scroll, rows.len(), body_height as usize);
    let palette = crate::cli::palette::Palette::from_env();

    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in rows.iter().skip(scroll).take(body_height as usize) {
        lines.push(Line::from(Span::styled(
            row.text.clone(),
            style_for(row.kind, palette),
        )));
    }

    let total = rows.len();
    let shown_from = if total == 0 { 0 } else { scroll + 1 };
    let shown_to = (scroll + body_height as usize).min(total);
    let mut status = format!("transcript — {shown_from}-{shown_to} of {total} lines");
    if evicted > 0 {
        status = format!("{evicted} older blocks evicted · {status}");
    }
    status.push_str(" · ↑↓ PgUp/PgDn Home/End · esc to close");

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
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    let mut scroll: usize;
    {
        let size = terminal.size()?;
        let width = size.width as usize;
        let rows = layout_blocks(transcript.blocks(), width);
        let body_height = size.height.saturating_sub(1) as usize;
        scroll = clamp_scroll(usize::MAX, rows.len(), body_height);
        terminal.draw(|f| {
            render_transcript(f, &rows, scroll, transcript.evicted());
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
                    render_transcript(f, &rows, scroll, transcript.evicted());
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
            crate::cli::input::Key::Char('\x1b')
            | crate::cli::input::Key::Char('q')
            | crate::cli::input::Key::CtrlO => break,
            _ => {}
        }
        scroll = clamp_scroll(scroll, rows.len(), body_height);
        terminal.draw(|f| {
            render_transcript(f, &rows, scroll, transcript.evicted());
        })?;
    }

    // Drop the fullscreen terminal before leaving the alternate screen so the
    // screen is cleared under the alternate-screen buffer, then re-pin the
    // inline viewport just as a resize would.
    drop(terminal);
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    let _ = execute!(std::io::stdout(), Show);
    renderer.reanchor();
    Ok(())
}

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
            },
            ViewRow {
                text: "beta".to_string(),
                kind: RowKind::Output,
            },
            ViewRow {
                text: "gamma".to_string(),
                kind: RowKind::Output,
            },
            ViewRow {
                text: "delta".to_string(),
                kind: RowKind::Output,
            },
            ViewRow {
                text: "epsilon".to_string(),
                kind: RowKind::Output,
            },
            ViewRow {
                text: "zeta".to_string(),
                kind: RowKind::Output,
            },
            ViewRow {
                text: "eta".to_string(),
                kind: RowKind::Output,
            },
        ];
        terminal
            .draw(|f| render_transcript(f, &rows, 2, 0))
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
            },
            ViewRow {
                text: "two".to_string(),
                kind: RowKind::System,
            },
        ];
        terminal
            .draw(|f| render_transcript(f, &rows, 9999, 1))
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
}
