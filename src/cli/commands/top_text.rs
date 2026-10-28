//! `top`'s text frames and layout: the panel texts, the budget bar, the header and `plan_layout`.

use super::*;

// ── Text frame ───────────────────────────────────────────────────────────────

/// The Budget section's text: spend, cap and percentage when a cap is set,
/// the spend and `no cap` when not, then the refusal count. Pure.
pub fn budget_text(budget: &BudgetPanel) -> String {
    let mut lines = Vec::new();
    let mut first = format!("spend: ${:.2}", budget.ai_cost);
    if budget.cap > 0.0 {
        let pct = budget.ai_cost / budget.cap * 100.0;
        first.push_str(&format!(" of ${:.2} cap ({:.0}%)", budget.cap, pct));
    } else {
        first.push_str(" (no cap)");
    }
    lines.push(first);
    lines.push(format!("tokens today: {}", budget.tokens));
    lines.push(format!("rate (60 s): {:.1} tok/s", budget.tok_per_s));
    lines.push(format!("refused today: {}", budget.refused));
    lines.join("\n")
}

/// How far today's spend is into the cap, so the bar is drawn a colour per
/// level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarLevel {
    /// Under 80 % of the cap.
    Ok,
    /// From 80 % of the cap.
    Warn,
    /// From 100 % of the cap.
    Over,
}

/// The Budget panel's spend bar: `[` + `width` cells of `█` filled and `░`
/// empty + `] NN%`, or `None` when there is no cap. The fill clamps to
/// `width` when spend is over the cap; the percentage is printed as it is.
/// Pure.
pub fn budget_bar(spent: f64, cap: f64, width: usize) -> Option<(String, BarLevel)> {
    if cap <= 0.0 {
        return None;
    }
    let pct = spent / cap * 100.0;
    let filled = if spent >= cap {
        width
    } else {
        ((spent / cap) * width as f64).round() as usize
    }
    .min(width);
    let bar = format!(
        "[{}{}] {}%",
        "█".repeat(filled),
        "░".repeat(width - filled),
        pct.round() as i64
    );
    let level = if pct >= 100.0 {
        BarLevel::Over
    } else if pct >= 80.0 {
        BarLevel::Warn
    } else {
        BarLevel::Ok
    };
    Some((bar, level))
}

/// Whether a run's line is drawn dimmed: a finished run past five minutes of
/// age stops shouting at the screen. Pure.
pub fn run_fades(row: &TopRunRow) -> bool {
    row.liveness == Liveness::Done && row.age_s >= 300
}

/// The Runs section's text. Pure.
pub fn runs_text(runs: &[TopRunRow]) -> String {
    if runs.is_empty() {
        return "none".to_string();
    }
    let id_w = runs.iter().map(|r| r.id.len()).max().unwrap_or(0).max(2);
    let kind_w = runs.iter().map(|r| r.kind.len()).max().unwrap_or(0).max(4);
    let subject_w = runs
        .iter()
        .map(|r| r.subject.chars().count())
        .max()
        .unwrap_or(0)
        .max(7);
    let state_w = runs.iter().map(|r| r.state.len()).max().unwrap_or(0).max(5);
    let live_w = runs
        .iter()
        .map(|r| r.liveness.label().len())
        .max()
        .unwrap_or(0)
        .max(8);
    let mut out = format!(
        "{:>id_w$}  {:<kind_w$}  {:<subject_w$}  {:<state_w$}  {:>5}  {:>8}  {:<live_w$}\n",
        "ID", "KIND", "SUBJECT", "STATE", "TURNS", "AGE", "LIVENESS"
    );
    for run in runs {
        out.push_str(&format!(
            "{:>id_w$}  {:<kind_w$}  {:<subject_w$}  {:<state_w$}  {:>5}  {:>8}  {:<live_w$}\n",
            run.id,
            run.kind,
            run.subject,
            run.state,
            run.turns,
            format_age(run.age_s),
            run.liveness.label(),
        ));
    }
    out.trim_end_matches('\n').to_string()
}

