//! Drawing `top`: one frame of header, panels or detail, and footer, and the
//! windowing that keeps the rows that matter on screen.

use super::*;
use crate::cli::commands::top::text_frame::ALERTS_MAX_LINES;

/// Draw one frame at the wall clock: [`draw_at`] with the local zone now.
pub fn draw(frame: &mut ratatui::Frame<'_>, model: &TopModel) {
    draw_at(frame, model, Local::now().fixed_offset());
}

/// Draw one frame at `now`: the header line, four bordered blocks over the
/// panel texts laid out by [`plan_layout`], the selected run's line
/// reversed, finished runs dimmed, and a footer naming the keys and the
/// mode on the last row. An overflowing body shows the lines that matter:
/// Activity its newest rows, Runs a window that keeps the selected row (and
/// the column header) on screen. The clock is a parameter, so the drawn
/// header time is testable without racing the wall clock.
///
/// With a detail open the four panels are replaced by one bordered panel
/// over the run's detail text, still under the header line and above the
/// footer, with the detail's own footer naming `esc back`.
pub fn draw_at(frame: &mut ratatui::Frame<'_>, model: &TopModel, now: DateTime<FixedOffset>) {
    let area = frame.area();
    if let Some(attach) = &model.attach {
        crate::cli::tail::render_tail(frame, &attach.view.tail);
        return;
    }
    let (rects, _heights) = layout_at(model, area);
    if let Some(detail) = &model.detail {
        draw_detail_at(frame, model, detail, &rects, area, now);
        return;
    }
    let runs_body = runs_text(&model.snapshot.runs);
    let budget_body = budget_text(&model.snapshot.budget);
    let shells_body = shells_text(&model.snapshot.shells);
    let activity_body = activity_text(&model.filtered_activity());

    let host = crate::daemon::utils::daemon_hostname();
    let header = header_text(model.snapshot.daemon.as_ref(), &host, now);
    Paragraph::new(Line::raw(header)).render(rects.header, frame.buffer_mut());

    let faded_runs: Vec<usize> = model
        .snapshot
        .runs
        .iter()
        .enumerate()
        .filter(|(_, r)| run_fades(r))
        .map(|(i, _)| i)
        .collect();
    let (runs_display, runs_sel, kept_lines) =
        runs_window(&runs_body, model.selected, inner_height(rects.runs));
    // Run row i is body line i + 1: the body's line 0 is the column header.
    // Map each faded row through runs_window's kept indices, so the
    // window arithmetic exists in one place.
    let display_fades: Vec<usize> = faded_runs
        .iter()
        .map(|i| i + 1)
        .filter_map(|body_line| kept_lines.iter().position(|l| l == &body_line))
        .collect();
    draw_panel(
        frame,
        rects.runs,
        &panel_title(Panel::Runs),
        &runs_display,
        runs_sel,
        &display_fades,
        model.focus == Panel::Runs,
    );

    let bar = budget_bar(
        model.snapshot.budget.ai_cost,
        model.snapshot.budget.cap,
        // The panel shows `[` + cells + `] NN%`; the cells leave room for the
        // brackets and the percentage tail.
        inner_width(rects.budget).saturating_sub(BAR_CHROME),
    );
    let mut budget_lines: Vec<Line<'_>> = budget_body
        .lines()
        .map(|l| Line::raw(l.to_string()))
        .collect();
    if let Some((bar_text, level)) = bar {
        let colour = match level {
            BarLevel::Ok => Color::Reset,
            BarLevel::Warn => Color::Yellow,
            BarLevel::Over => Color::Red,
        };
        budget_lines.push(Line::styled(bar_text, Style::default().fg(colour)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(panel_title_line(
            Panel::Budget,
            model.focus == Panel::Budget,
        ));
    let inner = block.inner(rects.budget);
    block.render(rects.budget, frame.buffer_mut());
    Paragraph::new(budget_lines).render(inner, frame.buffer_mut());

    let shells_sel = shells_selected_line(model, inner_height(rects.shells));
    let shells_window = window_text(&shells_body, model.shells_top, inner_height(rects.shells));
    draw_panel(
        frame,
        rects.shells,
        &panel_title(Panel::Shells),
        &shells_window,
        shells_sel,
        &[],
        model.focus == Panel::Shells,
    );

    // The Alerts panel is not focusable: it takes no Panel and never gets
    // the focus colour or a selection. The severity word carries the row's
    // colour — the Budget bar's levels, gray for the rest.
    let alerts_window = alerts_body_text(&model.snapshot.alerts, now);
    draw_alerts_panel(frame, rects.alerts, &alerts_window);

    let activity_total = activity_body.lines().count();
    let activity_window = if model.tailing {
        // The newest rows fill the panel: the tail starts where the last
        // full window starts.
        window_text(
            &activity_body,
            activity_total.saturating_sub(inner_height(rects.activity)),
            inner_height(rects.activity),
        )
    } else {
        window_text(
            &activity_body,
            model.activity_top,
            inner_height(rects.activity),
        )
    };
    draw_panel(
        frame,
        rects.activity,
        &panel_title(Panel::Activity),
        &activity_window,
        None,
        &[],
        model.focus == Panel::Activity,
    );
    render_footer(frame, model, rects.footer);
}

/// The footer line: the key hints, the focused panel, the Runs-follow and
/// Activity-tail modes, the attach hint and note, the filter. Drops the
/// left hints (never `q quit`, how the user leaves) when it overflows.
fn render_footer(
    frame: &mut ratatui::Frame<'_>,
    model: &TopModel,
    footer_rect: ratatui::layout::Rect,
) {
    let mode = if model.follow {
        "following"
    } else {
        "scrolled"
    };
    let activity_mode = if model.tailing {
        "tailing"
    } else {
        "activity scrolled"
    };
    let mut footer = format!(
        "q quit \u{b7} \u{2191}\u{2193} scroll \u{b7} tab focus \u{b7} f follow \u{b7} / filter \u{b7} focus: {} \u{b7} {mode} \u{b7} {activity_mode}",
        model.focus.label()
    );
    if model.focus == Panel::Shells && model.shells_row_count().is_some() {
        footer.push_str(" \u{b7} enter attach");
    }
    if let Some(note) = &model.attach_note {
        footer.push_str(&format!(" \u{b7} {note}"));
    }
    if !model.filter.is_empty() {
        footer.push_str(&format!(" \u{b7} filter: {}", model.filter));
    }
    // An overflow would cut the right end — the newest segments, the attach
    // hint and an open error. Drop key hints from the left (never `q quit`,
    // how the user leaves) until the line fits.
    let width = footer_rect.width as usize;
    while footer.chars().count() > width {
        let Some(sep) = footer.char_indices().skip(2).find(|(_, c)| *c == '\u{b7}') else {
            break;
        };
        footer = footer[sep.0 + 2..].to_string();
    }
    Paragraph::new(Line::raw(footer)).render(footer_rect, frame.buffer_mut());
}

/// The selected shell's line index inside the Shells panel's drawn window,
/// `None` when the panel has no rows — the daemon-down mark and `none` are
/// never drawn reversed — or the selection is scrolled out. `shells_text`
/// emits one line per row, so body line i is shell row i.
fn shells_selected_line(model: &TopModel, inner_h: usize) -> Option<usize> {
    let rows = match &model.snapshot.shells {
        ShellsPanel::Rows(rows) if !rows.is_empty() => rows.len(),
        _ => return None,
    };
    let sel = model.shells_selected.min(rows - 1);
    let range = visible_range(rows, inner_h, model.shells_top);
    range.contains(&sel).then(|| sel - range.start)
}

/// A panel's title text: `<n> <Name>`. `draw_panel` and `panel_title_line`
/// put the one padding around it; no caller pads it twice.
fn panel_title(panel: Panel) -> String {
    format!("{} {}", panel.index() + 1, panel.label())
}

/// The detail view: the header line, one bordered panel titled `Run <id>`
/// over the detail text's visible window, and a footer naming `esc back`.
/// Scroll and the clamped window use the same helpers the panels do.
fn draw_detail_at(
    frame: &mut ratatui::Frame<'_>,
    model: &TopModel,
    detail: &OpenDetail,
    rects: &PanelRects,
    area: ratatui::layout::Rect,
    now: DateTime<FixedOffset>,
) {
    let host = crate::daemon::utils::daemon_hostname();
    let header = header_text(model.snapshot.daemon.as_ref(), &host, now);
    Paragraph::new(Line::raw(header)).render(rects.header, frame.buffer_mut());
    let panel = detail_rect(rects, area);
    let body = window_text(&detail.text, model.detail_top, inner_height(panel));
    draw_panel(
        frame,
        panel,
        &format!("Run {}", detail.id),
        &body,
        None,
        &[],
        true,
    );
    let footer = "esc back \u{b7} \u{2191}\u{2193} scroll \u{b7} q quit".to_string();
    Paragraph::new(Line::raw(footer)).render(rects.footer, frame.buffer_mut());
}

/// A panel's title line, cyan on the focused panel. Pads once, like
/// `draw_panel` does; the Budget block renders it as its own title.
fn panel_title_line(panel: Panel, focused: bool) -> Line<'static> {
    let mut style = Style::default().add_modifier(Modifier::BOLD);
    if focused {
        style = style.fg(Color::Cyan);
    }
    Line::styled(format!(" {} ", panel_title(panel)), style)
}

/// The Shells body's visible window: the lines `top` onward as far as the
/// panel's inner height shows, from the one pure range helper.
/// The Alerts panel's drawn body, at the wall clock `now`: the same lines
/// [`crate::cli::commands::top_tui::alerts_body`] lays out, with each line
/// carrying its severity colour so [`draw_alerts_panel`] can style the
/// severity word. One `Line` per alert, in the daemon's order.
fn alerts_body_text(
    panel: &crate::cli::commands::top::AlertsPanel,
    now: DateTime<FixedOffset>,
) -> Vec<Line<'_>> {
    let now_unix = now.timestamp().max(0) as u64;
    let rows = match panel {
        crate::cli::commands::top::AlertsPanel::Rows(rows) => rows,
        crate::cli::commands::top::AlertsPanel::DaemonDown => {
            return vec![Line::raw("daemon not running".to_string())];
        }
    };
    if rows.is_empty() {
        return vec![Line::raw("none".to_string())];
    }
    let shown = rows
        .len()
        .min(ALERTS_MAX_LINES - usize::from(rows.len() > ALERTS_MAX_LINES));
    let mut lines: Vec<Line<'_>> = rows[..shown]
        .iter()
        .map(|a| {
            let severity = a.severity.clone();
            let rest = format!(
                " {} firing {} seen {} ago",
                a.name,
                format_age(now_unix.saturating_sub(a.first_seen_unix) as i64),
                format_age(now_unix.saturating_sub(a.last_seen_unix) as i64),
            );
            severity_colour(&severity, |colour| {
                Line::from(vec![
                    Span::styled(severity.clone(), Style::default().fg(colour)),
                    Span::raw(rest.clone()),
                ])
            })
        })
        .collect();
    let more = rows.len() - shown;
    if more > 0 {
        lines.push(Line::raw(format!("+{more} more")));
    }
    lines
}

/// The colour a severity word carries: `critical` red, `warning` yellow,
/// everything else gray — the Budget bar's levels. `paint` builds the line
/// from the colour, so the caller keeps the spans in one place.
fn severity_colour<F>(severity: &str, paint: F) -> Line<'static>
where
    F: FnOnce(Color) -> Line<'static>,
{
    let colour = match severity {
        "critical" => Color::Red,
        "warning" => Color::Yellow,
        _ => Color::Reset,
    };
    paint(colour)
}

/// One bordered Alerts panel, full width, not focusable: no Panel, no
/// focus colour, no selection. The severity word of each row carries the
/// colour [`severity_colour`] picks.
fn draw_alerts_panel(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    lines: &[Line<'_>],
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Line::styled(
            " Alerts ",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());
    Paragraph::new(lines.to_vec()).render(inner, frame.buffer_mut());
}

fn window_text(body: &str, top: usize, inner_h: usize) -> String {
    let range = visible_range(body.lines().count(), inner_h, top);
    body.lines()
        .skip(range.start)
        .take(range.len())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lines a panel shows: `top` onward for `inner_h` lines, clamped so
/// the range always lies inside `0..total`. Empty when the content is
/// empty.
pub fn visible_range(total: usize, inner_h: usize, top: usize) -> std::ops::Range<usize> {
    let start = top.min(total.saturating_sub(inner_h));
    let end = start.saturating_add(inner_h).min(total);
    start..end
}

/// A bordered panel's inner height: the borders take one line at each edge.
pub(super) fn inner_height(area: ratatui::layout::Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

/// A bordered panel's inner width: the borders take one column at each edge.
fn inner_width(area: ratatui::layout::Rect) -> usize {
    area.width.saturating_sub(2) as usize
}

/// The Runs panel's body as drawn: when the rows overflow the panel, the
/// window keeps the column header on top and the selected run on screen.
/// The returned index is the selected line's position in the returned body,
/// `None` when the panel has no runs. `kept_lines` names, for every line of
/// the returned body, its index in `body` — so callers that need to style
/// one body line (a faded run) map through it instead of redoing the
/// window arithmetic.
fn runs_window(body: &str, selected: usize, inner_h: usize) -> (String, Option<usize>, Vec<usize>) {
    let line_count = body.lines().count();
    if line_count == 0 {
        return (body.to_string(), None, Vec::new());
    }
    if line_count <= inner_h {
        let sel = (line_count > 1).then(|| selected + 1);
        let kept = (0..line_count).collect();
        return (body.to_string(), sel, kept);
    }
    let rows_visible = inner_h.saturating_sub(1).max(1);
    let row = (selected + 1).min(line_count - 1);
    let mut start = 1;
    if row >= start + rows_visible {
        start = row + 1 - rows_visible;
    }
    let display = body
        .lines()
        .next()
        .into_iter()
        .chain(body.lines().skip(start).take(rows_visible))
        .collect::<Vec<_>>()
        .join("\n");
    let mut kept = vec![0usize];
    kept.extend(start..(start + rows_visible).min(line_count));
    (display, Some(1 + row - start), kept)
}

/// One bordered panel with its title and a plain-text body. `title` is the
/// text between the border and the one padding (`╭ <title> ─`); `selected`,
/// when given, is the line index inside `body` drawn reversed. `fades` is
/// the set of line indices drawn dimmed. The focused panel's border and
/// title are cyan.
fn draw_panel(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    title: &str,
    body: &str,
    selected: Option<usize>,
    fades: &[usize],
    focused: bool,
) {
    let mut style = Style::default().add_modifier(Modifier::BOLD);
    if focused {
        style = style.fg(Color::Cyan);
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        })
        .title(Line::styled(format!(" {title} "), style));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());
    let lines: Vec<Line<'_>> = body
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let style = if selected == Some(i) {
                Style::default().add_modifier(Modifier::REVERSED)
            } else if fades.contains(&i) {
                Style::default().add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };
            Line::styled(l.to_string(), style)
        })
        .collect();
    Paragraph::new(lines).render(inner, frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::AlertBadge;

    /// One `AlertBadge` with the given identity and stamps.
    fn alert_badge(name: &str, severity: &str, first: u64, last: u64) -> AlertBadge {
        AlertBadge {
            fingerprint: format!("fp-{name}"),
            name: name.to_string(),
            severity: severity.to_string(),
            first_seen_unix: first,
            last_seen_unix: last,
        }
    }

    #[test]
    fn alerts_panel_caps_and_counts_the_rest() {
        let alerts: Vec<_> = (1..=9)
            .map(|i| alert_badge(&format!("Alert{i}"), "critical", 100, 110))
            .collect();
        let mut snap = TopModel::new(super::super::tests::snapshot(vec![]));
        snap.snapshot.alerts = crate::cli::commands::top::AlertsPanel::Rows(alerts);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| draw(f, &snap)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        for i in 1..=5 {
            assert!(
                text.contains(&format!("Alert{i}")),
                "alert {i} is among the shown first five: {text:?}"
            );
        }
        assert!(
            text.contains("+4 more"),
            "the overflow line names the rest: {text:?}"
        );
        assert!(
            !text.contains("Alert6"),
            "the sixth alert is past the cap: {text:?}"
        );
    }

    #[test]
    fn alerts_panel_colours_the_severity() {
        let mut model = TopModel::new(super::super::tests::snapshot(vec![]));
        model.snapshot.alerts = crate::cli::commands::top::AlertsPanel::Rows(vec![
            alert_badge("DiskFull", "critical", 100, 110),
            alert_badge("HighCPU", "warning", 100, 110),
            alert_badge("StaleRun", "info", 100, 110),
        ]);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| draw(f, &model)).unwrap();
        // The severity word is the row's first span, so the cell that starts
        // the row carries its colour. Read row-wise: find the row whose text
        // names the alert, then its first cell inside the border.
        let width = 80usize;
        let cells = terminal.backend().buffer().content.clone();
        let severity_cell = |needle: &str| {
            let row = (0..cells.len() / width).find(|&y| {
                (0..width)
                    .filter_map(|x| cells.get(y * width + x))
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .contains(needle)
            });
            let y = row.expect("the alert row is drawn");
            cells
                .iter()
                .skip(y * width)
                .find(|c| c.symbol() != " " && c.symbol() != "│")
                .map(|c| c.style().fg)
        };
        assert_eq!(
            severity_cell("DiskFull").flatten(),
            Some(Color::Red),
            "a critical row's severity cell is red"
        );
        assert_eq!(
            severity_cell("HighCPU").flatten(),
            Some(Color::Yellow),
            "a warning row's severity cell is yellow"
        );
        assert_eq!(
            severity_cell("StaleRun").flatten(),
            Some(Color::Reset),
            "everything else is gray"
        );
    }
}
