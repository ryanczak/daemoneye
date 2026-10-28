//! `RatatuiRenderer's drawing and committing methods, for any backend:
//! `draw` and its panel variants, `commit` and `commit_panel`, and the repaint
//! history they keep.

use super::*;

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
