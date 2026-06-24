use crate::cli::input::InputLine;
use crate::cli::render::StatusBarState;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

/// The number of rows the inline viewport occupies (input + status bar).
const VIEWPORT_ROWS: u16 = 4;

/// Ratatui-based inline-viewport renderer.
///
/// Owns a `Terminal<B>` with an inline viewport.  The viewport holds only the
/// input box and status bar; everything else is committed to scrollback via
/// `insert_before`.
///
/// `B` is the backend.  Production uses `CrosstermBackend<Stdout>`; tests use
/// `TestBackend`.
pub struct RatatuiRenderer<B: Backend> {
    terminal: Terminal<B>,
    start_time: std::time::Instant,
}

// Type alias for the production backend.
pub type RatatuiRendererStdout =
    RatatuiRenderer<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

impl RatatuiRenderer<ratatui::backend::CrosstermBackend<std::io::Stdout>> {
    /// Create a new renderer with an inline viewport on stdout.
    ///
    /// Enters raw mode and constructs the terminal.  Callers must **not**
    /// have called `set_raw_mode()` from `input.rs` before this — ratatui
    /// manages raw mode internally and we avoid double-entering it.
    pub fn new(start_time: std::time::Instant) -> std::io::Result<Self> {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
            },
        )?;
        Ok(Self {
            terminal,
            start_time,
        })
    }
}

impl<B: Backend> RatatuiRenderer<B> {
    /// Commit one or more finished transcript lines into scrollback above
    /// the inline viewport.
    pub fn commit(&mut self, lines: &str) -> Result<(), B::Error> {
        let row_count = lines.matches('\n').count() + 1;
        self.terminal.insert_before(row_count as u16, |buf| {
            let area = buf.area;
            for (i, line) in lines.split('\n').enumerate() {
                let y = i as u16;
                if y >= area.height {
                    break;
                }
                let text: String = line.chars().take(area.width as usize).collect();
                buf.set_string(area.x, area.y + y, &text, Style::default());
            }
        })
    }

    /// Draw the live region: input box and status bar.
    pub fn draw(&mut self, input: &InputLine, status: &StatusBarState<'_>) -> Result<(), B::Error> {
        let input_text = input.as_str();
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;

        let _completed = self.terminal.draw(|frame| {
            let area = frame.area();
            render_live_region(frame, area, &input_text, &session_id, &model, start_time);
        })?;
        Ok(())
    }

    /// Restore the terminal to its original state (exit raw mode, show
    /// cursor, clear inline viewport rows).
    pub fn restore(&mut self) {
        let _ = ratatui::try_restore();
    }
}

fn render_live_region(
    frame: &mut ratatui::Frame,
    area: Rect,
    input_text: &str,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    // ── Input box ──────────────────────────────────────────────
    let input_line = Line::from(Span::raw(input_text));
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Gray));
    let input_para = Paragraph::new(input_line).block(input_block);
    frame.render_widget(input_para, chunks[0]);

    // ── Status bar ─────────────────────────────────────────────
    let uptime = fmt_uptime(start_time.elapsed());
    let status_text = format!(
        " session:{} · {} · up {} ",
        &session_id[..8.min(session_id.len())],
        model,
        uptime,
    );
    let status_block = Block::default().borders(Borders::NONE).style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    );
    let status_para = Paragraph::new(Line::from(Span::raw(status_text))).block(status_block);
    frame.render_widget(status_para, chunks[1]);
}

fn fmt_uptime(elapsed: std::time::Duration) -> String {
    let s = elapsed.as_secs();
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn make_test_renderer() -> RatatuiRenderer<TestBackend> {
        let backend = TestBackend::new(60, 10);
        let terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
            },
        )
        .unwrap();
        RatatuiRenderer {
            terminal,
            start_time: std::time::Instant::now(),
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
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        // Status bar should contain session id and model name.
        assert!(
            all_text.contains("session:abcdef12"),
            "status bar should contain session id, got: {}",
            all_text
        );
        assert!(
            all_text.contains("test-model"),
            "status bar should contain model name, got: {}",
            all_text
        );
        // Input box border should be present.
        assert!(
            all_text.contains('┌'),
            "input box top border should be present, got: {}",
            all_text
        );
        assert!(
            all_text.contains('│'),
            "input box side border should be present, got: {}",
            all_text
        );
        // Input text should be rendered.
        assert!(
            all_text.contains("Hello"),
            "input box should contain 'Hello', got: {}",
            all_text
        );
    }

    #[test]
    fn commit_renders_transcript_line_into_buffer() {
        let mut renderer = make_test_renderer();

        // First draw the live region so the viewport has content.
        let input = InputLine::new();
        let status = StatusBarState {
            session_id: "test-session",
            approval_hint: "",
            model: "test-model",
            prompt_tokens: 0,
            context_window: 200_000,
            daemon_up: false,
            tools_total: 0,
            cost_usd: 0.0,
            has_untracked: false,
        };
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
            buf_text.contains("Hello from transcript")
                || scroll_text.contains("Hello from transcript"),
            "committed line should appear in buffer or scrollback. buf: {} scroll: {}",
            buf_text,
            scroll_text,
        );
    }

    #[test]
    fn fmt_uptime_formats_seconds() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(42)), "42s");
    }

    #[test]
    fn fmt_uptime_formats_minutes() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(125)), "2m 5s");
    }

    #[test]
    fn fmt_uptime_formats_hours() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3725)), "1h 2m");
    }
}
