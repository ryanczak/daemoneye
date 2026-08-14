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

/// Rows for a bottom repin: (clear_from, cursor_park).
///
/// `cursor_park` is the future viewport TOP (`height − VIEWPORT_ROWS`) —
/// see the phase-06 scroll-trap note for why never the bottom row.
/// `clear_from` starts the wipe at the highest of the safe rows: the old
/// viewport top, the end of real committed content (`content_end`), or the
/// park row — whichever is highest on screen. Clearing from `content_end`
/// is what removes stale live-region debris parked between history and the
/// bottom; the `park` clamp makes a full-scrolled session degrade to the
/// bottom-rows-only clear, which is correct there.
fn repin_rows(old_top: u16, content_end: u16, height: u16) -> (u16, u16) {
    let park = height.saturating_sub(VIEWPORT_ROWS);
    (old_top.min(content_end).min(park), park)
}

/// Rows the old live region occupies after tmux rewraps it at a new pane
/// width: ceil(VIEWPORT_ROWS × old_w / new_w). These rows sit at the bottom
/// of the screen, below committed content — guaranteed non-history — so a
/// reanchor after a width change may clear them freely. Returns 0 when the
/// width did not change or either width is 0 (no band; the width-blind wipe
/// is already correct). Capped at 4 × VIEWPORT_ROWS so a pathological
/// old_w/new_w ratio cannot wipe most of the screen.
fn ghost_band_rows(old_w: u16, new_w: u16) -> u16 {
    if old_w == new_w || old_w == 0 || new_w == 0 {
        return 0;
    }
    let band = (u32::from(VIEWPORT_ROWS) * u32::from(old_w)).div_ceil(u32::from(new_w));
    (band.min(4 * u32::from(VIEWPORT_ROWS))) as u16
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
    /// Viewport top row at construction — where committed content starts.
    origin_row: u16,
    /// Total rows ever passed to `insert_before` (saturating). origin_row +
    /// inserted_rows = the row just past real content, until the screen
    /// fills and the clamp in `repin_rows` takes over.
    inserted_rows: u16,
    /// Pane width at the last construction or reanchor. A change between
    /// reanchors means tmux rewrapped the old live region — see
    /// `ghost_band_rows`.
    last_width: u16,
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
        let mut terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
            },
        )?;
        let origin_row = terminal.get_frame().area().y;
        let last_width = terminal.size().map(|s| s.width).unwrap_or(0);
        Ok(Self {
            terminal,
            start_time,
            palette: crate::cli::palette::Palette::from_env(),
            origin_row,
            inserted_rows: 0,
            last_width,
        })
    }

    /// Deterministically re-pin the inline viewport to the bottom of the
    /// terminal after tmux moved or rewrapped the screen (window switch,
    /// resize). `Terminal::resize` cannot do this — it anchors relative to
    /// the live cursor and only clears on horizontal shrink — so instead:
    /// wipe from the old viewport top downward, park the cursor exactly at
    /// the new viewport top, and rebuild the Terminal (inline init anchors
    /// at the cursor row with offset 0).
    pub fn reanchor(&mut self) {
        use crossterm::cursor::MoveTo;
        use crossterm::execute;
        use crossterm::terminal::{Clear, ClearType};

        let Ok(size) = self.terminal.size() else {
            return;
        };
        let old_top = self.terminal.get_frame().area().y;
        let content_end = self.origin_row.saturating_add(self.inserted_rows);
        let (clear_from, park) = repin_rows(old_top, content_end, size.height);
        let old_w = self.last_width;
        let band = ghost_band_rows(old_w, size.width);
        self.last_width = size.width;
        let clear_from = clear_from.min(size.height.saturating_sub(band));
        if std::env::var("DAEMONEYE_REANCHOR_TRACE").is_ok()
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/daemoneye-reanchor.log")
        {
            use std::io::Write as _;
            let _ = writeln!(
                f,
                "reanchor old_top={old_top} content_end={content_end} park={park} w={} h={} old_w={} band={band} clear_from={clear_from}",
                size.width, size.height, old_w
            );
        }
        let mut out = std::io::stdout();
        if execute!(
            out,
            MoveTo(0, clear_from),
            Clear(ClearType::FromCursorDown),
            MoveTo(0, park)
        )
        .is_err()
        {
            return;
        }
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        if let Ok(terminal) = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
            },
        ) {
            self.terminal = terminal;
        }
    }
}