/// One run's age as a compact string: seconds under a minute, `Mm` under an
/// hour, `HhMM` under a day, `DdHHh` beyond.
pub(super) fn format_age(secs: i64) -> String {
    if secs < 0 {
        return "0s".to_string();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h{:02}m", hours, mins % 60);
    }
    format!("{}d{}h", hours / 24, hours % 24)
}

/// The Activity section's text. Pure.
pub fn activity_text(rows: &[ActivityRow]) -> String {
    if rows.is_empty() {
        return "none".to_string();
    }
    rows.iter()
        .map(|r| format!("{} {} {}", r.time, r.name, r.summary))
        .collect::<Vec<String>>()
        .join("\n")
}

/// The header line: the daemon's pid, uptime, provider and model, the host
/// and the local time — `daemon not running` where the daemon facts would
/// go when no daemon answered. Pure.
pub fn header_text(
    daemon: Option<&DaemonHeader>,
    host: &str,
    now: DateTime<FixedOffset>,
) -> String {
    let clock = now.format("%H:%M:%S");
    match daemon {
        Some(d) => format!(
            "daemoneye · pid {} · up {} · {}/{} · {host} · {clock}",
            d.pid,
            crate::cli::status::fmt_uptime(d.uptime_secs),
            d.provider,
            d.model
        ),
        None => format!("daemoneye · daemon not running · {host} · {clock}"),
    }
}

/// The live screen's rects, one per region: the unbordered header and
/// footer lines plus the four bordered panels.
pub struct PanelRects {
    /// The unbordered header line.
    pub header: Rect,
    /// The Runs panel.
    pub runs: Rect,
    /// The Budget panel.
    pub budget: Rect,
    /// The Shells panel.
    pub shells: Rect,
    /// The Activity panel.
    pub activity: Rect,
    /// The unbordered footer line.
    pub footer: Rect,
}

/// The widest a Runs panel's body gets, in lines.
const RUNS_MAX_LINES: usize = 12;
/// The widest a Shells panel's body gets, in lines.
const SHELLS_MAX_LINES: usize = 6;
/// A bordered panel's border rows, top and bottom.
const BORDER_ROWS: usize = 2;
/// The width Budget takes when it shares a row with Runs.
const BUDGET_COLS: usize = 40;
/// The terminal width from which Runs and Budget share one row.
const WIDE_MIN: usize = 120;
/// The rows the header and footer take.
const CHROME_ROWS: usize = 2;

/// A rect inside `area` at `x`,`y` sized `w`×`h`, clamped to lie inside
/// `area` when the numbers would not fit — a too-small terminal clamps
/// instead of panicking. Shared with the detail view's panel rect.
pub(crate) fn rect_or_area(area: Rect, x: u16, y: u16, w: usize, h: usize) -> Rect {
    let w = (w as u16).min(area.width);
    let h = (h as u16).min(area.height);
    let rx = x.min(area.right().saturating_sub(w));
    let ry = y.min(area.bottom().saturating_sub(h));
    Rect {
        x: rx,
        y: ry,
        width: w,
        height: h,
    }
}

