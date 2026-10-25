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
use chrono::{DateTime, FixedOffset, Local};
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
    ActivityRow, BarLevel, Liveness, PanelRects, TopFingerprint, TopRunRow, activity_text,
    budget_bar, budget_text, collapse_activity, filter_activity, fingerprint, header_text,
    plan_layout, rect_or_area, run_fades, runs_text, shells_text,
};
use crate::cli::viewer::AltScreenGuard;
/// Poll the disk fingerprint every this long.
const TICK: Duration = Duration::from_millis(500);

/// A budget bar's chrome: the two brackets and the ` NNN%` tail the panel
/// draws beside the cells — three digits, because the percentage is printed
/// as it is and runs over the cap.
const BAR_CHROME: usize = 7;

/// Replace the model's Shells panel and daemon header with fresh answers
/// from the daemon: one read-only `Request::ShellList` and one
/// `Request::Status`, the header's facts on the same 2 s cadence as the
/// shells. A test drives it without a daemon behind the [`ShellFetcher`]
/// seam.
pub(crate) async fn shells_refresh(model: &mut TopModel, fetch: &impl ShellFetcher) {
    model.snapshot.shells = fetch.shells().await;
    model.snapshot.daemon = fetch.daemon().await;
}

/// The daemon answers the live loop asks for: its shells and the header's
/// facts. The production impl calls the real IPC client; tests install a
/// fake, so the loop's tests stay hermetic.
pub(crate) trait ShellFetcher {
    fn shells(&self) -> impl Future<Output = crate::cli::commands::top::ShellsPanel>;
    fn daemon(&self) -> impl Future<Output = Option<crate::cli::commands::top::DaemonHeader>>;
}

/// The production fetcher: real read-only IPC, one round-trip per call.
struct IpcFetcher;

impl ShellFetcher for IpcFetcher {
    async fn shells(&self) -> crate::cli::commands::top::ShellsPanel {
        crate::cli::commands::top::shells_panel().await
    }

    async fn daemon(&self) -> Option<crate::cli::commands::top::DaemonHeader> {
        crate::cli::commands::top::daemon_header().await
    }
}

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
    /// The panel the scroll keys act on.
    pub focus: Panel,
    /// The index of the Shells body line drawn first; clamped to the
    /// content and the panel's inner height.
    pub shells_top: usize,
    /// The index of the Activity body line drawn first while not tailing.
    pub activity_top: usize,
    /// The Activity panel shows its newest rows until the user scrolls it.
    pub tailing: bool,
    /// The first visible Activity row while not tailing, so a content
    /// refresh can put the window back on it; `None` while tailing.
    pub activity_anchor: Option<ActivityRow>,
    /// The inner heights the current layout gives Runs, Shells and
    /// Activity; the loop refreshes them so key handling clamps against
    /// the real panels.
    pub heights: PanelHeights,
    /// The Activity filter text.
    pub filter: String,
    /// Whether the filter is being edited.
    pub editing: bool,
    /// The loop saw a quit key.
    pub quit: bool,
    /// The run whose detail is open, when one is.
    pub detail: Option<OpenDetail>,
    /// The detail view's scroll: the index of the detail text's first
    /// visible line. Clamped through [`Self::clamp_offsets`].
    pub detail_top: usize,
}

/// The open detail view: which run it shows and the text it draws — the
/// heading, the record or running state, the end state and the log tail, or
/// the vanished line when the run's turn log is no longer on disk. The text
/// is rendered once per load, so scroll clamping and drawing read the same
/// lines.
pub struct OpenDetail {
    /// The open run's id.
    pub id: crate::daemon::run::RunId,
    /// The text the detail panel draws, line-broken.
    pub text: String,
}

/// The inner heights of the scrollable panels, from `plan_layout` for the
/// current terminal size. Zero until the loop measures them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelHeights {
    /// The Runs panel's inner height.
    pub runs: usize,
    /// The Shells panel's inner height.
    pub shells: usize,
    /// The Activity panel's inner height.
    pub activity: usize,
    /// The detail view's panel inner height.
    pub detail: usize,
}

impl TopModel {
    pub fn new(snapshot: crate::cli::commands::top::TopSnapshot) -> Self {
        let selected = follow_target(&snapshot.runs);
        Self {
            snapshot,
            selected,
            follow: true,
            focus: Panel::Runs,
            shells_top: 0,
            activity_top: 0,
            tailing: true,
            activity_anchor: None,
            heights: PanelHeights::default(),
            filter: String::new(),
            editing: false,
            quit: false,
            detail: None,
            detail_top: 0,
        }
    }

    /// Open the detail view on one run: load its record and log from disk,
    /// render the detail text, and start scrolled to the top. The loop and
    /// the `--run` argument both open through this, so they cannot drift.
    pub fn open_detail(&mut self, id: crate::daemon::run::RunId) {
        self.detail = Some(self.load_detail(id));
        self.detail_top = 0;
    }

    /// Re-read an open detail from disk: the fresh row, record and log, the
    /// text re-rendered. The scroll offset is left for
    /// [`Self::clamp_offsets`] to clamp. A run whose turn log has gone off
    /// disk draws the vanished line instead of panicking.
    pub fn reload_detail(&mut self) {
        let Some(d) = &self.detail else { return };
        let id = d.id.clone();
        self.detail = Some(self.load_detail(id));
    }

