//! The live ratatui screen of `daemoneye top` (M30 phase 09). The same four
//! panels `top --once` prints, redrawn every 500 ms over
//! [`TopFingerprint`]: the disk panels are re-read only when the fingerprint
//! changes or the ages have gone stale, and the Shells panel every 2 s.
//! The selection follows the newest live run until the user scrolls; `/`
//! edits the Activity filter; `q` quits. Read-only: nothing is written.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};

use crate::cli::commands::top::{
    ActivityRow, BarLevel, Liveness, TopFingerprint, TopRunRow, activity_text, budget_bar,
    budget_text, collapse_activity, filter_activity, fingerprint, header_text, plan_layout,
    run_fades, runs_text, shells_text,
};
use crate::cli::viewer::AltScreenGuard;
/// Poll the disk fingerprint every this long.
const TICK: Duration = Duration::from_millis(500);

/// Re-read the disk panels when the fingerprint has sat unchanged this long —
/// ages and the 30 s liveness window move with the clock even when nothing
/// new landed.
const DISK_MAX_AGE: Duration = Duration::from_secs(5);

/// Re-ask the daemon for its shells at least this often.
const SHELLS_INTERVAL: Duration = Duration::from_secs(2);

/// What the next refresh should re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refresh {
    /// Re-read the disk panels (runs, budget, activity).
    pub disk: bool,
    /// Ask the daemon for its shells.
    pub shells: bool,
}

/// Decide what one tick re-reads. Pure, so tests drive it with invented
/// elapsed times.
pub fn plan_refresh(
    last: Option<TopFingerprint>,
    now: TopFingerprint,
    since_disk: Duration,
    since_shells: Duration,
) -> Refresh {
    let disk = last.is_none() || last != Some(now) || since_disk >= DISK_MAX_AGE;
    let shells = since_shells >= SHELLS_INTERVAL;
    Refresh { disk, shells }
}

/// The index the follow mode selects: the first `Live` row in the already
/// sorted rows (running first, then newest). With no live row, 0.
pub fn follow_target(runs: &[TopRunRow]) -> usize {
    runs.iter()
        .position(|r| r.liveness == Liveness::Live)
        .unwrap_or(0)
}

/// Everything the screen shows, plus where the user is in it.
pub struct TopModel {
    /// The snapshot the screen draws.
    pub snapshot: crate::cli::commands::top::TopSnapshot,
    /// The selected row's index into `snapshot.runs`.
    pub selected: usize,
    /// Keep the selection on the newest live run.
    pub follow: bool,
    /// The Activity filter text.
    pub filter: String,
    /// Whether the filter is being edited.
    pub editing: bool,
    /// The loop saw a quit key.
    pub quit: bool,
}

impl TopModel {
    pub fn new(snapshot: crate::cli::commands::top::TopSnapshot) -> Self {
        let selected = follow_target(&snapshot.runs);
        Self {
            snapshot,
            selected,
            follow: true,
            filter: String::new(),
            editing: false,
            quit: false,
        }
    }