/// Lay the live screen out: one `Rect` each for header, runs, budget, shells,
/// activity and footer. Pure, so layout is tested without a terminal.
///
/// `*_lines` are body line counts; a bordered panel is its lines plus 2 for
/// the borders. Runs is capped at 12 body lines, Shells at 6; Activity takes
/// every remaining row and shrinks first. At `area.width >= 120` Runs and
/// Budget share one row, Budget on the right 40 columns wide; below that they
/// stack Runs, Budget, Shells, Activity.
pub fn plan_layout(
    area: Rect,
    runs_lines: usize,
    budget_lines: usize,
    shells_lines: usize,
) -> PanelRects {
    let h = area.height as usize;
    let w = area.width as usize;
    let wide = w >= WIDE_MIN;
    let runs_h = (runs_lines.min(RUNS_MAX_LINES) + BORDER_ROWS).min(h);
    let shells_h = (shells_lines.min(SHELLS_MAX_LINES) + BORDER_ROWS).min(h);
    let budget_h = (budget_lines + BORDER_ROWS).min(h);
    let row_h = if wide { runs_h.max(budget_h) } else { 0 };

    // Chrome first (header, footer), then the fixed panels, then whatever
    // rows are left go to Activity — it shrinks first.
    let inner = h.saturating_sub(CHROME_ROWS);
    let mut y = area.y as usize + 1;
    let header = rect_or_area(area, area.x, area.y, w, 1);

    let (runs, budget) = if wide {
        let run_w = w.saturating_sub(BUDGET_COLS);
        let r = rect_or_area(area, area.x, y as u16, run_w, row_h.min(inner));
        let bx = area.x + run_w as u16;
        let b = rect_or_area(area, bx, y as u16, BUDGET_COLS, row_h.min(inner));
        y += row_h.min(inner);
        (r, b)
    } else {
        let r = rect_or_area(area, area.x, y as u16, w, runs_h.min(inner));
        y += runs_h.min(inner);
        let b = rect_or_area(area, area.x, y as u16, w, budget_h.min(inner));
        y += budget_h.min(inner);
        (r, b)
    };

    let shells = rect_or_area(area, area.x, y as u16, w, shells_h.min(inner));
    y += shells_h.min(inner);

    let bottom = area.y as usize + h;
    let activity_h = bottom.saturating_sub(y).saturating_sub(1).min(inner);
    let activity = rect_or_area(area, area.x, y as u16, w, activity_h);

    let footer = rect_or_area(area, area.x, (bottom - 1) as u16, w, 1);
    PanelRects {
        header,
        runs,
        budget,
        shells,
        activity,
        footer,
    }
}

/// One `--once` frame as plain text: the four sections, each headed by its
/// name on its own line. Pure.
pub fn render_once(snapshot: &TopSnapshot, filter: &str) -> String {
    let activity = collapse_activity(filter_activity(snapshot.activity.clone(), filter));
    format!(
        "Runs\n{}\n\nBudget\n{}\n\nShells\n{}\n\nActivity\n{}",
        runs_text(&snapshot.runs),
        budget_text(&snapshot.budget),
        shells_text(&snapshot.shells),
        activity_text(&activity),
    )
}

/// A shell owner key worth printing: `chat:<16 hex>` to `chat:<8>`,
/// `ghost:<runbook>-<32 hex>` to `ghost:<runbook>`, anything else unchanged.
/// Reuses [`short_session`] for the id part. Pure.
pub fn short_owner(owner: &str) -> String {
    if let Some(id) = owner.strip_prefix("chat:") {
        return format!("chat:{}", short_session(id));
    }
    if let Some(id) = owner.strip_prefix("ghost:") {
        // The owner key already says `ghost:`, so the session id's own
        // `ghost-` marker is redundant and is dropped with its hex tail.
        let short = short_session(id);
        let inner = short.strip_prefix("ghost-").unwrap_or(&short);
        return format!("ghost:{inner}");
    }
    owner.to_string()
}

/// The Shells section as text: one row per live shell — id, owner,
/// foreground and last line — `daemon not running` when the daemon did not
/// answer, `none` when it is up with no shells. Pure. The live screen in
/// `top_tui.rs` draws the same body.
pub(crate) fn shells_text(panel: &ShellsPanel) -> String {
    let rows = match panel {
        ShellsPanel::DaemonDown => return "daemon not running".to_string(),
        ShellsPanel::Rows(rows) => rows,
    };
    if rows.is_empty() {
        return "none".to_string();
    }
    rows.iter()
        .map(|s| {
            format!(
                "{} {} {} {}",
                s.id,
                short_owner(&s.owner),
                s.foreground.as_deref().unwrap_or("-"),
                s.last_line.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}