impl<B: Backend> RatatuiRenderer<B> {
    /// Commit one or more finished transcript lines into scrollback above
    /// the inline viewport.  Plain text, no styling.
    pub fn commit(&mut self, lines: &str) -> Result<(), B::Error> {
        let row_count = lines.matches('\n').count() + 1;
        self.inserted_rows = self.inserted_rows.saturating_add(row_count as u16);
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
        self.inserted_rows = self.inserted_rows.saturating_add(row_count as u16);
        self.terminal.insert_before(row_count as u16, |buf| {
            let text: ratatui::text::Text<'static> = lines.to_vec().into();
            let para = Paragraph::new(text);
            para.render(buf.area, buf);
        })
    }

    /// Draw the live region: input box and status bar.
    pub fn draw(&mut self, input: &InputLine, status: &StatusBarState<'_>) -> Result<(), B::Error> {
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;

        let _completed = self.terminal.draw(|frame| {
            let area = frame.area();
            let content_width = area.width.saturating_sub(2) as usize;
            // One wrapper for glyphs and cursor: visual_lines is the authority.
            let visual: Vec<String> = input
                .visual_lines(content_width)
                .into_iter()
                .map(|l| l.into_iter().collect())
                .collect();
            let input_text: ratatui::text::Text<'static> = visual
                .into_iter()
                .map(|l| Line::from(Span::raw(l)))
                .collect();
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

    /// Draw the live region as a themed approval dialog: a rounded blood-red
    /// bordered panel (yellow title) holding the command summary, the
    /// multicolor Y/A/N options line, and the editable input line; the status
    /// bar keeps the bottom row. Transient — leaves no residue in scrollback.
    pub fn draw_approval_panel(
        &mut self,
        title: &str,
        summary: &str,
        session_label: &str,
        input: &InputLine,
        status: &StatusBarState<'_>,
    ) -> Result<(), B::Error> {
        let area = self.terminal.get_frame().area();
        if area.height < 6 {
            // Too short for the panel — fall back to the plain prompt shape.
            return self.draw_prompt("Approve? [Y]es [A]pprove [N]o › ", input, status);
        }

        let input_text = input.as_str();
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;
        let title_owned = title.to_string();
        let summary_owned = summary.to_string();
        let session_label_owned = session_label.to_string();

        self.terminal.draw(|frame| {
            let area = frame.area();
            let chunks = Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(area);

            let red = self.palette.red();
            let yellow = self.palette.yellow();

            let panel = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(red))
                .title(Span::styled(
                    format!(" {title_owned} "),
                    Style::default().fg(yellow).add_modifier(Modifier::BOLD),
                ));

            let inner_width = area.width.saturating_sub(2) as usize;
            let content = Paragraph::new(vec![
                Line::from(Span::styled(
                    truncate_with_ellipsis(&summary_owned, inner_width),
                    Style::default().fg(Color::Gray),
                )),
                approval_options_line(&session_label_owned, red, yellow),
                Line::from(vec![
                    Span::styled("› ", Style::default().fg(yellow)),
                    Span::raw(input_text),
                ]),
            ])
            .block(panel);
            frame.render_widget(content, chunks[0]);

            let uptime = fmt_uptime(start_time.elapsed());
            let status_text = format!(
                " session:{} · {} · up {} ",
                short_session(&session_id),
                model,
                uptime,
            );
            let status_block = Block::default().borders(Borders::NONE).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            );
            let status_para =
                Paragraph::new(Line::from(Span::raw(status_text))).block(status_block);
            frame.render_widget(status_para, chunks[1]);
        })?;

        Ok(())
    }

    /// Draw the live region as a themed credential dialog: the phase-04 panel
    /// shape with the daemon's prompt text as the detail row, the Enter/Esc
    /// hint row, and the masked input row. The caller passes the bullet
    /// display buffer — the real credential never reaches the renderer.
    pub fn draw_credential_panel(
        &mut self,
        title: &str,
        detail: &str,
        input: &InputLine,
        status: &StatusBarState<'_>,
    ) -> Result<(), B::Error> {
        let area = self.terminal.get_frame().area();
        if area.height < 6 {
            // Too short for the panel — fall back to the plain prompt shape.
            return self.draw_prompt("  Password: ", input, status);
        }

        let input_text = input.as_str();
        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;
        let title_owned = title.to_string();
        let detail_owned = detail.to_string();

        self.terminal.draw(|frame| {
            let area = frame.area();
            let chunks = Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(area);

            let red = self.palette.red();
            let yellow = self.palette.yellow();

            let panel = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(red))
                .title(Span::styled(
                    format!(" {title_owned} "),
                    Style::default().fg(yellow).add_modifier(Modifier::BOLD),
                ));

            let inner_width = area.width.saturating_sub(2) as usize;
            let content = Paragraph::new(vec![
                Line::from(Span::styled(
                    truncate_with_ellipsis(&detail_owned, inner_width),
                    Style::default().fg(Color::Gray),
                )),
                credential_hint_line(red, yellow),
                Line::from(vec![
                    Span::styled("› ", Style::default().fg(yellow)),
                    Span::raw(input_text),
                ]),
            ])
            .block(panel);
            frame.render_widget(content, chunks[0]);

            let uptime = fmt_uptime(start_time.elapsed());
            let status_text = format!(
                " session:{} · {} · up {} ",
                short_session(&session_id),
                model,
                uptime,
            );
            let status_block = Block::default().borders(Borders::NONE).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            );
            let status_para =
                Paragraph::new(Line::from(Span::raw(status_text))).block(status_block);
            frame.render_widget(status_para, chunks[1]);
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
        self.commit_panel_labeled(title, body, dim_body, None)
    }

    pub fn commit_panel_labeled(
        &mut self,
        title: &str,
        body: &[String],
        dim_body: bool,
        bottom_label: Option<&str>,
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
            for seg in crate::cli::render::wrap_line_hard(line, inner.saturating_sub(2)) {
                lines.push(Line::from(Span::styled(format!("  {}", seg), body_style)));
            }
        }

        // Bottom border
        let bottom_line: Line<'static> = match bottom_label {
            Some(label) => {
                let padded = format!(" {label} ");
                let label_vis = padded.chars().count();
                let dashes = inner.saturating_sub(label_vis + 1);
                Line::from(vec![
                    Span::styled(format!("╰{}", "─".repeat(dashes)), border_style),
                    Span::styled(padded, title_style),
                    Span::styled("─╯".to_string(), border_style),
                ])
            }
            None => Line::from(Span::styled(
                format!("╰{}╯", "─".repeat(inner)),
                border_style,
            )),
        };
        lines.push(bottom_line);

