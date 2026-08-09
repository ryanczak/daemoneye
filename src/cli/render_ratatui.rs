use crate::cli::input::InputLine;
use crate::cli::render::StatusBarState;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap};

/// Convert a string with ANSI escape sequences into a vector of styled `Span`s
/// suitable for ratatui rendering.  Each contiguous run of characters sharing
/// the same style becomes one `Span`.
pub fn parse_ansi_to_spans(input: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style = Style::default();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Flush accumulated text with current style.
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text.clone(), current_style));
                current_text.clear();
            }
            // Peek for '[' without consuming.
            let is_csi = matches!(chars.peek(), Some(&'['));
            if is_csi {
                chars.next(); // consume '['
                let mut seq = String::new();
                loop {
                    let c = chars.peek().copied();
                    match c {
                        Some(c) if c.is_ascii_alphabetic() => {
                            chars.next();
                            seq.push(c);
                            break;
                        }
                        Some(c) if c.is_ascii_digit() || c == ';' => {
                            chars.next();
                            seq.push(c);
                        }
                        _ => break,
                    }
                }
                current_style = apply_sgr(current_style, &seq);
            }
        } else {
            current_text.push(ch);
        }
    }
    if !current_text.is_empty() {
        spans.push(Span::styled(current_text, current_style));
    }
    spans
}

/// Apply an SGR escape sequence to a `Style`.
fn apply_sgr(mut style: Style, seq: &str) -> Style {
    let params_str = seq.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let parts: Vec<&str> = params_str.split(';').collect();
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        match part {
            "0" => style = Style::default(),
            "1" => style = style.add_modifier(Modifier::BOLD),
            "2" => style = style.add_modifier(Modifier::DIM),
            "3" => style = style.add_modifier(Modifier::ITALIC),
            "22" => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            "23" => style = style.remove_modifier(Modifier::ITALIC),
            "90" => style = style.fg(Color::DarkGray),
            "93" => style = style.fg(Color::Yellow),
            "94" => style = style.fg(Color::Blue),
            "95" => style = style.fg(Color::Magenta),
            "96" => style = style.fg(Color::Cyan),
            "97" => style = style.fg(Color::Gray),
            "31" => style = style.fg(Color::Red),
            "32" => style = style.fg(Color::Green),
            "33" => style = style.fg(Color::Yellow),
            "36" => style = style.fg(Color::Cyan),
            "38" if i + 2 < parts.len() && parts[i + 1] == "5" => {
                if let Ok(idx) = parts[i + 2].parse::<u8>() {
                    style = style.fg(color_from_256(idx));
                    i += 2;
                }
            }
            "38" if i + 4 < parts.len() && parts[i + 1] == "2" => {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    parts[i + 2].parse::<u8>(),
                    parts[i + 3].parse::<u8>(),
                    parts[i + 4].parse::<u8>(),
                ) {
                    style = style.fg(Color::Rgb(r, g, b));
                    i += 4;
                }
            }
            _ => {}
        }
        i += 1;
    }
    style
}

/// Map a 256-color index to a ratatui `Color`.
fn color_from_256(idx: u8) -> Color {
    match idx {
        0 => Color::Reset,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::Red,
        10 => Color::Green,
        11 => Color::Yellow,
        12 => Color::Blue,
        13 => Color::Magenta,
        14 => Color::Cyan,
        15 => Color::White,
        _ => Color::Indexed(idx),
    }
}

/// The number of rows the inline viewport occupies (input + status bar).
const VIEWPORT_ROWS: u16 = 6;

/// Rows reserved above the input box for the streaming spinner line. The row
/// is always reserved — blank when idle — so the input box never moves
/// vertically when streaming starts or stops.
const SPINNER_ROWS: u16 = 1;

/// Minimum live-region height at which the spinner row is reserved. Below
/// this the row collapses so a very short region still gets a usable box.
const MIN_HEIGHT_FOR_SPINNER_ROW: u16 = 5;

