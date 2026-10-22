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
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};

use crate::cli::commands::top::{
    ActivityRow, Liveness, TopFingerprint, TopRunRow, activity_text, budget_text, filter_activity,
    fingerprint, runs_text,
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

    /// The Activity rows the filter admits, newest first.
    pub fn filtered_activity(&self) -> Vec<ActivityRow> {
        filter_activity(self.snapshot.activity.clone(), &self.filter)
    }
}

/// Draw one frame: four bordered blocks over the panel texts, the selected
/// run's line reversed, and a footer naming the keys and the mode.
pub fn draw(frame: &mut ratatui::Frame<'_>, model: &TopModel) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_panel(
        frame,
        chunks[0],
        "Runs",
        &runs_text(&model.snapshot.runs),
        Some(model.selected),
    );
    draw_panel(
        frame,
        chunks[1],
        "Budget",
        &budget_text(&model.snapshot.budget),
        None,
    );
    let shells = match &model.snapshot.shells {
        crate::cli::commands::top::ShellsPanel::DaemonDown => "daemon not running".to_string(),
        crate::cli::commands::top::ShellsPanel::Rows(rows) if rows.is_empty() => "none".to_string(),
        crate::cli::commands::top::ShellsPanel::Rows(rows) => rows
            .iter()
            .map(|s| {
                format!(
                    "{} {} {} {}",
                    s.id,
                    s.owner,
                    s.foreground.as_deref().unwrap_or("-"),
                    s.last_line.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    draw_panel(frame, chunks[2], "Shells", &shells, None);
    let activity = model.filtered_activity();
    draw_panel(
        frame,
        chunks[3],
        "Activity",
        &activity_text(&activity),
        None,
    );

    let mode = if model.follow {
        "following"
    } else {
        "scrolled"
    };
    let mut footer = format!("q quit · ↑↓ scroll · f follow · / filter · {mode}");
    if !model.filter.is_empty() {
        footer.push_str(&format!(" · filter: {}", model.filter));
    }
    Paragraph::new(Line::raw(footer)).render(chunks[4], frame.buffer_mut());
}

/// One bordered panel with its title and a plain-text body. `selected`, when
/// given, is the row index whose line is drawn reversed.
fn draw_panel(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    title: &str,
    body: &str,
    selected: Option<usize>,
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
            activity: vec![],
            shells: crate::cli::commands::top::ShellsPanel::DaemonDown,
        }
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
}