        // Blank line after panel
        lines.push(Line::from(vec![]));

        let row_count = lines.len();
        self.inserted_rows = self.inserted_rows.saturating_add(row_count as u16);
        self.terminal.insert_before(row_count as u16, |buf| {
            Clear.render(buf.area, buf);
            let text: ratatui::text::Text<'static> = lines.into();
            let para = Paragraph::new(text);
            para.render(buf.area, buf);
        })
    }

    /// Re-pin the inline viewport to the bottom of the terminal.
    ///
    /// The input box's inner content width — the same value `draw` wraps
    /// with. Key handling must use this, not the tmux/ioctl-derived width.
    pub fn input_content_width(&self) -> usize {
        self.terminal
            .size()
            .map(|s| s.width.saturating_sub(2) as usize)
            .unwrap_or(78)
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
        .scroll((scroll_offset, 0));
    frame.render_widget(input_para, content_area);

    // ── Set cursor position (clamped to visible content area) ──
    if let Some((col, row)) = cursor_pos {
        let visible_row = (row as usize).saturating_sub(scroll_offset as usize);
        let x = content_area.x + 1 + col.min(content_area.width.saturating_sub(3));
        let y =
            content_area.y + 1 + (visible_row as u16).min(content_area.height.saturating_sub(3));
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

/// The approval options line: bright-yellow key letters in blood-red
/// brackets, dim redirect affordance. `session_label` is "session" or
/// "sudo session".
fn approval_options_line(session_label: &str, red: Color, yellow: Color) -> Line<'static> {
    let key =
        |c: &'static str| Span::styled(c, Style::default().fg(yellow).add_modifier(Modifier::BOLD));
    let br = |s: &'static str| Span::styled(s, Style::default().fg(red));
    let word = |s: String| Span::styled(s, Style::default().fg(red));
    Line::from(vec![
        br("["),
        key("Y"),
        br("]"),
        word("es".to_string()),
        Span::raw("  "),
        br("["),
        key("A"),
        br("]"),
        word(format!("pprove for {session_label}")),
        Span::raw("  "),
        br("["),
        key("N"),
        br("]"),
        word("o".to_string()),
        Span::raw("  "),
        Span::styled(
            "or type to redirect",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
    ])
}

/// The credential-dialog hint line: `[Enter] submit  [Esc] cancel`,
/// yellow key words in blood-red brackets, dim tail.
fn credential_hint_line(red: Color, yellow: Color) -> Line<'static> {
    let key =
        |c: &'static str| Span::styled(c, Style::default().fg(yellow).add_modifier(Modifier::BOLD));
    let br = |s: &'static str| Span::styled(s, Style::default().fg(red));
    Line::from(vec![
        br("["),
        key("Enter"),
        br("]"),
        Span::styled(" submit", Style::default().fg(red)),
        Span::raw("  "),
        br("["),
        key("Esc"),
        br("]"),
        Span::styled(" cancel", Style::default().fg(red)),
    ])
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
            origin_row: 0,
            inserted_rows: 0,
            last_width: 0,
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
            origin_row: 0,
            inserted_rows: 0,
            last_width: 0,
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
    fn commit_panel_wraps_long_body_lines() {
        let backend = TestBackend::new(60, 10);
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
            last_width: 0,
        };

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
        let backend = TestBackend::new(60, 10);
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
            last_width: 0,
        };

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
        let backend = TestBackend::new(60, 10);
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
            last_width: 0,
        };

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
            origin_row: 0,
            inserted_rows: 0,
            last_width: 0,
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
            last_width: 0,
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
    fn ghost_band_rows_zero_when_width_unchanged() {
        assert_eq!(ghost_band_rows(127, 127), 0);
        assert_eq!(ghost_band_rows(255, 255), 0);
    }

    #[test]
    fn ghost_band_rows_narrowing_ceils() {
        assert_eq!(ghost_band_rows(254, 127), 12);
        assert_eq!(ghost_band_rows(255, 127), 13);
    }

    #[test]
    fn ghost_band_rows_widening_small_band() {
        assert_eq!(ghost_band_rows(127, 255), 3);
        // The band never raises the clear row above the park on a widening.
        let h: u16 = 61;
        let park = h.saturating_sub(VIEWPORT_ROWS);
        assert_eq!(park, 55);
        assert_eq!(park.min(h.saturating_sub(ghost_band_rows(127, 255))), 55);
    }

    #[test]
    fn ghost_band_rows_zero_width_guard() {
        assert_eq!(ghost_band_rows(0, 127), 0);
        assert_eq!(ghost_band_rows(127, 0), 0);
    }

    #[test]
    fn ghost_band_rows_capped() {
        assert_eq!(ghost_band_rows(u16::MAX, 1), 4 * VIEWPORT_ROWS);
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
            last_width: 0,
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
        summary: &str,
        session_label: &str,
        input: &InputLine,
    ) {
        renderer
            .draw_approval_panel(
                "approve command",
                summary,
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
            last_width: 0,
        };

        let input = InputLine::new();
        draw_approval_panel_test(&mut renderer, "$ ls -la", "session", &input);

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
            last_width: 0,
        };

        let input = InputLine::new();
        draw_approval_panel_test(&mut renderer, "$ sudo apt install", "sudo session", &input);

        let rows = buffer_rows(&renderer);
        assert!(
            rows.iter()
                .any(|r| r.contains("[A]pprove for sudo session")),
            "options row should read [A]pprove for sudo session, got: {:?}",
            rows
        );
    }

    #[test]
    fn approval_panel_truncates_long_summary() {
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
            last_width: 0,
        };

        let input = InputLine::new();
        let long_summary = format!("$ {}", "x".repeat(300));
        draw_approval_panel_test(&mut renderer, &long_summary, "session", &input);

        let rows = buffer_rows(&renderer);
        let summary_row = rows
            .iter()
            .find(|r| r.contains('$'))
            .expect("summary row should be rendered");
        // The row is `│<summary>…│` — the ellipsis is the last content glyph
        // before the right border.
        assert!(
            summary_row.ends_with("…│"),
            "summary row should end with ellipsis before the border, got: {:?}",
            summary_row
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
            last_width: 0,
        };

        let mut input = InputLine::new();
        input.insert_str("why");
        draw_approval_panel_test(&mut renderer, "$ ls", "session", &input);

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
            last_width: 0,
        };

        let input = InputLine::new();
        // Must not panic — falls back to the plain prompt shape.
        renderer
            .draw_approval_panel(
                "approve command",
                "$ ls",
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
            last_width: 0,
        };

        let input = InputLine::new();
        draw_credential_panel_test(
            &mut renderer,
            "sudo password",
            "[sudo] password required for: /usr/bin/apt",
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
            last_width: 0,
        };

        let mut input = InputLine::new();
        input.insert('•');
        input.insert('•');
        input.insert('•');
        draw_credential_panel_test(
            &mut renderer,
            "sudo password",
            "[sudo] password required for: /usr/bin/apt",
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
            last_width: 0,
        };

        let input = InputLine::new();
        let long_detail = format!("[sudo] password required for: {}", "x".repeat(300));
        draw_credential_panel_test(&mut renderer, "sudo password", &long_detail, &input);

        let rows = buffer_rows(&renderer);
        let detail_row = rows
            .iter()
            .find(|r| r.contains("password required for"))
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
            last_width: 0,
        };

        let input = InputLine::new();
        // Must not panic — falls back to the plain prompt shape.
        renderer
            .draw_credential_panel(
                "sudo password",
                "[sudo] password required for: /usr/bin/apt",
                &input,
                &approval_test_status(),
            )
            .unwrap();
    }
}