    /// Load one run's detail text from disk. Private: opening and reloading
    /// differ only in whether the scroll resets.
    fn load_detail(&self, id: crate::daemon::run::RunId) -> OpenDetail {
        let loaded = crate::cli::commands::top::load_run_detail(&id);
        let row = loaded
            .as_ref()
            .and_then(|d| crate::cli::commands::top::detail_row(&id, &d.log, chrono::Utc::now()));
        let text = match (row, loaded) {
            (Some(row), Some(loaded)) => crate::cli::commands::top::run_detail_text(
                &row,
                loaded.record.as_ref(),
                &loaded.log,
            ),
            _ => format!("run {id} is no longer on disk"),
        };
        OpenDetail { id, text }
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
            self.clamp_offsets();
            return;
        }
        if self.detail.is_some() {
            match key.code {
                KeyCode::Esc => self.detail = None,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.detail_top = self.detail_top.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.detail_top = self.detail_top.saturating_add(1)
                }
                KeyCode::PageUp => self.detail_top = self.detail_top.saturating_sub(self.page()),
                KeyCode::PageDown => self.detail_top = self.detail_top.saturating_add(self.page()),
                KeyCode::Home => self.detail_top = 0,
                KeyCode::End => {
                    let lines = self.detail_lines();
                    self.detail_top = self.last_window_top(lines, self.heights.detail);
                }
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            }
            self.clamp_offsets();
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.previous(),
            KeyCode::Char(c @ '1'..='4') => self.focus = focus_of(c),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_down(1),
            KeyCode::PageUp => self.scroll_up(self.page()),
            KeyCode::PageDown => self.scroll_down(self.page()),
            KeyCode::Home => self.scroll_home(),
            KeyCode::End => self.scroll_end(),
            KeyCode::Enter if self.focus == Panel::Runs => {
                if let Some(row) = self.snapshot.runs.get(self.selected)
                    && let Ok(id) = crate::daemon::run::RunId::parse(&row.id)
                {
                    self.open_detail(id);
                }
            }
            KeyCode::Char('f') => {
                self.follow = true;
                self.tailing = true;
                self.clamp_offsets();
            }
            KeyCode::Char('/') => self.editing = true,
            _ => {}
        }
        if self.follow {
            self.selected = follow_target(&self.snapshot.runs);
        }
        self.clamp_offsets();
    }

    /// A page of the focused panel: its inner height minus one, at least 1.
    fn page(&self) -> usize {
        if self.detail.is_some() {
            return self.heights.detail.saturating_sub(1).max(1);
        }
        let h = match self.focus {
            Panel::Runs => self.heights.runs,
            Panel::Budget => 0,
            Panel::Shells => self.heights.shells,
            Panel::Activity => self.heights.activity,
        };
        h.saturating_sub(1).max(1)
    }

    /// One line up on the focused panel. Runs moves the selection and stops
    /// follow; Shells and Activity move their top offset and stop Activity
    /// tailing; Budget ignores the key.
    fn scroll_up(&mut self, n: usize) {
        match self.focus {
            Panel::Runs => {
                self.selected = self.selected.saturating_sub(n);
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => self.shells_top = self.shells_top.saturating_sub(n),
            Panel::Activity => {
                self.activity_top = self.activity_top.saturating_sub(n);
                self.tailing = false;
            }
        }
    }

    /// One line down on the focused panel; see [`TopModel::scroll_up`].
    fn scroll_down(&mut self, n: usize) {
        match self.focus {
            Panel::Runs => {
                let max = self.snapshot.runs.len().saturating_sub(1);
                self.selected = (self.selected + n).min(max);
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => {
                self.shells_top = (self.shells_top + n).min(
                    self.last_window_top(shells_lines(&self.snapshot.shells), self.heights.shells),
                );
            }
            Panel::Activity => {
                self.activity_top += n;
                self.tailing = false;
            }
        }
    }

    /// The top of the focused panel. Runs selects the first run; Shells and
    /// Activity go to their first body line; Budget ignores the key.
    fn scroll_home(&mut self) {
        match self.focus {
            Panel::Runs => {
                self.selected = 0;
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => self.shells_top = 0,
            Panel::Activity => {
                self.activity_top = 0;
                self.tailing = false;
            }
        }
    }

    /// The bottom of the focused panel. Runs selects the last run; Shells
    /// and Activity scroll to their last full window; Budget ignores the
    /// key. Scrolling Activity to its bottom resumes tailing.
    fn scroll_end(&mut self) {
        match self.focus {
            Panel::Runs => {
                self.selected = self.snapshot.runs.len().saturating_sub(1);
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => {
                let total = shells_lines(&self.snapshot.shells);
                self.shells_top = self.last_window_top(total, self.heights.shells);
            }
            Panel::Activity => {
                self.tailing = true;
            }
        }
    }

    /// The first top offset that shows the last full window of `total`
    /// lines in `inner_h` rows: `total - inner_h` when the content
    /// overflows, else 0.
    fn last_window_top(&self, total: usize, inner_h: usize) -> usize {
        total.saturating_sub(inner_h)
    }

    /// Pull every offset back inside the content and the stored heights,
    /// after a key, a snapshot refresh or a filter change. Activity that
    /// has scrolled to its bottom resumes tailing.
    pub fn clamp_offsets(&mut self) {
        let rows = self.filtered_activity();
        let activity_lines = activity_text(&rows).lines().count();
        let bottom = self.last_window_top(activity_lines, self.heights.activity);
        if self.tailing {
            self.activity_top = bottom;
            self.activity_anchor = None;
        } else {
            self.activity_top = self.activity_top.min(bottom);
            // The newest row is visible again: tail from here.
            if self.activity_top == bottom {
                self.tailing = true;
                self.activity_anchor = None;
            } else {
                // The row the window shows first, so a refresh that slides
                // the list can put the window back on it.
                self.activity_anchor = rows.get(self.activity_top).cloned();
            }
        }
        let shells = shells_lines(&self.snapshot.shells);
        self.shells_top = self
            .shells_top
            .min(self.last_window_top(shells, self.heights.shells));
        if let Some(detail) = &self.detail {
            let lines = detail.text.lines().count();
            self.detail_top = self
                .detail_top
                .min(self.last_window_top(lines, self.heights.detail));
        }
    }

    /// The detail text's line count — the same lines the detail panel
    /// windows. Zero when no detail is open.
    fn detail_lines(&self) -> usize {
        self.detail
            .as_ref()
            .map(|d| d.text.lines().count())
            .unwrap_or(0)
    }

    /// Put the not-tailing Activity window back on the row it showed: the
    /// panel holds the day's last events, so a refresh drops the oldest
    /// rows and every index shifts under a fixed top. While tailing the
    /// window follows the bottom and this does nothing.
    pub fn hold_activity(&mut self) {
        if self.tailing {
            return;
        }
        let Some(anchor) = self.activity_anchor.clone() else {
            return;
        };
        let rows = self.filtered_activity();
        if let Some(idx) = rows.iter().position(|r| *r == anchor) {
            self.activity_top = idx;
        }
    }

    /// Everything the loop applies after a refresh, in one place: hold the
    /// Activity anchor, re-measure the stored heights against the fresh
    /// content — the panels are sized to their content, so a refresh
    /// resizes them without a terminal resize — and clamp the offsets.
    /// The loop calls this after every disk and shells refresh and on
    /// resize; it never clamps on its own.
    pub fn after_refresh(&mut self, size: Option<ratatui::layout::Rect>) {
        self.hold_activity();
        refresh_heights(self, size);
        self.clamp_offsets();
    }
}

/// The panel the keys act on, in the order `1` to `4` and the Tab cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// The run list.
    Runs,
    /// Today's spend.
    Budget,
    /// The daemon's live shells.
    Shells,
    /// The day's events.
    Activity,
}

impl Panel {
    /// The next panel in the cycle, wrapping from Activity to Runs.
    pub fn next(self) -> Self {
        match self {
            Panel::Runs => Panel::Budget,
            Panel::Budget => Panel::Shells,
            Panel::Shells => Panel::Activity,
            Panel::Activity => Panel::Runs,
        }
    }

    /// The previous panel in the cycle, wrapping from Runs to Activity.
    pub fn previous(self) -> Self {
        match self {
            Panel::Runs => Panel::Activity,
            Panel::Budget => Panel::Runs,
            Panel::Shells => Panel::Budget,
            Panel::Activity => Panel::Shells,
        }
    }

    /// The word the footer names: `Runs`, `Budget`, `Shells`, `Activity`.
    pub fn label(self) -> &'static str {
        match self {
            Panel::Runs => "Runs",
            Panel::Budget => "Budget",
            Panel::Shells => "Shells",
            Panel::Activity => "Activity",
        }
    }

    /// The panel's position in the Tab cycle, `0` to `3`.
    pub fn index(self) -> usize {
        match self {
            Panel::Runs => 0,
            Panel::Budget => 1,
            Panel::Shells => 2,
            Panel::Activity => 3,
        }
    }
}

/// The panel a digit key jumps to; `1` to `4`.
fn focus_of(c: char) -> Panel {
    match c {
        '1' => Panel::Runs,
        '2' => Panel::Budget,
        '3' => Panel::Shells,
        _ => Panel::Activity,
    }
}

/// The rects and the four panel inner heights the model's current content
/// and terminal give. The one call site of the layout planner in this file,
/// so the drawn panels and the stored heights cannot disagree again.
fn layout_at(model: &TopModel, area: ratatui::layout::Rect) -> (PanelRects, PanelHeights) {
    let rects = plan_layout(
        area,
        runs_lines(&model.snapshot.runs),
        budget_lines(&model.snapshot.budget),
        shells_lines(&model.snapshot.shells),
    );
    let heights = PanelHeights {
        runs: inner_height(rects.runs),
        shells: inner_height(rects.shells),
        activity: inner_height(rects.activity),
        detail: inner_height(detail_rect(&rects, area)),
    };
    (rects, heights)
}

/// The one panel the detail view draws: the full span between the header
/// and the footer, at the area's width. Derived from the laid-out rects, so
/// `plan_layout` stays the single layout call site.
fn detail_rect(rects: &PanelRects, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    rect_or_area(
        area,
        area.x,
        rects.header.bottom(),
        area.width as usize,
        rects.footer.y.saturating_sub(rects.header.bottom()) as usize,
    )
}

/// Re-measure the stored heights for `size` and the model's current content.
/// A size the backend cannot report leaves the stored heights alone. The
/// loop calls this on resize and after every content refresh, because the
/// panels are sized to their content.
fn refresh_heights(model: &mut TopModel, size: Option<ratatui::layout::Rect>) {
    let Some(size) = size else {
        return;
    };
    model.heights = layout_at(model, size).1;
}

/// The Runs panel's body line count: one per run plus the column header,
/// capped where `plan_layout` caps it. Pure.
fn runs_lines(runs: &[crate::cli::commands::top::TopRunRow]) -> usize {
    runs.len() + 1
}

/// The Budget panel's body line count: the four text lines plus the bar
/// when a cap is set. Pure.
fn budget_lines(budget: &crate::cli::commands::top::BudgetPanel) -> usize {
    budget_text(budget).lines().count() + usize::from(budget.cap > 0.0)
}

/// The Shells panel's body line count from its text. Pure.
fn shells_lines(panel: &crate::cli::commands::top::ShellsPanel) -> usize {
    shells_text(panel).lines().count()
}

impl TopModel {
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

    let shells_window = window_text(&shells_body, model.shells_top, inner_height(rects.shells));
    draw_panel(
        frame,
        rects.shells,
        &panel_title(Panel::Shells),
        &shells_window,
        None,
        &[],
        model.focus == Panel::Shells,
    );

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
        activity_window_text(
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
    if !model.filter.is_empty() {
        footer.push_str(&format!(" \u{b7} filter: {}", model.filter));
    }
    Paragraph::new(Line::raw(footer)).render(rects.footer, frame.buffer_mut());
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
fn window_text(body: &str, top: usize, inner_h: usize) -> String {
    let total = body.lines().count();
    visible_range(total, inner_h, top)
        .map(|i| body.lines().nth(i).unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The Activity body's visible window while not tailing, from the same
/// range helper.
fn activity_window_text(body: &str, top: usize, inner_h: usize) -> String {
    window_text(body, top, inner_h)
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

/// The live loop. Enters raw mode, the alternate screen and a hidden cursor;
/// the `AltScreenGuard` teardown leaves all three, so `?` cannot strand the
/// terminal. A thread reads keys into a channel; the tokio loop waits on it
/// and a 500 ms tick, re-reading only what [`plan_refresh`] asked for, and
/// redraws after every key and every tick.
pub(crate) async fn run_tui(
    config_cap: f64,
    filter: String,
    open_run: Option<crate::daemon::run::RunId>,
) -> Result<()> {
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
    // The snapshot's panels are the content the first frame sizes to, so the
    // key handling clamps against real heights from the first key.
    if let Ok(size) = terminal.size() {
        model.after_refresh(Some(ratatui::layout::Rect::new(
            0,
            0,
            size.width,
            size.height,
        )));
    }

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

    let mut last_size = None;
    if let Some(id) = open_run {
        model.open_detail(id);
        model.after_refresh(Some(ratatui::layout::Rect::new(
            0,
            0,
            terminal.size().map(|s| s.width).unwrap_or(80),
            terminal.size().map(|s| s.height).unwrap_or(24),
        )));
    }
    let result = loop {
        let size = terminal
            .size()
            .ok()
            .map(|s| ratatui::layout::Rect::new(0, 0, s.width, s.height));
        if size != last_size {
            last_size = size;
            model.after_refresh(size);
        }
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
                    model.reload_detail();
                    model.after_refresh(size);
                    since_disk = Duration::ZERO;
                    last_fp = Some(now_fp);
                }
                if plan.shells {
                    shells_refresh(&mut model, &IpcFetcher).await;
                    model.after_refresh(size);
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
    use crate::cli::commands::top::{DaemonHeader, ShellsPanel, TopRunRow};
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

    fn shell(id: &str) -> crate::ipc::ShellInfo {
        crate::ipc::ShellInfo {
            id: id.to_string(),
            host: "local".to_string(),
            owner: format!("chat:{id}{id}"),
            started_unix: 0,
            dedicated: false,
            exited_unix: None,
            last_command: None,
            foreground: None,
            last_line: None,
        }
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

    fn snapshot_with_shells(
        runs: Vec<TopRunRow>,
        shells: Vec<crate::ipc::ShellInfo>,
    ) -> crate::cli::commands::top::TopSnapshot {
        let mut snap = snapshot_with(runs, vec![]);
        snap.shells = crate::cli::commands::top::ShellsPanel::Rows(shells);
        snap
    }

    fn snapshot_with_all(
        runs: Vec<TopRunRow>,
        activity: Vec<crate::cli::commands::top::ActivityRow>,
        shells: Vec<crate::ipc::ShellInfo>,
    ) -> crate::cli::commands::top::TopSnapshot {
        let mut snap = snapshot_with(runs, activity);
        snap.shells = crate::cli::commands::top::ShellsPanel::Rows(shells);
        snap
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

    /// The drawn screen as rows, one `String` per row, so a title needle is
    /// matched within a single row and not across the whole screen.
    fn drawn_rows(terminal: &Terminal<ratatui::backend::TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| {
                        buffer
                            .cell((x, y))
                            .map(|c| c.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect::<String>()
            })
            .collect()
    }

    /// Assert a title needle sits on one row, and no row carries a doubled
    /// padding on either side of a title.
    fn assert_title_rows(rows: &[String], needles: &[&str]) {
        for needle in needles {
            assert!(
                rows.iter().any(|row| row.contains(needle)),
                "a row contains {needle:?}: {rows:?}"
            );
        }
        for row in rows {
            assert!(
                !row.contains("╭  ") && !row.contains("  ─"),
                "no doubled padding on a title row: {row:?}"
            );
        }
    }

    #[test]
    fn every_title_is_padded_once() {
        for (w, h) in [(80u16, 24u16), (160u16, 48u16)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            let model = TopModel::new(snapshot(vec![]));
            terminal.draw(|f| draw(f, &model)).unwrap();
            let rows = drawn_rows(&terminal);
            assert_title_rows(
                &rows,
                &[
                    "╭ 1 Runs ─",
                    "╭ 2 Budget ─",
                    "╭ 3 Shells ─",
                    "╭ 4 Activity ─",
                ],
            );
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
    fn screen_shows_the_budget_percentage() {
        for (w, h) in [(80u16, 24u16), (120u16, 40u16)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
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
            assert!(
                text.contains("85%"),
                "the drawn buffer holds 85% at {w}x{h}: {text:?}"
            );
        }
    }

    #[test]
    fn screen_shows_the_percentage_over_the_cap() {
        for (w, h) in [(80u16, 24u16), (120u16, 40u16)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            let mut snap = snapshot(vec![]);
            snap.budget.ai_cost = 15.0;
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
            assert!(
                text.contains("150%"),
                "the drawn buffer holds 150% at {w}x{h}: {text:?}"
            );
        }
    }

    #[test]
    fn screen_fades_with_more_runs_than_fit() {
        let runs: Vec<TopRunRow> = (0..20)
            .map(|i| {
                let mut r = row(&format!("r{i:02}"), Liveness::Done);
                if i % 2 == 1 {
                    r = row(&format!("r{i:02}"), Liveness::Live);
                    r.age_s = 10;
                } else {
                    r.age_s = 600;
                }
                r
            })
            .collect();
        let done_ids: Vec<String> = (0..20).step_by(2).map(|i| format!("r{i:02}")).collect();
        let live_ids: Vec<String> = (1..20).step_by(2).map(|i| format!("r{i:02}")).collect();
        for selected in [0usize, 19] {
            let mut model = TopModel::new(snapshot(runs.clone()));
            model.follow = false;
            model.selected = selected;
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| draw(f, &model)).expect("a draw");
            let rows: Vec<Vec<(char, ratatui::style::Modifier)>> = (0..24)
                .map(|y| {
                    (0..80)
                        .map(|x| {
                            let c = &terminal.backend().buffer().content[y * 80 + x];
                            (c.symbol().chars().next().unwrap_or(' '), c.modifier)
                        })
                        .collect()
                })
                .collect();
            let dimmed: Vec<String> = rows
                .iter()
                .filter(|r| {
                    r.iter()
                        .any(|(_, m)| m.contains(ratatui::style::Modifier::DIM))
                })
                .map(|r| r.iter().map(|(c, _)| c).collect())
                .collect();
            assert!(
                !dimmed.is_empty(),
                "a visible finished run dims, selection {selected}"
            );
            for line in &dimmed {
                assert!(
                    done_ids.iter().any(|id| line.contains(id)),
                    "a dimmed line is a finished run's: {line:?}"
                );
                assert!(
                    !line.contains("ID KIND"),
                    "the column header never dims: {line:?}"
                );
                assert!(
                    !live_ids.iter().any(|id| line.contains(id)),
                    "a live run never dims: {line:?}"
                );
            }
        }
    }

    #[test]
    fn screen_dims_only_finished_runs() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let live = row("aaaa", Liveness::Live);
        let mut done = row("dddd", Liveness::Done);
        done.age_s = 600;
        let model = TopModel::new(snapshot(vec![live, done]));
        terminal.draw(|f| draw(f, &model)).unwrap();
        let rows: Vec<Vec<(char, ratatui::style::Modifier)>> = (0..24)
            .map(|y| {
                (0..80)
                    .map(|x| {
                        let c = &terminal.backend().buffer().content[y * 80 + x];
                        (c.symbol().chars().next().unwrap_or(' '), c.modifier)
                    })
                    .collect()
            })
            .collect();
        let dimmed: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.iter()
                    .any(|(_, m)| m.contains(ratatui::style::Modifier::DIM))
            })
            .map(|(y, _)| y)
            .collect();
        assert!(!dimmed.is_empty(), "a finished run draws dim: {rows:?}");
        for y in &dimmed {
            let line: String = rows[*y].iter().map(|(c, _)| c).collect();
            assert!(
                line.contains("dddd") && !line.contains("ID KIND") && !line.contains("aaaa"),
                "only the finished run's line dims: {line:?}"
            );
        }
    }

    #[test]
    fn screen_header_clock_is_the_draw_time() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut snap = snapshot(vec![]);
        snap.taken_at = "2026-10-22T00:00:00Z".parse().expect("a fixed time");
        let model = TopModel::new(snap);
        let draw_time = "2026-10-22T09:00:00+00:00"
            .parse::<DateTime<FixedOffset>>()
            .expect("a fixed time");
        terminal
            .draw(|f| draw_at(f, &model, draw_time))
            .expect("a draw");
        let header: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .take(80)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            header.contains("09:00:00"),
            "the header names the draw time: {header:?}"
        );
        assert!(
            !header.contains("00:00:00"),
            "the header does not name taken_at's clock: {header:?}"
        );
    }

    #[test]
    fn shells_refresh_replaces_the_daemon_header() {
        let mut model = TopModel::new(snapshot(vec![]));
        assert!(model.snapshot.daemon.is_none(), "no daemon at first");
        let fetch = FakeFetcher {
            daemon: Some(DaemonHeader {
                pid: 4242,
                uptime_secs: 90,
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
            }),
        };
        tokio::runtime::Runtime::new()
            .expect("a runtime")
            .block_on(shells_refresh(&mut model, &fetch));
        let daemon = model.snapshot.daemon.expect("the header was replaced");
        assert_eq!(daemon.pid, 4242, "the refresh replaced the header");
        assert_eq!(daemon.model, "gpt-5", "the refresh replaced the header");
    }

    struct FakeFetcher {
        daemon: Option<DaemonHeader>,
    }

    impl ShellFetcher for FakeFetcher {
        async fn shells(&self) -> ShellsPanel {
            ShellsPanel::DaemonDown
        }

        async fn daemon(&self) -> Option<DaemonHeader> {
            self.daemon.clone()
        }
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

    // ── focus ────────────────────────────────────────────────────────────────

    #[test]
    fn focus_cycles_with_tab_and_wraps() {
        let mut model = TopModel::new(snapshot(vec![]));
        assert_eq!(model.focus, Panel::Runs, "the screen starts on Runs");
        for want in [Panel::Budget, Panel::Shells, Panel::Activity, Panel::Runs] {
            press(&mut model, KeyCode::Tab);
            assert_eq!(model.focus, want, "Tab moves to the next panel");
        }
        press(&mut model, KeyCode::BackTab);
        assert_eq!(
            model.focus,
            Panel::Activity,
            "BackTab from Runs wraps to Activity"
        );
        press(&mut model, KeyCode::BackTab);
        assert_eq!(model.focus, Panel::Shells, "BackTab moves back one panel");
    }

    #[test]
    fn number_keys_jump_focus() {
        let mut model = TopModel::new(snapshot(vec![]));
        press(&mut model, KeyCode::Char('3'));
        assert_eq!(model.focus, Panel::Shells, "3 jumps to Shells");
        press(&mut model, KeyCode::Char('1'));
        assert_eq!(model.focus, Panel::Runs, "1 jumps to Runs");
        press(&mut model, KeyCode::Char('4'));
        assert_eq!(model.focus, Panel::Activity, "4 jumps to Activity");
        press(&mut model, KeyCode::Char('2'));
        assert_eq!(model.focus, Panel::Budget, "2 jumps to Budget");
    }

    #[test]
    fn number_keys_type_into_the_filter_while_editing() {
        let mut model = TopModel::new(snapshot(vec![]));
        press(&mut model, KeyCode::Char('/'));
        press(&mut model, KeyCode::Char('3'));
        assert_eq!(
            model.filter, "3",
            "a digit typed while editing goes to the filter"
        );
        assert_eq!(
            model.focus,
            Panel::Runs,
            "a digit typed while editing leaves the focus"
        );
    }

    // ── scroll keys act on the focused panel only ────────────────────────────

    #[test]
    fn arrows_move_only_the_focused_panel() {
        let activity: Vec<_> = (1..=30)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let shells: Vec<_> = (1..=10).map(|i| shell(&format!("s{i}"))).collect();
        let mut model = TopModel::new(snapshot_with_all(
            vec![row("r01", Liveness::Live)],
            activity,
            shells,
        ));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Shells;
        model.clamp_offsets();
        let selected = model.selected;
        let activity_top = model.activity_top;
        press(&mut model, KeyCode::Down);
        assert!(
            model.shells_top > 0,
            "↓ moves the Shells offset when Shells is focused"
        );
        assert_eq!(
            model.selected, selected,
            "the Runs selection does not move when Shells is focused"
        );
        assert!(model.follow, "the Runs follow does not stop");
        assert!(model.tailing, "Activity keeps tailing");
        assert_eq!(
            model.activity_top, activity_top,
            "the Activity offset does not move when Shells is focused"
        );
        let shells_top = model.shells_top;
        press(&mut model, KeyCode::Down);
        assert!(
            model.shells_top > shells_top,
            "another ↓ moves the Shells offset again"
        );
        press(&mut model, KeyCode::Up);
        assert_eq!(
            model.shells_top, shells_top,
            "↑ moves the Shells offset back one"
        );
        model.focus = Panel::Budget;
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
        ] {
            press(&mut model, code);
            assert_eq!(
                model.shells_top, shells_top,
                "a scroll key on Budget leaves the Shells offset"
            );
            assert!(model.follow, "a scroll key on Budget leaves the follow");
            assert!(model.tailing, "a scroll key on Budget leaves the tailing");
        }
    }

    #[test]
    fn shells_scroll_clamps_at_both_ends() {
        let shells: Vec<_> = (1..=20).map(|i| shell(&format!("s{i}"))).collect();
        let mut model = TopModel::new(snapshot_with_shells(vec![], shells));
        model.heights = PanelHeights {
            runs: 3,
            shells: 6,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::Up);
        assert_eq!(model.shells_top, 0, "↑ at the top stays 0");
        press(&mut model, KeyCode::End);
        assert_eq!(
            model.shells_top, 14,
            "End shows the last full window: 20 - 6"
        );
        press(&mut model, KeyCode::Down);
        assert_eq!(model.shells_top, 14, "↓ at the bottom stays 14");
        press(&mut model, KeyCode::Up);
        assert_eq!(model.shells_top, 13, "one ↑ from the bottom moves one line");
        let shells: Vec<_> = (1..=3).map(|i| shell(&format!("s{i}"))).collect();
        let mut model = TopModel::new(snapshot_with_shells(vec![], shells));
        model.heights = PanelHeights {
            runs: 3,
            shells: 6,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::End);
        assert_eq!(
            model.shells_top, 0,
            "End on content that fits stays at the top"
        );
    }

    // ── Activity tailing ─────────────────────────────────────────────────────

    #[test]
    fn activity_tails_until_scrolled() {
        let activity: Vec<_> = (1..=30)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let mut model = TopModel::new(snapshot_with(vec![], activity));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Activity;
        model.clamp_offsets();
        assert!(model.tailing, "the panel starts tailing");
        assert_eq!(
            model.activity_top, 20,
            "tailing shows the newest rows: 30 - 10"
        );
        // A refresh that adds rows moves the window with them.
        let more: Vec<_> = (1..=35)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        model.snapshot.activity = more;
        model.clamp_offsets();
        assert!(model.tailing, "new rows do not stop the tail");
        assert_eq!(
            model.activity_top, 25,
            "tailing shows the newest rows after the refresh"
        );
        press(&mut model, KeyCode::Up);
        assert!(!model.tailing, "↑ stops tailing");
        let frozen = model.activity_top;
        let still: Vec<_> = (1..=40)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        model.snapshot.activity = still;
        model.clamp_offsets();
        assert!(!model.tailing, "a refresh does not resume tailing");
        assert_eq!(
            model.activity_top, frozen,
            "new rows arriving leave the top row fixed"
        );
    }

    #[test]
    fn activity_scrolling_to_the_bottom_resumes_tailing() {
        let activity: Vec<_> = (1..=30)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let mut model = TopModel::new(snapshot_with(vec![], activity));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Activity;
        model.clamp_offsets();
        press(&mut model, KeyCode::Up);
        assert!(!model.tailing, "↑ stops tailing");
        // Twenty presses move the window back to its bottom.
        for _ in 0..20 {
            press(&mut model, KeyCode::Down);
        }
        assert!(
            model.tailing,
            "scrolling until the newest row is visible resumes tailing"
        );
        press(&mut model, KeyCode::Up);
        press(&mut model, KeyCode::End);
        assert!(model.tailing, "End resumes tailing");
    }

    #[test]
    fn f_resumes_follow_and_tailing() {
        let activity: Vec<_> = (1..=30)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let shells: Vec<_> = (1..=20).map(|i| shell(&format!("s{i}"))).collect();
        let mut model = TopModel::new(snapshot_with_all(
            vec![row("r01", Liveness::Live)],
            activity,
            shells,
        ));
        model.heights = PanelHeights {
            runs: 3,
            shells: 6,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Runs;
        press(&mut model, KeyCode::Down);
        assert!(!model.follow, "scrolling Runs stops the follow");
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::Down);
        press(&mut model, KeyCode::Down);
        let shells_top = model.shells_top;
        assert!(shells_top > 0, "the Shells panel scrolled");
        assert!(!model.follow, "the Runs follow stopped");
        assert!(model.tailing, "Activity still tails until it is scrolled");
        model.focus = Panel::Activity;
        press(&mut model, KeyCode::Up);
        assert!(!model.tailing, "Activity stopped tailing");
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::Char('f'));
        assert!(model.follow, "f resumes the Runs follow");
        assert!(model.tailing, "f resumes Activity tailing");
        assert_eq!(
            model.shells_top, shells_top,
            "f leaves the Shells scroll where it was"
        );
    }

    #[test]
    fn page_and_home_keys_move_by_a_page() {
        let activity: Vec<_> = (1..=100)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let mut model = TopModel::new(snapshot_with(vec![], activity));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Activity;
        model.clamp_offsets();
        assert!(model.tailing, "the panel starts tailing");
        press(&mut model, KeyCode::Home);
        assert!(!model.tailing, "Home stops tailing");
        assert_eq!(model.activity_top, 0, "Home shows row 0 at the top");
        press(&mut model, KeyCode::PageDown);
        assert_eq!(
            model.activity_top, 9,
            "PageDown moves the top by the page: 10 - 1"
        );
        press(&mut model, KeyCode::End);
        assert!(model.tailing, "End resumes tailing");
        // The tail's top is 90 (100 - 10); one PageUp of 9 lands on 81.
        press(&mut model, KeyCode::PageUp);
        assert_eq!(
            model.activity_top, 81,
            "PageUp moves up a page from the tail"
        );
    }

    #[test]
    fn empty_panels_ignore_scroll_keys() {
        let mut model = TopModel::new(snapshot_with(vec![], vec![]));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        for focus in [Panel::Runs, Panel::Budget, Panel::Shells, Panel::Activity] {
            model.focus = focus;
            for code in [
                KeyCode::Up,
                KeyCode::Down,
                KeyCode::PageUp,
                KeyCode::PageDown,
                KeyCode::Home,
                KeyCode::End,
            ] {
                press(&mut model, code);
                assert_eq!(
                    model.shells_top, 0,
                    "a scroll key on {focus:?} leaves the Shells offset at 0"
                );
                assert_eq!(
                    model.activity_top, 0,
                    "a scroll key on {focus:?} leaves the Activity offset at 0"
                );
                assert_eq!(
                    model.selected, 0,
                    "a scroll key on {focus:?} leaves the Runs selection at 0"
                );
            }
        }
    }

    #[test]
    fn offsets_clamp_when_content_or_size_shrinks() {
        let activity: Vec<_> = (1..=100)
            .map(|i| activity_row("12:00:00", "ai_cost", &format!("model{i:02}")))
            .collect();
        let mut model = TopModel::new(snapshot_with(vec![], activity));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        model.focus = Panel::Activity;
        model.clamp_offsets();
        press(&mut model, KeyCode::Home);
        press(&mut model, KeyCode::PageDown);
        model.activity_top = 80;
        model.clamp_offsets();
        assert_eq!(
            model.activity_top, 80,
            "a top of 80 of 100 rows with height 10 stays: 100 - 10 = 90 >= 80"
        );
        // A filter that leaves 5 rows clamps the offset back inside. The
        // filter keeps every `ai_cost` row, so 100 rows stay; the point is
        // that the clamp sees the filtered count.
        model.filter = "ai_cost".to_string();
        model.clamp_offsets();
        let top = model.activity_top;
        let total = activity_text(&model.filtered_activity()).lines().count();
        assert_eq!(total, 100, "the filter keeps every ai_cost row");
        assert!(
            top + model.heights.activity <= total,
            "the window lies inside the filtered content: top {top} + 10 <= {total}"
        );
        // A filter that leaves fewer rows than the panel shows clamps the
        // offset to 0.
        model.filter = "model01".to_string();
        model.clamp_offsets();
        let top = model.activity_top;
        assert_eq!(top, 0, "a window taller than the content clamps to 0");
        // Halved heights clamp the offset again.
        model.filter = "ai_cost".to_string();
        model.heights.activity = 5;
        model.clamp_offsets();
        let top = model.activity_top;
        let total = activity_text(&model.filtered_activity()).lines().count();
        assert!(
            top + model.heights.activity <= total,
            "the window lies inside the content at the smaller height"
        );
        assert!(top <= total.saturating_sub(model.heights.activity));
    }

    // ── visible_range ────────────────────────────────────────────────────────

    #[test]
    fn visible_range_never_leaves_the_content() {
        assert!(
            visible_range(0, 10, 0).is_empty(),
            "no content gives an empty range"
        );
        assert_eq!(
            visible_range(5, 10, 0),
            0..5,
            "content under the height shows all of it"
        );
        assert_eq!(
            visible_range(20, 10, 15),
            10..20,
            "a top past the last window clamps to the last window"
        );
        assert!(
            visible_range(5, 0, 0).is_empty(),
            "a zero height shows nothing"
        );
        for total in [0usize, 1, 5, 20] {
            for inner_h in [0usize, 1, 6, 10] {
                for top in [0usize, 3, 15, 100] {
                    let range = visible_range(total, inner_h, top);
                    assert!(
                        range.start <= range.end && range.end <= total,
                        "the range lies inside 0..{total}: {range:?}"
                    );
                }
            }
        }
    }

    // ── screen ───────────────────────────────────────────────────────────────

    #[test]
    fn screen_names_the_focused_panel() {
        let mut model = TopModel::new(snapshot(vec![]));
        model.focus = Panel::Shells;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model)).unwrap();
        let buffer = terminal.backend().buffer().content.clone();
        let cells_at = |y: u16| {
            (0..80)
                .map(|x| &buffer[y as usize * 80 + x as usize])
                .collect::<Vec<_>>()
        };
        let title_row_of = |needle: &str| {
            (0..24u16)
                .find(|y| {
                    cells_at(*y)
                        .iter()
                        .map(|c| c.symbol())
                        .collect::<String>()
                        .contains(needle)
                })
                .expect("the title row")
        };
        let shells_row = title_row_of(" 3 Shells ");
        let runs_row = title_row_of(" 1 Runs ");
        let shells_cyan = cells_at(shells_row)
            .iter()
            .filter(|c| c.fg == ratatui::style::Color::Cyan)
            .count();
        assert!(
            shells_cyan >= 9,
            "the focused Shells title is cyan: {shells_cyan} cells"
        );
        let runs_cyan = cells_at(runs_row)
            .iter()
            .filter(|c| c.fg == ratatui::style::Color::Cyan)
            .count();
        assert_eq!(runs_cyan, 0, "the unfocused Runs title is not cyan");
        let footer = cells_at(23).iter().map(|c| c.symbol()).collect::<String>();
        assert!(
            footer.contains("focus: Shells"),
            "the footer names the focus: {footer:?}"
        );
    }

    #[test]
    fn screen_draws_scrolled_shells() {
        let shells: Vec<_> = (1..=10).map(|i| shell(&format!("s{i}"))).collect();
        let mut model = TopModel::new(snapshot_with_shells(vec![], shells));
        model.shells_top = 2;
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
            text.contains("s3 chat"),
            "the third shell is the first body row at offset 2: {text:?}"
        );
        assert!(
            !text.contains("s1 chat"),
            "the first shell scrolled off: {text:?}"
        );
        assert!(
            !text.contains("s2 chat"),
            "the second shell scrolled off with it: {text:?}"
        );
    }

    // ── bounce 1: stored heights and the sliding Activity window ─────────────

    /// The heights `draw_at` lays out, computed here from the drawn text so a
    /// drift in the input convention between drawing and measuring fails.
    fn drawn_heights(model: &TopModel, size: ratatui::layout::Rect) -> PanelHeights {
        let rects = plan_layout(
            size,
            runs_text(&model.snapshot.runs).lines().count(),
            budget_text(&model.snapshot.budget).lines().count()
                + usize::from(model.snapshot.budget.cap > 0.0),
            shells_text(&model.snapshot.shells).lines().count(),
        );
        PanelHeights {
            runs: inner_height(rects.runs),
            shells: inner_height(rects.shells),
            activity: inner_height(rects.activity),
            detail: inner_height(detail_rect(&rects, size)),
        }
    }

    #[test]
    fn stored_heights_match_the_drawn_layout() {
        let size = ratatui::layout::Rect::new(0, 0, 80, 24);
        for cap in [0.0, 10.0] {
            let mut model = TopModel::new(snapshot_with_all(
                vec![row("r01", Liveness::Live), row("r02", Liveness::Done)],
                vec![],
                vec![shell("s1")],
            ));
            model.snapshot.budget.cap = cap;
            model.after_refresh(Some(size));
            assert_eq!(
                model.heights,
                drawn_heights(&model, size),
                "the stored heights match the drawn layout at cap {cap}"
            );
            // The content grows; no resize in between.
            model.snapshot.shells =
                ShellsPanel::Rows((1..=10).map(|i| shell(&format!("s{i}"))).collect());
            model.snapshot.runs = (1..=20)
                .map(|i| row(&format!("r{i:02}"), Liveness::Done))
                .collect();
            model.after_refresh(Some(size));
            assert_eq!(
                model.heights,
                drawn_heights(&model, size),
                "the stored heights follow the grown content at cap {cap}"
            );
        }
    }

    /// The loop's refresh path — `after_refresh`, never `refresh_heights` on
    /// its own — re-measures the stored heights after any content refresh,
    /// with no resize in between.
    #[test]
    fn content_refresh_re_measures_heights() {
        let size = ratatui::layout::Rect::new(0, 0, 80, 24);
        let mut model = TopModel::new(snapshot_with_all(
            vec![row("r01", Liveness::Live), row("r02", Liveness::Done)],
            vec![],
            vec![shell("s1")],
        ));
        model.after_refresh(Some(size));
        let before = model.heights;

        // A disk refresh grows the runs from 2 to 20.
        model.snapshot.runs = (1..=20)
            .map(|i| row(&format!("r{i:02}"), Liveness::Done))
            .collect();
        model.after_refresh(Some(size));
        assert_ne!(
            model.heights, before,
            "a run refresh moves the heights: {} -> {:?}",
            before.runs, model.heights.runs
        );
        assert_eq!(
            model.heights,
            drawn_heights(&model, size),
            "the stored heights match the drawn layout after the run refresh"
        );

        // A shells refresh grows the shells from 1 to 10.
        model.snapshot.shells =
            ShellsPanel::Rows((1..=10).map(|i| shell(&format!("s{i}"))).collect());
        model.after_refresh(Some(size));
        assert_eq!(
            model.heights,
            drawn_heights(&model, size),
            "the stored heights match the drawn layout after the shells refresh"
        );
    }

    #[test]
    fn scrolled_activity_holds_its_rows_as_the_window_slides() {
        let activity: Vec<_> = (1..=50)
            .map(|i| activity_row(&format!("12:{i:02}:00"), "ai_cost", &format!("model{i:02}")))
            .collect();
        let mut model = TopModel::new(snapshot_with(vec![], activity));
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 8,
            detail: 0,
        };
        model.focus = Panel::Activity;
        model.clamp_offsets();
        for _ in 0..10 {
            press(&mut model, KeyCode::Up);
        }
        assert!(!model.tailing, "scrolled up 10, the panel stopped tailing");
        assert_eq!(model.activity_top, 32, "the tail top is 42; 10 up is 32");
        let panel_text = |terminal: &Terminal<ratatui::backend::TestBackend>| -> Vec<String> {
            (0..24u16)
                .map(|y| {
                    (0..80u16)
                        .map(|x| {
                            terminal.backend().buffer().content[(y * 80 + x) as usize].symbol()
                        })
                        .collect::<String>()
                })
                .filter(|l| l.contains("model"))
                .collect()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model)).expect("a draw");
        let before = panel_text(&terminal);
        assert_eq!(
            before.len(),
            8,
            "the panel's rows are on screen: {before:?}"
        );
        // A refresh drops the five oldest rows and lands five new ones: the
        // window slides under a fixed index.
        let mut fresh: Vec<_> = (6..=50)
            .map(|i| activity_row(&format!("12:{i:02}:00"), "ai_cost", &format!("model{i:02}")))
            .collect();
        fresh.extend(
            (51..=55).map(|i| {
                activity_row(&format!("12:{i:02}:00"), "ai_cost", &format!("model{i:02}"))
            }),
        );
        model.snapshot.activity = fresh;
        model.hold_activity();
        model.clamp_offsets();
        terminal.draw(|f| draw(f, &model)).expect("a draw");
        let after = panel_text(&terminal);
        assert_eq!(
            before, after,
            "the visible rows hold still as the window slides"
        );
        press(&mut model, KeyCode::End);
        terminal.draw(|f| draw(f, &model)).expect("a draw");
        let end = panel_text(&terminal);
        assert!(
            end.iter().any(|l| l.contains("model55")),
            "End shows the newest row: {end:?}"
        );
        assert!(
            !end.iter().any(|l| l.contains("model05")),
            "End left the scrolled window: {end:?}"
        );
    }

    // ── run detail ───────────────────────────────────────────────────────────

    use crate::daemon::run::{RunId, TurnLogEvent, TurnLogRecord, turn_log_file};

    /// RAII-guarded HOME pointing at a fresh tempdir, so a detail test that
    /// loads from disk never touches the developer's real `~/.daemoneye`.
    struct HomeGuard {
        _lock: crate::TestHomeGuard,
        _tmp: tempfile::TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let _lock = crate::test_home_guard();
            let tmp = tempfile::tempdir().expect("a tempdir");
            unsafe { std::env::set_var("HOME", tmp.path()) };
            HomeGuard { _lock, _tmp: tmp }
        }
    }

    const DETAIL_ID: &str = "7eb9151d45fa49f1";

    fn detail_id() -> RunId {
        RunId::parse(DETAIL_ID).expect("a valid fixture id")
    }

    fn write_detail_log(notes: usize) {
        let id = detail_id();
        crate::daemon::run::append_turn_event(&TurnLogRecord {
            timestamp: "2026-10-25T09:00:00Z".to_string(),
            run_id: id.clone(),
            event: TurnLogEvent::RunStarted {
                kind: crate::daemon::run::RunKind::Ghost {
                    runbook: "disk-full-l4".into(),
                    trigger: "webhook".into(),
                },
            },
        });
        for i in 0..notes {
            crate::daemon::run::append_turn_event(&TurnLogRecord {
                timestamp: "2026-10-25T09:00:01Z".to_string(),
                run_id: id.clone(),
                event: TurnLogEvent::Note {
                    text: format!("note-{i:03}"),
                },
            });
        }
    }

    fn open_detail_on(model: &mut TopModel) {
        model.open_detail(detail_id());
    }

    #[test]
    fn enter_opens_the_selected_runs_detail() {
        let _g = HomeGuard::new();
        write_detail_log(2);
        let mut model = TopModel::new(snapshot(vec![row(DETAIL_ID, Liveness::Done)]));
        model.selected = 0;
        press(&mut model, KeyCode::Enter);
        let detail = model.detail.as_ref().expect("the detail opened");
        assert_eq!(detail.id, detail_id(), "the detail names the selected run");
        assert!(
            detail.text.contains(DETAIL_ID),
            "the heading names the run: {:?}",
            detail.id
        );
        assert!(
            detail.text.contains("note-000") && detail.text.contains("note-001"),
            "the log tail is in the text: {:?}",
            detail.id
        );
        assert_eq!(model.detail_top, 0, "the view opens at the top");
    }

    #[test]
    fn enter_does_nothing_off_runs_with_no_runs_or_while_editing() {
        let mut model = TopModel::new(snapshot(vec![]));
        model.selected = 0;
        press(&mut model, KeyCode::Enter);
        assert!(model.detail.is_none(), "no runs, no detail");

        let mut model = TopModel::new(snapshot(vec![
            row("r01", Liveness::Live),
            row("r02", Liveness::Done),
        ]));
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::Enter);
        assert!(model.detail.is_none(), "Enter off Runs does nothing");

        model.focus = Panel::Runs;
        press(&mut model, KeyCode::Char('/'));
        assert!(model.editing, "setup: the filter is being edited");
        press(&mut model, KeyCode::Enter);
        assert!(
            model.detail.is_none() && !model.editing,
            "while editing, Enter ends editing and opens nothing"
        );
    }

    #[test]
    fn esc_restores_focus_selection_and_offsets() {
        let _g = HomeGuard::new();
        write_detail_log(1);
        let activity: Vec<crate::cli::commands::top::ActivityRow> = (0..30)
            .map(|i| activity_row(&format!("09:00:{i:02}"), "event", &format!("row-{i:02}")))
            .collect();
        let mut model = TopModel::new(snapshot_with_all(
            vec![row("r01", Liveness::Live), row("r02", Liveness::Done)],
            activity,
            vec![shell("s1"), shell("s2"), shell("s3")],
        ));
        model.focus = Panel::Shells;
        model.selected = 1;
        model.heights = PanelHeights {
            runs: 3,
            shells: 2,
            activity: 10,
            detail: 4,
        };
        model.activity_top = 2;
        model.tailing = false;
        model.clamp_offsets();
        model.shells_top = 1;
        let (focus, selected, shells_top, activity_top) = (
            model.focus,
            model.selected,
            model.shells_top,
            model.activity_top,
        );
        assert_eq!(activity_top, 2, "setup: the Activity window sat at row 2");
        open_detail_on(&mut model);
        assert!(model.detail.is_some(), "setup: the detail opened");
        press(&mut model, KeyCode::Esc);
        assert!(model.detail.is_none(), "Esc closes the detail");
        assert_eq!(model.focus, focus, "Esc restores the focus");
        assert_eq!(model.selected, selected, "Esc restores the selection");
        assert_eq!(model.shells_top, shells_top, "Esc restores Shells' offset");
        assert_eq!(
            model.activity_top, activity_top,
            "Esc restores Activity's offset"
        );
    }

    #[test]
    fn detail_scroll_clamps_at_both_ends() {
        let text = (0..50)
            .map(|i| format!("line-{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut model = TopModel::new(snapshot(vec![row(DETAIL_ID, Liveness::Done)]));
        model.detail = Some(OpenDetail {
            id: detail_id(),
            text,
        });
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 10,
        };
        model.detail_top = 5;
        press(&mut model, KeyCode::End);
        assert_eq!(model.detail_top, 40, "End stops at the last full window");
        press(&mut model, KeyCode::Down);
        assert_eq!(model.detail_top, 40, "↓ past the end stays 40");
        press(&mut model, KeyCode::Up);
        assert_eq!(model.detail_top, 39, "one ↑ moves one line back");

        model.detail = Some(OpenDetail {
            id: detail_id(),
            text: "one\ntwo\nthree".to_string(),
        });
        press(&mut model, KeyCode::End);
        assert_eq!(
            model.detail_top, 0,
            "three lines in a ten-line window: End is 0"
        );
    }

    #[test]
    fn detail_refresh_keeps_the_scroll_and_survives_a_vanished_run() {
        let _g = HomeGuard::new();
        let size = ratatui::layout::Rect::new(0, 0, 80, 24);
        write_detail_log(30);
        let mut model = TopModel::new(snapshot(vec![row(DETAIL_ID, Liveness::Live)]));
        open_detail_on(&mut model);
        model.after_refresh(Some(size));
        press(&mut model, KeyCode::End);
        let top = model.detail_top;
        assert!(top > 0, "setup: 30 notes overflow a 20-line panel");

        write_detail_log(40);
        model.reload_detail();
        model.after_refresh(Some(size));
        assert_eq!(
            model.detail_top, top,
            "a refresh that grew the log keeps the scroll offset"
        );

        std::fs::remove_file(turn_log_file(&detail_id())).expect("removing the log");
        model.reload_detail();
        let detail = model.detail.as_ref().expect("the detail stays open");
        assert!(
            detail
                .text
                .contains(&format!("run {DETAIL_ID} is no longer on disk")),
            "a vanished run draws the message, not a panic: {}",
            detail.text
        );
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| draw(f, &model))
            .expect("the vanished run still draws");
    }

    #[test]
    fn panel_keys_do_nothing_in_the_detail() {
        let mut model = TopModel::new(snapshot(vec![
            row("r01", Liveness::Live),
            row("r02", Liveness::Done),
        ]));
        model.focus = Panel::Shells;
        model.detail = Some(OpenDetail {
            id: detail_id(),
            text: "heading\nbody".to_string(),
        });
        let (focus, filter, follow, tailing) = (
            model.focus,
            model.filter.clone(),
            model.follow,
            model.tailing,
        );
        for code in [
            KeyCode::Tab,
            KeyCode::Char('3'),
            KeyCode::Char('f'),
            KeyCode::Char('/'),
        ] {
            press(&mut model, code);
        }
        assert_eq!(
            model.focus, focus,
            "Tab does not move the focus in the detail"
        );
        assert_eq!(model.filter, filter, "'f' and '/' leave the filter alone");
        assert!(!model.editing, "'/' does not start editing in the detail");
        assert_eq!(
            model.follow, follow,
            "'f' does not change follow in the detail"
        );
        assert_eq!(
            model.tailing, tailing,
            "'f' does not change tailing in the detail"
        );
    }

    #[test]
    fn the_detail_title_is_padded_once() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut model = TopModel::new(snapshot(vec![
            row("r01", Liveness::Live),
            row("r02", Liveness::Done),
        ]));
        model.detail = Some(OpenDetail {
            id: detail_id(),
            text: "7eb9151d45fa49f1  ghost  disk-full-l4 ← webhook  hard-fail  2m".to_string(),
        });
        terminal.draw(|f| draw(f, &model)).unwrap();
        let rows = drawn_rows(&terminal);
        assert!(
            rows.iter()
                .any(|row| row.contains(&format!("╭ Run {} ─", DETAIL_ID))),
            "a row contains the padded detail title: {rows:?}"
        );
        for row in &rows {
            assert!(
                !row.contains("╭  Run"),
                "no doubled padding before the detail title: {row:?}"
            );
        }
    }

    #[test]
    fn screen_draws_the_detail_view() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut model = TopModel::new(snapshot(vec![
            row("r01", Liveness::Live),
            row("r02", Liveness::Done),
        ]));
        model.detail = Some(OpenDetail {
            id: detail_id(),
            text:
                "7eb9151d45fa49f1  ghost  disk-full-l4 ← webhook  hard-fail  2m\nstatus: hard_fail"
                    .to_string(),
        });
        let draw_time = "2026-10-22T09:00:00+00:00"
            .parse::<DateTime<FixedOffset>>()
            .expect("a fixed time");
        terminal
            .draw(|f| draw_at(f, &model, draw_time))
            .expect("a draw");
        let text: String = terminal
            .backend()
            .to_string()
            .lines()
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(
            text.contains(&format!("╭ Run {DETAIL_ID} ─")),
            "the panel is titled with the run id: {text}"
        );
        assert!(
            text.contains("esc back"),
            "the footer names esc back: {text}"
        );
        assert!(
            !text.contains(" 1 Runs "),
            "the panels are not drawn behind the detail: {text}"
        );
    }

    #[test]
    fn screen_detail_on_a_tiny_terminal() {
        let mut model = TopModel::new(snapshot(vec![row(DETAIL_ID, Liveness::Done)]));
        model.detail = Some(OpenDetail {
            id: detail_id(),
            text: "7eb9151d45fa49f1  ghost  disk-full-l4\nno run record yet".to_string(),
        });
        for (w, h) in [(40u16, 6u16), (1u16, 1u16)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| draw(f, &model))
                .unwrap_or_else(|_| panic!("the detail draws at {w}x{h}"));
        }
    }
}
