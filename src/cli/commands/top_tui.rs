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
    ActivityRow, BarLevel, Liveness, PanelRects, ShellsPanel, TopFingerprint, TopRunRow,
    activity_text, budget_bar, budget_text, collapse_activity, filter_activity, fingerprint,
    header_text, plan_layout, rect_or_area, run_fades, runs_text, shells_text,
};
use crate::cli::viewer::AltScreenGuard;
use crate::ipc::Response;
/// Poll the disk fingerprint every this long.
const TICK: Duration = Duration::from_millis(500);

/// A budget bar's chrome: the two brackets and the ` NNN%` tail the panel
/// draws beside the cells — three digits, because the percentage is printed
/// as it is and runs over the cap.
const BAR_CHROME: usize = 7;

/// Replace the model's Shells panel and daemon header with fresh answers
/// from the daemon: one read-only `Request::ShellList` and one
/// `Request::Status`, the header's facts on the same 2 s cadence as the
/// shells. The selection stays on the shell it was on; a shell that has
/// gone clamps it into the new rows. A test drives it without a daemon
/// behind the [`ShellFetcher`] seam.
pub(crate) async fn shells_refresh(model: &mut TopModel, fetch: &impl ShellFetcher) {
    let keep = model.selected_shell_id();
    model.snapshot.shells = fetch.shells().await;
    model.snapshot.daemon = fetch.daemon().await;
    model.restore_shell_selection(keep);
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
    disk_in_flight: bool,
) -> Refresh {
    let disk =
        !disk_in_flight && (last.is_none() || last != Some(now) || since_disk >= DISK_MAX_AGE);
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
    /// content and the panel's inner height, and following the selected
    /// shell.
    pub shells_top: usize,
    /// The selected shell row's index into the Shells panel's rows; 0 when
    /// the panel has none.
    pub shells_selected: usize,
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
    /// The open attach view on a shell, when one is.
    pub attach: Option<AttachBox>,
    /// Why the last attach open failed; cleared by the next key.
    pub attach_note: Option<String>,
    /// Set while a key asked for the attach view to open; the loop opens it.
    pub attach_requested: bool,
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
            shells_selected: 0,
            activity_top: 0,
            tailing: true,
            activity_anchor: None,
            heights: PanelHeights::default(),
            filter: String::new(),
            editing: false,
            quit: false,
            detail: None,
            detail_top: 0,
            attach: None,
            attach_note: None,
            attach_requested: false,
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
        if self.attach.is_some() {
            self.attach_key(key);
            self.clamp_offsets();
            return;
        }
        self.attach_note = None;
        if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
            match (key.code, self.focus) {
                (KeyCode::Enter | KeyCode::Char('a'), Panel::Shells)
                    if self.attach_candidate().is_some() =>
                {
                    self.attach_requested = true;
                }
                _ => {}
            }
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

    /// One line up on the focused panel. Runs and Shells move their
    /// selection (Runs stops follow); Activity moves its top offset and
    /// stops tailing; Budget ignores the key.
    fn scroll_up(&mut self, n: usize) {
        match self.focus {
            Panel::Runs => {
                self.selected = self.selected.saturating_sub(n);
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => {
                if self.shells_row_count().is_some() {
                    self.shells_selected = self.shells_selected.saturating_sub(n);
                    self.follow_shells();
                }
            }
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
                if let Some(rows) = self.shells_row_count() {
                    self.shells_selected = (self.shells_selected + n).min(rows - 1);
                    self.follow_shells();
                }
            }
            Panel::Activity => {
                self.activity_top += n;
                self.tailing = false;
            }
        }
    }

    /// The top of the focused panel. Runs and Shells select their first row
    /// (Runs stops follow); Activity goes to its first body line and stops
    /// tailing; Budget ignores the key.
    fn scroll_home(&mut self) {
        match self.focus {
            Panel::Runs => {
                self.selected = 0;
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => {
                if self.shells_row_count().is_some() {
                    self.shells_selected = 0;
                    self.follow_shells();
                }
            }
            Panel::Activity => {
                self.activity_top = 0;
                self.tailing = false;
            }
        }
    }

    /// The bottom of the focused panel. Runs and Shells select their last
    /// row (Runs stops follow); Activity scrolls to its last full window;
    /// Budget ignores the key. Scrolling Activity to its bottom resumes
    /// tailing.
    fn scroll_end(&mut self) {
        match self.focus {
            Panel::Runs => {
                self.selected = self.snapshot.runs.len().saturating_sub(1);
                self.follow = false;
            }
            Panel::Budget => {}
            Panel::Shells => {
                if let Some(rows) = self.shells_row_count() {
                    self.shells_selected = rows - 1;
                    self.follow_shells();
                }
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
    /// resizes them without a terminal resize — clamp the offsets, and
    /// keep the Shells window on its selection. The loop calls this after
    /// every disk and shells refresh and on resize; it never clamps on its
    /// own.
    pub fn after_refresh(&mut self, size: Option<ratatui::layout::Rect>) {
        self.hold_activity();
        refresh_heights(self, size);
        self.clamp_offsets();
        self.follow_shells();
    }

    /// Apply one disk read to the model: the three panels replaced, the
    /// frame's `taken_at` kept — the header clock is the draw time, not the
    /// read's — follow re-targeted and an open detail reloaded. The loop
    /// calls it with a blocking read's result; [`Self::after_refresh`]
    /// follows.
    fn apply_disk(&mut self, disk: crate::cli::commands::top::DiskPanels) {
        let taken_at = self.snapshot.taken_at;
        self.snapshot.runs = disk.runs;
        self.snapshot.budget = disk.budget;
        self.snapshot.activity = disk.activity;
        self.snapshot.taken_at = taken_at;
        if self.follow {
            self.selected = follow_target(&self.snapshot.runs);
        }
        self.reload_detail();
    }

    /// The Shells panel's shell-row count: `None` when the panel shows the
    /// daemon-down mark or `none` — nothing selectable.
    pub(crate) fn shells_row_count(&self) -> Option<usize> {
        match &self.snapshot.shells {
            ShellsPanel::Rows(rows) if !rows.is_empty() => Some(rows.len()),
            _ => None,
        }
    }

    /// The selected shell's id, when the panel has a row at the selection.
    pub(crate) fn selected_shell_id(&self) -> Option<String> {
        match &self.snapshot.shells {
            ShellsPanel::Rows(rows) => rows.get(self.shells_selected).map(|s| s.id.clone()),
            ShellsPanel::DaemonDown => None,
        }
    }

    /// Put the selection back on `id` after a Shells refresh: the same shell
    /// keeps it, a gone or absent one clamps into the new rows, and the
    /// window follows the selection.
    fn restore_shell_selection(&mut self, id: Option<String>) {
        if let Some(id) = id
            && let ShellsPanel::Rows(rows) = &self.snapshot.shells
            && let Some(idx) = rows.iter().position(|s| s.id == id)
        {
            self.shells_selected = idx;
        }
        self.follow_shells();
    }

    /// Keep the Shells window on the selected row and the selection inside
    /// the rows: the panel's window follows the selection, as the Runs
    /// panel's does. A panel with no rows selects nothing.
    fn follow_shells(&mut self) {
        let rows = match &self.snapshot.shells {
            ShellsPanel::Rows(rows) => rows.len(),
            ShellsPanel::DaemonDown => 0,
        };
        self.shells_selected = self.shells_selected.min(rows.saturating_sub(1));
        let inner = self.heights.shells;
        if inner == 0 {
            return;
        }
        self.shells_top = self.shells_top.min(self.last_window_top(rows, inner));
        if self.shells_selected < self.shells_top {
            self.shells_top = self.shells_selected;
        } else if self.shells_selected >= self.shells_top + inner {
            self.shells_top = self.shells_selected + 1 - inner;
        }
    }

    /// The selected shell's id when a key may open the attach view on it:
    /// Shells focused, not editing the filter, and a row selected. The
    /// drawn selection is the selection — the panel starts on row 0
    /// selected, so a bare `Enter` on a populated Shells opens that row.
    pub fn attach_candidate(&self) -> Option<String> {
        if self.focus != Panel::Shells || self.editing {
            return None;
        }
        self.shells_row_count()
            .and_then(|_| self.selected_shell_id())
    }

    /// Apply one key to the open attach view; the tail viewer's key table
    /// decides what it means. `Ctrl-C` closes the view and quits `top`.
    /// Returns nothing: the view's own state is the answer.
    fn attach_key(&mut self, key: KeyEvent) {
        self.attach_note = None;
        let Some(view) = self.attach.as_mut() else {
            return;
        };
        let page = self.heights.shells.max(1);
        if !attach_key(&mut view.view, key, page)
            && let Some(boxed) = self.attach.take()
        {
            boxed.view.task.abort();
        }
        self.clamp_offsets();
    }

    /// Apply one frame the attach view's channel gave: `Some` feeds the
    /// model, `None` marks the stream ended and keeps the view. Kept on the
    /// model so the select arm reads as one step.
    fn apply_attach_frame(&mut self, frame: Option<Response>) {
        let Some(view) = self.attach.as_mut() else {
            return;
        };
        view.view.apply_frame(frame);
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

#[path = "top_tui_draw.rs"]
mod draw;
pub use draw::draw;
use draw::inner_height;
#[cfg(test)]
use draw::{draw_at, visible_range};
#[path = "top_tui_attach.rs"]
mod attach;
use attach::attach_key;
pub(crate) use attach::{AttachBox, AttachOpen, open_attach};
#[cfg(test)]
#[path = "top_tui_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "top_tui_once_tests.rs"]
mod top_once_escape_tests;

/// Wait for one frame from the open attach view's channel: `Some` applies
/// it, `None` marks the model's stream ended and keeps the view. Called
/// from `run_tui`'s select arm; the receive is cancel-safe, and the channel
/// closing (its task ended) is the stream ending too.
async fn attach_frame(model: &mut TopModel) -> Option<Response> {
    model.attach.as_mut()?.view.frames.recv().await
}

/// Open the attach view the last key asked for: the request, the first
/// frame, and the forwarding task. A failure names itself in the footer and
/// the panels stay.
async fn open_requested_attach(model: &mut TopModel) {
    let Some(id) = model
        .attach_requested
        .then(|| model.attach_candidate())
        .flatten()
    else {
        model.attach_requested = false;
        return;
    };
    model.attach_requested = false;
    match open_attach(&id).await {
        AttachOpen::Opened(view) => model.attach = Some(AttachBox { view: *view }),
        AttachOpen::Failed(msg) => model.attach_note = Some(msg),
    }
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
    let mut disk_in_flight = false;
    let (disk_tx, mut disk_rx) = tokio::sync::mpsc::unbounded_channel();
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
                if model.attach_requested && model.attach.is_none() {
                    open_requested_attach(&mut model).await;
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
                let plan =
                    plan_refresh(last_fp, now_fp, since_disk, since_shells, disk_in_flight);
                if plan.disk && !disk_in_flight {
                    let tx = disk_tx.clone();
                    let cap = config_cap;
                    tokio::task::spawn_blocking(move || {
                        let disk = crate::cli::commands::top::disk_panels(cap);
                        let _ = tx.send(disk);
                    });
                    disk_in_flight = true;
                    last_fp = Some(now_fp);
                }
                if plan.shells {
                    shells_refresh(&mut model, &IpcFetcher).await;
                    model.after_refresh(size);
                    since_shells = Duration::ZERO;
                }
                terminal.draw(|f| draw(f, &model))?;
            }
            Some(disk) = disk_rx.recv() => {
                disk_in_flight = false;
                since_disk = Duration::ZERO;
                model.apply_disk(disk);
                model.after_refresh(size);
                terminal.draw(|f| draw(f, &model))?;
            }
            frame = attach_frame(&mut model), if model.attach.is_some() => {
                model.apply_attach_frame(frame);
                terminal.draw(|f| draw(f, &model))?;
            }
        }
    };

    running.store(false, Ordering::Relaxed);
    let _ = reader.join();
    result
}
