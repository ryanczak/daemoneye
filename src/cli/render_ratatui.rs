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

/// Cap on the committed-line history ring used for reanchor repaints. Only
/// the last screenful ever gets repainted, so this just needs to exceed any
/// realistic pane height.
const MAX_REPAINT_HISTORY: usize = 2048;

/// Repaint plan for a reanchor once the screen has scrolled: where to start
/// clearing, where to paint the history tail, and how many rows of it.
///
/// Once any `insert_before` has scrolled the screen, absolute row arithmetic
/// (`origin_row + inserted_rows`) no longer describes reality — tmux may have
/// shifted the grid by amounts the application cannot observe (screen scroll,
/// height-shrink reflow, bottom-park gaps being consumed). The only safe
/// recovery is to clear everything that could hold committed content or live
/// -region debris and repaint the transcript tail from the history ring,
/// bottom-anchored against the future viewport top (`park`).
///
/// `content_top` is the highest row that may still hold committed content:
/// `origin_row - rows_scrolled` (everything above it is pre-renderer shell
/// output that only ever moves into scrollback, never into our region).
/// Returns `(clear_from, paint_top, paint_rows)`.
fn scrolled_repaint_plan(content_top: u16, history_rows: u16, park: u16) -> (u16, u16, u16) {
    let clear_from = content_top.min(park);
    let paint_rows = history_rows.min(park - clear_from);
    (clear_from, park - paint_rows, paint_rows)
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
    /// reanchors means tmux reflowed the grid — `reanchor` then takes the
    /// repaint path instead of trusting row arithmetic.
    last_width: u16,
    /// Pane height at the last reanchor (0 until the first one). A change
    /// means tmux shifted the grid vertically by an unobservable amount.
    last_height: u16,
    /// Rows scrolled off the top of the screen by `insert_before` since the
    /// last reanchor normalization: the shortfall between rows inserted and
    /// how far the viewport actually moved down. Zero means the screen has
    /// never scrolled and absolute row arithmetic is still exact; non-zero
    /// switches `reanchor` to the repaint-from-history path.
    rows_scrolled: u16,
    /// Ring of committed transcript rows (one `Line` per screen row), used to
    /// repaint the visible tail deterministically on reanchor. Capped at
    /// `MAX_REPAINT_HISTORY`.
    history: std::collections::VecDeque<Line<'static>>,
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
        let (last_width, last_height) = terminal
            .size()
            .map(|s| (s.width, s.height))
            .unwrap_or((0, 0));
        Ok(Self {
            terminal,
            start_time,
            palette: crate::cli::palette::Palette::from_env(),
            origin_row,
            inserted_rows: 0,
            last_width,
            last_height,
            rows_scrolled: 0,
            history: std::collections::VecDeque::new(),
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
        let park = size.height.saturating_sub(VIEWPORT_ROWS);
        let old_w = self.last_width;
        let old_h = self.last_height;
        self.last_width = size.width;
        self.last_height = size.height;

        // A pane resize reflows/shifts the grid by amounts we cannot observe,
        // so even a never-scrolled layout stops being trustworthy: force the
        // repaint path whenever the size changed.
        let resized = old_h != size.height || old_w != size.width;

        let (clear_from, paint_top, paint_rows) = if self.rows_scrolled == 0 && !resized {
            // Exact regime: nothing has ever scrolled and the pane size is
            // unchanged, so every committed row is still on screen where we
            // put it and `origin_row + inserted_rows` is the true end of
            // committed content. Clear from the highest trustworthy marker;
            // nothing needs repainting.
            let content_end = self.origin_row.saturating_add(self.inserted_rows);
            let (clear_from, _park) = repin_rows(old_top, content_end, size.height);
            (clear_from, 0, 0)
        } else {
            // Repaint regime: the screen scrolled and/or the pane was
            // resized, so absolute arithmetic is stale. Clear everything that
            // may hold committed content or live-region debris and repaint
            // the transcript tail from the history ring. A height shrink can
            // additionally shift the grid up by as much as the height delta;
            // a width change reflows unpredictably, so clear the whole
            // region in that case.
            let content_top = if old_w != size.width {
                0
            } else {
                self.origin_row
                    .saturating_sub(self.rows_scrolled)
                    .saturating_sub(old_h.saturating_sub(size.height))
            };
            let history_rows = u16::try_from(self.history.len()).unwrap_or(u16::MAX);
            scrolled_repaint_plan(content_top, history_rows, park)
        };

        if std::env::var("DAEMONEYE_REANCHOR_TRACE").is_ok()
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/daemoneye-reanchor.log")
        {
            use std::io::Write as _;
            let _ = writeln!(
                f,
                "reanchor old_top={old_top} inserted={} scrolled={} park={park} w={} h={} old_w={old_w} clear_from={clear_from} paint_top={paint_top} paint_rows={paint_rows}",
                self.inserted_rows, self.rows_scrolled, size.width, size.height
            );
        }

        let mut out = std::io::stdout();
        if execute!(out, MoveTo(0, clear_from), Clear(ClearType::FromCursorDown)).is_err() {
            return;
        }
        if paint_rows > 0 {
            let tail: Vec<Line<'static>> = self
                .history
                .iter()
                .skip(self.history.len().saturating_sub(paint_rows as usize))
                .cloned()
                .collect();
            let area = Rect {
                x: 0,
                y: paint_top,
                width: size.width,
                height: paint_rows,
            };
            let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
            if let Ok(mut fixed) = Terminal::with_options(
                backend,
                ratatui::TerminalOptions {
                    viewport: ratatui::Viewport::Fixed(area),
                },
            ) {
                let _ = fixed.draw(|frame| {
                    let text: ratatui::text::Text<'static> = tail.into();
                    frame.render_widget(Paragraph::new(text), frame.area());
                });
            }
        }
        if execute!(out, MoveTo(0, park)).is_err() {
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
            if self.rows_scrolled > 0 {
                // Normalize back into the exact regime: committed content now
                // provably ends at `park` (the repainted tail), so absolute
                // arithmetic is trustworthy again until the next scroll.
                self.origin_row = paint_top;
                self.inserted_rows = paint_rows;
                self.rows_scrolled = 0;
            }
        }
    }
}

impl<B: Backend> RatatuiRenderer<B> {
    /// Record committed rows in the repaint history ring and account for any
    /// screen scrolling the insert caused. Call with the viewport top row as
    /// it was before the `insert_before`: the shortfall between rows inserted
    /// and how far the viewport actually moved down is exactly how many rows
    /// scrolled off the top of the region.
    fn record_committed_rows(&mut self, rows: Vec<Line<'static>>, viewport_top_before: u16) {
        let row_count = rows.len() as u16;
        for line in rows {
            if self.history.len() >= MAX_REPAINT_HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(line);
        }
        let moved = self
            .terminal
            .get_frame()
            .area()
            .y
            .saturating_sub(viewport_top_before);
        self.rows_scrolled = self
            .rows_scrolled
            .saturating_add(row_count.saturating_sub(moved));
    }

    /// Commit one or more finished transcript lines into scrollback above
    /// the inline viewport.  Plain text, no styling.
    pub fn commit(&mut self, lines: &str) -> Result<(), B::Error> {
        let row_count = lines.matches('\n').count() + 1;
        self.inserted_rows = self.inserted_rows.saturating_add(row_count as u16);
        let viewport_top = self.terminal.get_frame().area().y;
        let result = self.terminal.insert_before(row_count as u16, |buf| {
            let area = buf.area;
            for (i, line) in lines.split('\n').enumerate() {
                let y = i as u16;
                if y >= area.height {
                    break;
                }
                let text = truncate_with_ellipsis(line, area.width as usize);
                buf.set_string(area.x, area.y + y, &text, Style::default());
            }
        });
        let rows: Vec<Line<'static>> = lines
            .split('\n')
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect();
        self.record_committed_rows(rows, viewport_top);
        result
    }

    /// Commit already-styled lines into scrollback above the inline viewport.
    ///
    /// Each `Line` carries its own `Span`s with `Style` — the `Paragraph`
    /// widget renders them into the `insert_before` buffer so styling becomes
    /// real cell attributes, not literal ANSI escape bytes.
    pub fn commit_styled(&mut self, lines: &[Line<'static>]) -> Result<(), B::Error> {
        let row_count = lines.len().max(1);
        self.inserted_rows = self.inserted_rows.saturating_add(row_count as u16);
        let viewport_top = self.terminal.get_frame().area().y;
        let result = self.terminal.insert_before(row_count as u16, |buf| {
            let text: ratatui::text::Text<'static> = lines.to_vec().into();
            let para = Paragraph::new(text);
            para.render(buf.area, buf);
        });
        let mut rows = lines.to_vec();
        if rows.is_empty() {
            rows.push(Line::default());
        }
        self.record_committed_rows(rows, viewport_top);
        result
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
    /// bordered panel (yellow title) holding the multicolor Y/A/N options line
    /// and the editable input line; the command it approves is in the
    /// scrollback panel directly above. The status bar keeps the bottom row.
    /// Transient — leaves no residue in scrollback.
    pub fn draw_approval_panel(
        &mut self,
        title: &str,
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

            let content = Paragraph::new(vec![
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

    /// Draw the live region as a picker: a bordered list of `rows` under
    /// `title`, showing the window of rows that keeps `selected` in view, with
    /// the selected row marked `›` and drawn reversed. Too short for a panel,
    /// it shows the selected row alone on the prompt line.
    pub fn draw_picker_panel(
        &mut self,
        title: &str,
        rows: &[String],
        selected: usize,
        status: &StatusBarState<'_>,
    ) -> Result<(), B::Error> {
        let area = self.terminal.get_frame().area();
        if area.height < 6 {
            let line = rows.get(selected).map(String::as_str).unwrap_or("");
            return self.draw_prompt(
                &format!("{line} · ↑↓ enter esc › "),
                &InputLine::new(),
                status,
            );
        }

        let session_id = status.session_id.to_string();
        let model = status.model.to_string();
        let start_time = self.start_time;
        let title_owned = title.to_string();

        self.terminal.draw(|frame| {
            let area = frame.area();
            let chunks = Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(area);

            let yellow = self.palette.yellow();
            let panel = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    format!(" {title_owned} "),
                    Style::default().fg(yellow).add_modifier(Modifier::BOLD),
                ));

            let inner_height = chunks[0].height.saturating_sub(2).max(1) as usize;
            let inner_width = chunks[0].width.saturating_sub(4) as usize;
            let (start, end) = picker_window(rows.len(), selected, inner_height);
            let lines: Vec<Line> = rows[start..end]
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let text = truncate_with_ellipsis(row, inner_width);
                    if start + i == selected {
                        Line::from(Span::styled(
                            format!("› {text}"),
                            Style::default().add_modifier(Modifier::REVERSED),
                        ))
                    } else {
                        Line::from(Span::raw(format!("  {text}")))
                    }
                })
                .collect();
            frame.render_widget(Paragraph::new(lines).block(panel), chunks[0]);

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
        let viewport_top = self.terminal.get_frame().area().y;
        let history_rows = lines.clone();
        let result = self.terminal.insert_before(row_count as u16, |buf| {
            Clear.render(buf.area, buf);
            let text: ratatui::text::Text<'static> = lines.into();
            let para = Paragraph::new(text);
            para.render(buf.area, buf);
        });
        self.record_committed_rows(history_rows, viewport_top);
        result
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
/// The rows `[start, end)` a picker of `len` rows shows in `height` lines:
/// all of them when they fit, otherwise a window that ends at `selected` once
/// it has moved past the first `height` rows.
pub(crate) fn picker_window(len: usize, selected: usize, height: usize) -> (usize, usize) {
    if len <= height {
        return (0, len);
    }
    let start = (selected + 1).saturating_sub(height).min(len - height);
    (start, start + height)
}

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
#[path = "render_ratatui_layout_tests.rs"]
mod layout_tests;
#[cfg(test)]
#[path = "render_ratatui_tests.rs"]
mod tests;