/// Split a live-region area into (spinner_row, body). The spinner row is
/// reserved in every draw mode; `body` is what the existing vertical layouts
/// then split into input box and status bar. On a short region the spinner
/// row is zero-height.
fn split_spinner_row(area: Rect) -> (Rect, Rect) {
    if area.height < MIN_HEIGHT_FOR_SPINNER_ROW {
        let empty = Rect { height: 0, ..area };
        return (empty, area);
    }
    let chunks =
        Layout::vertical([Constraint::Length(SPINNER_ROWS), Constraint::Min(1)]).split(area);
    (chunks[0], chunks[1])
}

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
    palette: crate::cli::palette::Palette,
}

// Type alias for the production backend.
pub type RatatuiRendererStdout =
    RatatuiRenderer<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

impl RatatuiRenderer<ratatui::backend::CrosstermBackend<std::io::Stdout>> {
    /// Create a new renderer with an inline viewport on stdout.
    ///
    /// Enters raw mode via `crossterm::terminal::enable_raw_mode()` and
    /// constructs the terminal. Callers must **not** have called
    /// `set_raw_mode()` from `input.rs` before this — the two raw-mode
    /// paths conflict.  Call `restore()` on exit to leave the terminal
    /// in cooked mode.
    pub fn new(start_time: std::time::Instant) -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        // Enable bracketed paste so pasted multi-line blocks are delivered as
        // a single paste event rather than individual keypresses.  Enable focus
        // reporting (DEC mode 1004) so the terminal emits ESC[I / ESC[O when this
        // tmux pane is re-focused — the cue used to re-pin the input box to the
        // bottom after a pane switch (see `reanchor`).
        use crossterm::event::{EnableBracketedPaste, EnableFocusChange};
        use crossterm::execute;
        let _ = execute!(std::io::stdout(), EnableBracketedPaste, EnableFocusChange);
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
            palette: crate::cli::palette::Palette::from_env(),
        })
    }
}

impl<B: Backend> RatatuiRenderer<B> {
    /// Commit one or more finished transcript lines into scrollback above
    /// the inline viewport.  Plain text, no styling.
    pub fn commit(&mut self, lines: &str) -> Result<(), B::Error> {
        let row_count = lines.matches('\n').count() + 1;
        self.terminal.insert_before(row_count as u16, |buf| {
            let area = buf.area;
            for (i, line) in lines.split('\n').enumerate() {
                let y = i as u16;
                if y >= area.height {
                    break;
                }
                let text = truncate_with_ellipsis(line, area.width as usize);
                buf.set_string(area.x, area.y + y, &text, Style::default());
            }
        })
    }