    /// Apply one key. Ctrl-C quits in every mode, because raw mode swallows
    /// SIGINT. Only `KeyEventKind::Press` counts — release and repeat events
    /// would double every action.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.editing {
            match key.code {
                KeyCode::Char(c) => self.filter.push(c),
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Enter => self.editing = false,
                KeyCode::Esc => {
                    self.filter.clear();
                    self.editing = false;
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                self.follow = false;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.snapshot.runs.len().saturating_sub(1);
                self.selected = (self.selected + 1).min(max);
                self.follow = false;
            }
            KeyCode::Char('f') => self.follow = true,
            KeyCode::Char('/') => self.editing = true,
            _ => {}
        }
        if self.follow {
            self.selected = follow_target(&self.snapshot.runs);
        }
    }

    /// The Activity rows the filter admits, in `filter_activity`'s order:
    /// log order, oldest first. The Activity panel draws them tail-first,
    /// so the newest row that fits is the one on screen.
    pub fn filtered_activity(&self) -> Vec<ActivityRow> {
        collapse_activity(filter_activity(
            self.snapshot.activity.clone(),
            &self.filter,
        ))
    }
}
/// Draw one frame: the header line, four bordered blocks over the panel
/// texts laid out by [`plan_layout`], the selected run's line reversed, and
/// a footer naming the keys and the mode on the last row. An overflowing
/// body shows the lines that matter: Activity its newest rows, Runs a window
/// that keeps the selected row (and the column header) on screen.
pub fn draw(frame: &mut ratatui::Frame<'_>, model: &TopModel) {
    let area = frame.area();
    let runs_body = runs_text(&model.snapshot.runs);
    let budget_body = budget_text(&model.snapshot.budget);
    let shells_body = shells_text(&model.snapshot.shells);
    let activity_body = activity_text(&model.filtered_activity());
    let rects = plan_layout(
        area,
        runs_body.lines().count(),
        budget_body.lines().count() + 1,
        shells_body.lines().count(),
    );

    let host = crate::daemon::utils::daemon_hostname();
    let header = header_text(
        model.snapshot.daemon.as_ref(),
        &host,
        model.snapshot.taken_at.with_timezone(Local::now().offset()),
    );
    Paragraph::new(Line::raw(header)).render(rects.header, frame.buffer_mut());

    let faded_runs = model
        .snapshot
        .runs
        .iter()
        .enumerate()
        .filter(|(_, r)| run_fades(r))
        .map(|(i, _)| i)
        .collect::<Vec<usize>>();
    let (runs_display, runs_sel) =
        runs_window(&runs_body, model.selected, inner_height(rects.runs));
    let display_fades = faded_runs
        .iter()
        .filter_map(|i| {
            runs_display
                .lines()
                .position(|l| l == runs_body.lines().nth(*i).unwrap_or(""))
        })
        .collect::<Vec<usize>>();
    draw_panel(
        frame,
        rects.runs,
        "Runs",
        &runs_display,
        runs_sel,
        &display_fades,
    );

    let bar = budget_bar(
        model.snapshot.budget.ai_cost,
        model.snapshot.budget.cap,
        inner_width(rects.budget),
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
        .title(Line::styled(
            " Budget ".to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rects.budget);
    block.render(rects.budget, frame.buffer_mut());
    Paragraph::new(budget_lines).render(inner, frame.buffer_mut());

    draw_panel(frame, rects.shells, "Shells", &shells_body, None, &[]);

    let activity_lines = activity_body.lines().count();
    // The new Activity rows land at the bottom of the body, so a body taller
    // than its panel shows its tail, not its head.
    let activity_shown = activity_body
        .lines()
        .skip(activity_lines.saturating_sub(inner_height(rects.activity)))
        .collect::<Vec<_>>()
        .join("\n");
    draw_panel(
        frame,
        rects.activity,
        "Activity",
        &activity_shown,
        None,
        &[],
    );
    let mode = if model.follow {
        "following"
    } else {
        "scrolled"
    };
    let mut footer = format!(
        "q quit \u{b7} \u{2191}\u{2193} scroll \u{b7} f follow \u{b7} / filter \u{b7} {mode}"
    );
    if !model.filter.is_empty() {
        footer.push_str(&format!(" \u{b7} filter: {}", model.filter));
    }
    Paragraph::new(Line::raw(footer)).render(rects.footer, frame.buffer_mut());
}

/// A bordered panel's inner height: the borders take one line at each edge.
fn inner_height(area: ratatui::layout::Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

/// A bordered panel's inner width: the borders take one column at each edge.
fn inner_width(area: ratatui::layout::Rect) -> usize {
    area.width.saturating_sub(2) as usize
}

/// The Runs panel's body as drawn: when the rows overflow the panel, the
/// window keeps the column header on top and the selected run on screen.
/// The returned index is the selected line's position in the returned body,
/// The returned index is the selected line's position in the returned body,
/// `None` when the panel has no runs.
fn runs_window(body: &str, selected: usize, inner_h: usize) -> (String, Option<usize>) {
    let line_count = body.lines().count();
    if line_count == 0 {
        return (body.to_string(), None);
    }
    if line_count <= inner_h {
        let sel = (line_count > 1).then(|| selected + 1);
        return (body.to_string(), sel);
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
    (display, Some(1 + row - start))
}

/// One bordered panel with its title and a plain-text body. `selected`, when
/// given, is the line index inside `body` drawn reversed. `fades` is the set
/// of line indices drawn dimmed.
fn draw_panel(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    title: &str,
    body: &str,
    selected: Option<usize>,
    fades: &[usize],
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Line::styled(
            format!(" {title} "),
            Style::default().add_modifier(Modifier::BOLD),
        ));
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

/// The live loop. Enters raw mode, the alternate screen and a hidden cursor;
/// the `AltScreenGuard` teardown leaves all three, so `?` cannot strand the
/// terminal. A thread reads keys into a channel; the tokio loop waits on it
/// and a 500 ms tick, re-reading only what [`plan_refresh`] asked for, and
/// redraws after every key and every tick.
/// and a 500 ms tick, re-reading only what [`plan_refresh`] asked for, and
/// redraws after every key and every tick.
pub async fn run_tui(config_cap: f64, filter: String) -> Result<()> {
    enable_raw_mode()?;
    let teardown = || {
        let _ = crossterm::execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
    };
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    let _guard = AltScreenGuard::new(teardown);

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let snapshot = crate::cli::commands::top::take_snapshot(config_cap).await;
    let mut model = TopModel::new(snapshot);
    model.filter = filter;

    let running = Arc::new(AtomicBool::new(true));
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let reader_running = Arc::clone(&running);
    let reader = std::thread::spawn(move || {
        while reader_running.load(Ordering::Relaxed) {
            if crossterm::event::poll(TICK).unwrap_or(false)
                && let Ok(event) = crossterm::event::read()
                && event_tx.send(event).is_err()
            {
                break;
            }
        }
    });

    let mut last_fp = None;
    let mut since_disk = Duration::ZERO;
    let mut since_shells = Duration::ZERO;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result = loop {
        tokio::select! {
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(Event::Key(key)) => model.handle_key(key),
                    Some(_) => {}
                    None => break Ok(()),
                }
                if model.quit {
                    break Ok(());
                }
                terminal.draw(|f| draw(f, &model))?;
            }
            _ = tick.tick() => {
                since_disk += TICK;
                since_shells += TICK;
                let now_fp = fingerprint();
                let plan = plan_refresh(last_fp, now_fp, since_disk, since_shells);
                if plan.disk {
                    let taken_at = model.snapshot.taken_at;
                    let disk = crate::cli::commands::top::disk_panels(config_cap);
                    model.snapshot.runs = disk.runs;
                    model.snapshot.budget = disk.budget;
                    model.snapshot.activity = disk.activity;
                    model.snapshot.taken_at = taken_at;
                    if model.follow {
                        model.selected = follow_target(&model.snapshot.runs);
                    }
                    since_disk = Duration::ZERO;
                    last_fp = Some(now_fp);
                }
                if plan.shells {
                    model.snapshot.shells = crate::cli::commands::top::shells_panel().await;
                    since_shells = Duration::ZERO;
                }
                terminal.draw(|f| draw(f, &model))?;
            }
        }
    };

    running.store(false, Ordering::Relaxed);
    let _ = reader.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::top::TopRunRow;
    use chrono::{DateTime, Utc};

    fn row(id: &str, l: Liveness) -> TopRunRow {
        TopRunRow {
            id: id.to_string(),
            kind: "ghost".to_string(),
            subject: "disk-full-l4 ← webhook".to_string(),
            state: if l == Liveness::Done {
                "complete"
            } else {
                "running"
            }
            .to_string(),
            turns: 2,
            age_s: 10,
            liveness: l,
        }
    }

    fn fp(len: u64) -> TopFingerprint {
        TopFingerprint {
            runs_entries: 1,
            runs_newest_mtime_s: 2,
            segment_len: len,
            segment_mtime_s: 3,
        }
    }

    // ── plan_refresh ────────────────────────────────────────────────────────

    #[test]
    fn plan_refresh_rereads_disk_only_on_change_or_age() {
        let f = fp(10);
        // No last fingerprint: the first tick reads.
        assert!(plan_refresh(None, f, Duration::from_secs(0), Duration::from_secs(0)).disk);
        // Same fingerprint at 1 s: nothing moved on disk, skip the read.
        assert!(!plan_refresh(Some(f), f, Duration::from_secs(1), Duration::from_secs(1)).disk);
        // Changed fingerprint at 1 s: read.
        assert!(
            plan_refresh(
                Some(f),
                fp(11),
                Duration::from_secs(1),
                Duration::from_secs(1)
            )
            .disk
        );
        // Same fingerprint at 5 s: ages and the liveness window moved, read.
        assert!(plan_refresh(Some(f), f, DISK_MAX_AGE, Duration::from_secs(5)).disk);
    }

    #[test]
    fn plan_refresh_asks_for_shells_every_two_seconds() {
        let f = fp(10);
        assert!(
            !plan_refresh(Some(f), f, Duration::ZERO, Duration::from_secs(1)).shells,
            "shells are not re-read before 2 s"
        );
        assert!(
            plan_refresh(Some(f), f, Duration::ZERO, Duration::from_secs(2)).shells,
            "shells are re-read at 2 s"
        );
    }

    // ── follow ──────────────────────────────────────────────────────────────

    #[test]
    fn follow_selects_the_newest_live_run() {
        let rows = vec![
            row("a", Liveness::Done),
            row("b", Liveness::Live),
            row("c", Liveness::Live),
        ];
        assert_eq!(follow_target(&rows), 1, "the first live row wins");
        let none = vec![row("a", Liveness::Done)];
        assert_eq!(follow_target(&none), 0, "no live row falls back to 0");
    }

    #[test]
    fn scrolling_stops_follow_and_f_resumes_it() {
        let mut model = TopModel::new(snapshot(vec![
            row("a", Liveness::Live),
            row("b", Liveness::Done),
        ]));
        assert!(model.follow, "starts following");
        assert_eq!(model.selected, 0);
        press(&mut model, KeyCode::Down);
        assert!(!model.follow, "scrolling stops follow");
        assert_eq!(model.selected, 1);
        press(&mut model, KeyCode::Down);
        assert_eq!(model.selected, 1, "clamped at the last row");
        press(&mut model, KeyCode::Up);
        assert_eq!(model.selected, 0);
        press(&mut model, KeyCode::Up);
        assert_eq!(model.selected, 0, "clamped at the first row");
        press(&mut model, KeyCode::Char('f'));
        assert!(model.follow, "f resumes follow");
        assert_eq!(model.selected, 0, "follow snaps back to the live row");
    }

    fn press(model: &mut TopModel, code: KeyCode) {
        model.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    // ── filter editing ──────────────────────────────────────────────────────

    #[test]
    fn filter_keys_edit_the_activity_filter() {
        let mut model = TopModel::new(snapshot(vec![]));
        press(&mut model, KeyCode::Char('/'));
        assert!(model.editing, "/ starts editing");
        for code in [KeyCode::Char('e'), KeyCode::Char('q'), KeyCode::Char('x')] {
            press(&mut model, code);
        }
        press(&mut model, KeyCode::Backspace);
        press(&mut model, KeyCode::Enter);
        assert_eq!(model.filter, "eq", "the q typed while editing is kept");
        assert!(!model.editing, "Enter stops editing");
        assert!(!model.quit, "a q typed into the filter never quits");
        press(&mut model, KeyCode::Char('/'));
        press(&mut model, KeyCode::Esc);
        assert_eq!(model.filter, "", "Esc clears the text");
        assert!(!model.editing, "Esc stops editing");
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut model = TopModel::new(snapshot(vec![]));
        press(&mut model, KeyCode::Char('q'));
        assert!(model.quit, "q quits when not editing");
        let mut model = TopModel::new(snapshot(vec![]));
        press(&mut model, KeyCode::Char('/'));
        model.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(model.quit, "Ctrl-C quits even while editing");
        let mut model = TopModel::new(snapshot(vec![]));
        model.handle_key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert!(!model.quit, "a release event is ignored");
    }

    // ── the screen ──────────────────────────────────────────────────────────

    fn snapshot(runs: Vec<TopRunRow>) -> crate::cli::commands::top::TopSnapshot {
        snapshot_with(runs, vec![])
    }

    fn snapshot_with(
        runs: Vec<TopRunRow>,
        activity: Vec<crate::cli::commands::top::ActivityRow>,
    ) -> crate::cli::commands::top::TopSnapshot {
        let taken_at: DateTime<Utc> = "2026-10-22T00:00:00Z".parse().expect("a fixed time");
        crate::cli::commands::top::TopSnapshot {
            taken_at,
            runs,
            budget: crate::cli::commands::top::BudgetPanel {
                ai_cost: 0.5,
                cap: 0.0,
                tokens: 100,
                refused: 0,
                tok_per_s: 0.0,
            },
            activity,
            shells: crate::cli::commands::top::ShellsPanel::DaemonDown,
            daemon: None,
        }
    }

    fn activity_row(
        time: &str,
        name: &str,
        summary: &str,
    ) -> crate::cli::commands::top::ActivityRow {
        crate::cli::commands::top::ActivityRow {
            time: time.to_string(),
            name: name.to_string(),
            summary: summary.to_string(),
        }
    }

    #[test]
    fn filtered_activity_collapses_repeats() {
        let activity: Vec<crate::cli::commands::top::ActivityRow> = (0..5)
            .map(|i| activity_row(&format!("09:00:0{i}"), "cache_refresh_error", "pane gone"))
            .collect();
        let model = TopModel::new(snapshot_with(vec![], activity));
        let out = model.filtered_activity();
        assert_eq!(
            out.len(),
            1,
            "five identical consecutive rows collapse to one: {out:?}"
        );
        assert_eq!(
            out.first().map(|r| r.summary.as_str()),
            Some("pane gone ×5"),
            "the single row is marked ×5 and keeps the last time: {out:?}"
        );
        assert_eq!(
            out.first().map(|r| r.time.as_str()),
            Some("09:00:04"),
            "the merged row keeps the last time of its run"
        );
    }

    #[test]
    fn screen_draws_the_four_panels() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let model = TopModel::new(snapshot(vec![]));
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        for title in ["Runs", "Budget", "Shells", "Activity"] {
            assert!(
                text.contains(title),
                "the {title} title is on screen: {text:?}"
            );
        }
        assert!(
            text.contains("daemon not running"),
            "the daemon-down mark is in the Shells panel: {text:?}"
        );
        assert!(
            text.contains("following"),
            "the footer names the mode: {text:?}"
        );
    }

    #[test]
    fn screen_draws_the_header_line() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut snap = snapshot(vec![]);
        snap.daemon = Some(crate::cli::commands::top::DaemonHeader {
            pid: 4242,
            uptime_secs: 95,
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
        });
        let model = TopModel::new(snap);
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(text.contains("pid 4242"), "row 0 names the pid: {text:?}");
        assert!(
            text.contains("up 1m 35s"),
            "row 0 names the uptime: {text:?}"
        );
        let mut snap = snapshot(vec![]);
        snap.daemon = None;
        let model = TopModel::new(snap);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            text.contains("daemon not running"),
            "a down daemon names itself in the header: {text:?}"
        );
    }

    #[test]
    fn screen_colours_the_budget_bar() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut snap = snapshot(vec![]);
        snap.budget.ai_cost = 8.5;
        snap.budget.cap = 10.0;
        let model = TopModel::new(snap);
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        let (_, level) = budget_bar(8.5, 10.0, 20).expect("a bar at 85%");
        assert_eq!(level, BarLevel::Warn, "85% is a warn");
        let filled = text.matches('\u{2588}').count();
        let empty = text.matches('\u{2591}').count();
        assert!(
            filled > 0 && empty > 0,
            "the bar is on screen with both cells: filled {filled} empty {empty}"
        );
        let yellow = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|c| c.fg == ratatui::style::Color::Yellow);
        assert!(yellow, "the bar draws yellow: {text:?}");
    }

    #[test]
    fn activity_panel_shows_the_newest_rows() {
        let activity: Vec<_> = (1..=40)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let model = TopModel::new(snapshot_with(vec![], activity));
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            text.contains("model40"),
            "the newest row is on screen: {text:?}"
        );
        assert!(
            !text.contains("model01"),
            "the oldest row scrolled off: {text:?}"
        );
    }

    #[test]
    fn selected_run_stays_on_screen() {
        let runs: Vec<_> = (1..=20)
            .rev()
            .map(|i| row(&format!("r{i:02}"), Liveness::Done))
            .collect();
        let mut model = TopModel::new(snapshot(runs));
        model.follow = false;
        model.selected = 19;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            text.contains("r01"),
            "the selected id is on screen: {text:?}"
        );
        assert!(
            !text.contains("r20"),
            "the window scrolled past the newest row: {text:?}"
        );
        assert!(
            text.contains("ID"),
            "the header line stays visible: {text:?}"
        );
    }
}