    /// Commit already-styled lines into scrollback above the inline viewport.
    ///
    /// Each `Line` carries its own `Span`s with `Style` — the `Paragraph`
    /// widget renders them into the `insert_before` buffer so styling becomes
    /// real cell attributes, not literal ANSI escape bytes.
    pub fn commit_styled(&mut self, lines: &[Line<'static>]) -> Result<(), B::Error> {
        let row_count = lines.len().max(1);
        self.terminal.insert_before(row_count as u16, |buf| {
            let text: ratatui::text::Text<'static> = lines.to_vec().into();
            let para = Paragraph::new(text);
            para.render(buf.area, buf);
        })
    }

    /// Draw the live region: input box and status bar.
    pub fn draw(&mut self, input: &InputLine, status: &StatusBarState<'_>) -> Result<(), B::Error> {
        // Split on '\n' so each logical line is a separate ratatui Line.
        let s = input.as_str();
        let input_text: ratatui::text::Text<'_> =
            s.split('\n').map(|l| Line::from(Span::raw(l))).collect();
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;

        let _completed = self.terminal.draw(|frame| {
            let area = frame.area();
            let content_width = area.width.saturating_sub(2) as usize;
            let (vis_row, vis_col) = input.cursor_visual_pos(content_width);
            let cursor_pos = Some((vis_col as u16, vis_row as u16));

            render_live_region(
                frame,
                area,
                &input_text,
                &session_id,
                &model,
                start_time,
                cursor_pos,
            );
        })?;
        Ok(())
    }

    /// Draw the live region with a spinner message in the input box area.
    ///
    /// The spinner is transient — it lives only in the `draw` frame and leaves
    /// no residue in scrollback.
    pub fn draw_spinner(
        &mut self,
        spinner_frame: &str,
        verb: &str,
        dot_count: usize,
        status: &StatusBarState<'_>,
    ) -> Result<(), B::Error> {
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;

        let blood_red = Style::default()
            .fg(self.palette.red())
            .add_modifier(Modifier::BOLD);
        let bright_yellow = Style::default().fg(self.palette.yellow());
        let (open, center, close) =
            if spinner_frame.starts_with('(') && spinner_frame.ends_with(')') {
                let inner = &spinner_frame[1..spinner_frame.len() - 1];
                ("(", inner.to_string(), ")")
            } else {
                ("", spinner_frame.to_string(), "")
            };
        let spinner_line = Line::from(vec![
            Span::styled(open, blood_red),
            Span::styled(center, bright_yellow),
            Span::styled(close, blood_red),
            Span::styled(format!(" {verb}"), blood_red),
            Span::styled(".".repeat(dot_count), bright_yellow),
        ]);

        let _completed = self.terminal.draw(|f| {
            let area = f.area();
            render_spinner_region(
                f,
                area,
                spinner_line.clone(),
                &session_id,
                &model,
                start_time,
            );
        })?;
        Ok(())
    }

    /// Draw the live region with a prompt and editable input line.
    ///
    /// The prompt is transient — it lives only in the `draw` frame and leaves
    /// no residue in scrollback.  The input text (from `InputLine`) is shown
    /// after the prompt label inside the bordered input box.
    pub fn draw_prompt(
        &mut self,
        prompt: &str,
        input: &InputLine,
        status: &StatusBarState<'_>,
    ) -> Result<(), B::Error> {
        let input_text = input.as_str();
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;
        let prompt_owned = prompt.to_string();

        let _completed = self.terminal.draw(|frame| {
            let area = frame.area();
            render_prompt_region(
                frame,
                area,
                &prompt_owned,
                &input_text,
                &session_id,
                &model,
                start_time,
            );
        })?;
        Ok(())
    }

    /// Commit a styled bordered panel into scrollback above the inline viewport.
    ///
    /// Renders a top border with a title, body lines (optionally dimmed and
    /// truncated), and a bottom border — the same logical structure as the
    /// legacy `print_tool_panel`, but as real styled cells with no literal
    /// ANSI escapes.
    pub fn commit_panel(
        &mut self,
        title: &str,
        body: &[String],
        dim_body: bool,
    ) -> Result<(), B::Error> {
        use ratatui::widgets::Clear;

        let w = self.terminal.size().map(|s| s.width as usize).unwrap_or(80);
        let inner = w.saturating_sub(2).max(2);
        let title_len = title.chars().count();
        let fill = inner.saturating_sub(title_len + 4);

        let border_color = self.palette.red();
        let title_color = self.palette.yellow();

        let border_style = Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD);
        let title_style = Style::default()
            .fg(title_color)
            .add_modifier(Modifier::BOLD);

        let top_border_spans: Vec<Span<'static>> = vec![
            Span::styled("╭─ ".to_string(), border_style),
            Span::styled(title.to_string(), title_style),
            Span::styled(format!(" {}─╮", "─".repeat(fill)), border_style),
        ];
        let bottom_border = format!("╰{}╯", "─".repeat(inner));

        let mut lines: Vec<Line<'static>> = Vec::new();

        // Top border
        lines.push(Line::from(top_border_spans));

        // Body lines
        let body_style = if dim_body {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };
        for line in body {
            let truncated = truncate_with_ellipsis(line, inner.saturating_sub(2));
            lines.push(Line::from(Span::styled(
                format!("  {}", truncated),
                body_style,
            )));
        }

        // Bottom border
        lines.push(Line::from(Span::styled(bottom_border, border_style)));

        // Blank line after panel
        lines.push(Line::from(vec![]));

        let row_count = lines.len();
        self.terminal.insert_before(row_count as u16, |buf| {
            Clear.render(buf.area, buf);
            let text: ratatui::text::Text<'static> = lines.into();
            let para = Paragraph::new(text);
            para.render(buf.area, buf);
        })
    }

    /// Re-pin the inline viewport to the bottom of the terminal.
    ///
    /// When a tmux pane is switched away from and back (or detached and
    /// re-attached), tmux repaints the pane and ratatui's cached viewport
    /// origin can go stale — the input box ends up drawn at the *top* of the
    /// pane with the history below it. `Terminal::resize` recomputes the inline
    /// viewport origin from the live cursor position, re-anchoring it to the
    /// bottom rows. It is safe to call when the size is unchanged (the normal
    /// `autoresize` path would otherwise skip the recompute).
    pub fn reanchor(&mut self) {
        if let Ok(size) = self.terminal.size() {
            let area = Rect::new(0, 0, size.width, size.height);
            let _ = self.terminal.resize(area);
        }
    }

    /// Restore the terminal to its original state (exit raw mode, show
    /// cursor, clear inline viewport rows).
    pub fn restore(&mut self) {
        use crossterm::event::{DisableBracketedPaste, DisableFocusChange};
        use crossterm::execute;
        let _ = execute!(std::io::stdout(), DisableFocusChange, DisableBracketedPaste);
        let _ = ratatui::try_restore();
    }
}

/// Render the live (non-committed) region: input box + status bar.
fn render_live_region(
    frame: &mut ratatui::Frame,
    area: Rect,
    input_text: &ratatui::text::Text<'_>,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
    cursor_pos: Option<(u16, u16)>, // (col, row) within content area (before scroll)
) {
    let (spinner_rect, body) = split_spinner_row(area);
    let _spinner_rect = spinner_rect; // reserved, blank in this mode

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(body);

    // ── Input box ──────────────────────────────────────────────
    let content_area = chunks[0];
    let content_height = content_area.height.saturating_sub(2) as usize; // minus borders
    let _content_width = content_area.width.saturating_sub(2) as usize;

    // Compute scroll offset so the cursor row stays visible
    let scroll_offset = if let Some((_, cursor_row)) = cursor_pos {
        let cursor_row = cursor_row as usize;
        if cursor_row >= content_height {
            (cursor_row + 1).saturating_sub(content_height) as u16
        } else {
            0
        }
    } else {
        0
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Gray));
    let input_para = Paragraph::new(input_text.clone())
        .block(input_block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));
    frame.render_widget(input_para, content_area);

    // ── Set cursor position (clamped to visible content area) ──
    if let Some((col, row)) = cursor_pos {
        let visible_row = (row as usize).saturating_sub(scroll_offset as usize);
        let x = content_area.x + 1 + col.min(content_area.width.saturating_sub(2));
        let y =
            content_area.y + 1 + (visible_row as u16).min(content_area.height.saturating_sub(2));
        frame.set_cursor_position((x, y));
    }

    // ── Status bar ─────────────────────────────────────────────
    let uptime = fmt_uptime(start_time.elapsed());
    let status_text = format!(
        " session:{} · {} · up {} ",
        short_session(session_id),
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

/// Render the live region with a prompt above the input box.
///
/// Layout: top rows show the prompt text, the bottom rows show the
/// bordered input box (with the current input text) and the status bar.
fn render_prompt_region(
    frame: &mut ratatui::Frame,
    area: Rect,
    prompt: &str,
    input_text: &str,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
) {
    let (spinner_rect, body) = split_spinner_row(area);

    if area.height < 4 {
        // Genuinely too small for prompt + box + status — existing fallback.
        let it: ratatui::text::Text<'_> = input_text
            .split('\n')
            .map(|l| Line::from(Span::raw(l)))
            .collect();
        render_live_region(frame, area, &it, session_id, model, start_time, None);
        return;
    }

    // Rows the prompt, box and status bar are drawn into. When the reserved
    // spinner row exists (height >= MIN_HEIGHT_FOR_SPINNER_ROW) the prompt
    // takes it and the box keeps its stable row. When it does not, carve a
    // one-row prompt strip off the top of `body` instead — the box shifts, but
    // at this size a visible prompt matters more than a stable box.
    let (prompt_rect, rest) = if spinner_rect.height > 0 {
        (spinner_rect, body)
    } else {
        let split = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(body);
        (split[0], split[1])
    };

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(rest);

    // ── Prompt text ────────────────────────────────────────────
    let prompt_line = Line::from(Span::styled(
        prompt,
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    ));
    let prompt_para = Paragraph::new(prompt_line);
    frame.render_widget(prompt_para, prompt_rect);

    // ── Input box ──────────────────────────────────────────────
    let input_text_obj: ratatui::text::Text<'_> = input_text
        .split('\n')
        .map(|l| Line::from(Span::raw(l)))
        .collect();
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Gray));
    let input_para = Paragraph::new(input_text_obj)
        .block(input_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(input_para, chunks[0]);

    // ── Status bar ─────────────────────────────────────────────
    let uptime = fmt_uptime(start_time.elapsed());
    let status_text = format!(
        " session:{} · {} · up {} ",
        short_session(session_id),
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

/// Render the live region with a spinner message replacing the input box content.
fn render_spinner_region(
    frame: &mut ratatui::Frame,
    area: Rect,
    spinner_line: Line<'static>,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
) {
    let (spinner_rect, body) = split_spinner_row(area);

    // ── Spinner line in the reserved row ───────────────────────
    let spinner_para = Paragraph::new(spinner_line);
    frame.render_widget(spinner_para, spinner_rect);

    // ── Empty bordered input box ───────────────────────────────
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(body);
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Gray));
    let input_para = Paragraph::new("").block(input_block);
    frame.render_widget(input_para, chunks[0]);

    // ── Status bar ─────────────────────────────────────────────
    let uptime = fmt_uptime(start_time.elapsed());
    let status_text = format!(
        " session:{} · {} · up {} ",
        short_session(session_id),
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

/// Clip `s` to at most `max` characters, marking truncation with a trailing '…'.
/// Returns `s` unchanged when it already fits. The '…' counts toward `max`, so a
/// truncated result is exactly `max` chars wide.
fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let kept: String = s.chars().take(max - 1).collect();
        format!("{kept}…")
    }
}

/// Abbreviate a session id for the status bar: the first 8 chars followed by '…'
/// when longer, otherwise the id unchanged.
fn short_session(id: &str) -> String {
    if id.chars().count() <= 8 {
        id.to_string()
    } else {
        let head: String = id.chars().take(8).collect();
        format!("{head}…")
    }
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
            palette: crate::cli::palette::Palette::for_depth(
                crate::cli::palette::ColorDepth::Truecolor,
            ),
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

    #[test]
    fn commit_styled_renders_into_buffer_without_escapes() {
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
        use crate::cli::palette::{ColorDepth, Palette};

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
            palette: Palette::for_depth(ColorDepth::Xterm256),
        };

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
    fn truncate_with_ellipsis_leaves_short_string_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
        assert!(!truncate_with_ellipsis("hello", 10).contains('…'));
        assert!(!truncate_with_ellipsis("hello", 5).contains('…'));
    }

    #[test]
    fn truncate_with_ellipsis_marks_overflow() {
        let result = truncate_with_ellipsis("hello world", 8);
        assert_eq!(result.chars().count(), 8);
        assert!(result.ends_with('…'));
        assert_eq!(result, "hello w…");
    }

    #[test]
    fn truncate_with_ellipsis_zero_max_is_empty() {
        assert_eq!(truncate_with_ellipsis("hello", 0), "");
    }

    #[test]
    fn short_session_marks_long_id() {
        let result = short_session("abcdef12-3456-7890-abcd-ef1234567890");
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), 9); // 8 chars + ellipsis
        assert!(result.starts_with("abcdef12"));
    }

    #[test]
    fn short_session_leaves_short_id_unchanged() {
        assert_eq!(short_session("short"), "short");
        assert_eq!(short_session("abcdef12"), "abcdef12");
        assert!(!short_session("short").contains('…'));
        assert!(!short_session("abcdef12").contains('…'));
    }

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
}
