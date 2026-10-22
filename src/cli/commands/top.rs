//! `daemoneye top` — a read-only dashboard over what is on disk (M30
//! phase 07). Three of the four panels, Runs, Budget and Activity, are read
//! straight from the run logs and the event segments; no daemon and no IPC.
//! `top --once` prints one frame as plain text and exits; the live ratatui
//! loop over [`TopFingerprint`] is a later phase.

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde::Serialize;

use crate::daemon::run::{list_runs, read_turn_log};
use crate::daemon::spend::local_day_bounds;
use crate::daemon::utils::for_each_event_between;

/// A run-log file untouched for this long marks its running run stale.
const LIVE_WINDOW: i64 = 30;

/// A `--once` frame holds at most this many run rows.
const MAX_RUN_ROWS: usize = 20;

/// The Activity panel holds the day's last this-many events.
const MAX_ACTIVITY_ROWS: usize = 50;

/// The Budget rate counts `ai_cost` tokens from this window, divided by 60.
const RATE_WINDOW: i64 = 60;

// ── Snapshot types ───────────────────────────────────────────────────────────

/// Whether a run's turn log is still being appended to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Liveness {
    /// A running run whose run-log file was modified in the last 30 s.
    Live,
    /// A running run whose log has gone quiet.
    Stale,
    /// A finished run.
    Done,
}

impl Liveness {
    /// The word the Runs panel prints.
    pub fn label(self) -> &'static str {
        match self {
            Liveness::Live => "live",
            Liveness::Stale => "stale",
            Liveness::Done => "done",
        }
    }
}

/// The liveness rule for one run, pure: the run's state, its log's mtime and
/// `now` decide. A finished run is `Done` however fresh its log is.
pub fn liveness(
    running: bool,
    log_modified: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Liveness {
    match (running, log_modified) {
        (false, _) => Liveness::Done,
        (true, None) => Liveness::Stale,
        (true, Some(m)) if (now - m).num_seconds() <= LIVE_WINDOW => Liveness::Live,
        (true, Some(_)) => Liveness::Stale,
    }
}

/// One row of the Runs panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopRunRow {
    /// The run id, as the run log filename carries it.
    pub id: String,
    /// The kind the log's `RunStarted` recorded.
    pub kind: String,
    /// The state word the run's last record gives.
    pub state: String,
    /// The furthest turn the log reached.
    pub turns: u32,
    /// Seconds since the log's `RunStarted` timestamp.
    pub age_s: i64,
    /// The liveness rule's verdict.
    pub liveness: Liveness,
}

/// One Activity row: a time, the event name and a one-line summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityRow {
    /// The event's `ts`, rendered `HH:MM:SS` in UTC — the same clock the
    /// event log writes.
    pub time: String,
    /// The event name, e.g. `ai_cost` or `budget_refused`.
    pub name: String,
    /// One line naming the event's subject.
    pub summary: String,
}

/// The Budget panel, read from today's events.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BudgetPanel {
    /// Sum of `cost.total_cost_usd` over today's `ai_cost` events, USD.
    pub ai_cost: f64,
    /// The four `tokens` buckets of today's `ai_cost` events, summed.
    pub tokens: u64,
    /// `[budget] max_cost_per_day`; `0` means no cap.
    pub cap: f64,
    /// Count of today's `budget_refused` events.
    pub refused: u32,
    /// Tokens of the `ai_cost` lines whose `ts` falls in the last 60 s,
    /// divided by 60.
    pub tok_per_s: f64,
}

/// The three panels this phase builds, plus when the frame was taken.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TopSnapshot {
    /// When the frame was taken.
    pub taken_at: DateTime<Utc>,
    /// The Runs panel: running runs first, then the rest newest first.
    pub runs: Vec<TopRunRow>,
    /// The Budget panel, from today's events.
    pub budget: BudgetPanel,
    /// The Activity panel: today's last events, oldest first.
    pub activity: Vec<ActivityRow>,
    /// The Shells panel: the daemon's live shells, or the daemon-down mark.
    pub shells: ShellsPanel,
}

/// The Shells panel: one row per live PTY shell, or the mark that the daemon
/// did not answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ShellsPanel {
    /// The daemon answered: its shell rows.
    Rows(Vec<crate::ipc::ShellInfo>),
    /// No daemon answered — the socket is closed or the reply was not a
    /// `ShellList`.
    DaemonDown,
}

/// Sum the four token buckets of one `ai_cost` event value. Pure.
fn event_tokens(value: &serde_json::Value) -> u64 {
    let buckets = [
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
    ];
    buckets
        .iter()
        .map(|b| {
            value
                .get("tokens")
                .and_then(|t| t.get(*b))
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        })
        .sum()
}

/// The Budget panel, folded from today's event values. Pure: takes the
/// values, no filesystem, so the fold is unit-testable without a segment on
/// disk. `now` decides the rate window; `cap` is the config's
/// `max_cost_per_day` verbatim.
fn fold_today(events: &[serde_json::Value], now: DateTime<Utc>, cap: f64) -> BudgetPanel {
    let rate_cutoff = now - chrono::Duration::seconds(RATE_WINDOW);
    let mut budget = BudgetPanel {
        ai_cost: 0.0,
        tokens: 0,
        cap,
        refused: 0,
        tok_per_s: 0.0,
    };
    let mut rate_tokens: u64 = 0;
    for value in events {
        let name = value.get("event").and_then(|v| v.as_str()).unwrap_or("");
        match name {
            "ai_cost" => {
                let cost = value
                    .get("cost")
                    .and_then(|c| c.get("total_cost_usd"))
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0);
                budget.ai_cost += cost;
                let tokens = event_tokens(value);
                budget.tokens += tokens;
                if let Some(ts) = event_ts(value)
                    && ts >= rate_cutoff
                {
                    rate_tokens += tokens;
                }
            }
            "budget_refused" => budget.refused += 1,
            _ => {}
        }
    }
    budget.tok_per_s = rate_tokens as f64 / RATE_WINDOW as f64;
    budget
}

/// Parse an event's `ts`, if it carries one and it parses.
fn event_ts(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    value
        .get("ts")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// One Activity row from an event value, or `None` when the event names no
/// subject worth a line. Pure.
fn activity_row(value: &serde_json::Value) -> Option<ActivityRow> {
    let name = value.get("event").and_then(|v| v.as_str())?;
    let time = event_ts(value)
        .map(|ts| ts.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string());
    let summary = match name {
        "ai_cost" => {
            let agent = value
                .get("agent_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let model = value
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let cost = value
                .get("cost")
                .and_then(|c| c.get("total_cost_usd"))
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            format!("{agent} {model} ${cost:.4}")
        }
        "budget_refused" => {
            let path = value
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let spent = value
                .get("spent_usd")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let cap = value.get("cap_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
            // Amounts without the `$` prefix: the Budget section names the
            // cap with one, and a second `$2.00` line would read as a
            // second cap when a frame is grepped.
            format!("{path} refused: {spent:.2} of cap {cap:.2}")
        }
        "run" => {
            let run_id = value.get("run_id").and_then(|v| v.as_str()).unwrap_or("-");
            let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("-");
            format!("{run_id} {status}")
        }
        _ => generic_summary(value),
    };
    Some(ActivityRow {
        time,
        name: name.to_string(),
        summary,
    })
}

/// Bounded generic summary for events with no dedicated arm: the remaining
/// fields as `key=value` pairs, truncated to MAX chars. Never empty.
fn generic_summary(value: &serde_json::Value) -> String {
    const MAX: usize = 80;
    let mut summary = String::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if matches!(k.as_str(), "ts" | "event" | "pid") {
                continue;
            }
            let rendered = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if !summary.is_empty() {
                summary.push(' ');
            }
            summary.push_str(k);
            summary.push('=');
            summary.push_str(&rendered);
            if summary.chars().count() >= MAX {
                break;
            }
        }
    }
    if summary.is_empty() {
        summary.push_str("(no fields)");
    }
    if summary.chars().count() > MAX {
        summary = summary.chars().take(MAX).collect();
    }
    summary
}

/// Keep the activity rows whose event name contains `needle`. An empty
/// needle keeps everything. Pure.
pub fn filter_activity(events: Vec<ActivityRow>, needle: &str) -> Vec<ActivityRow> {
    if needle.is_empty() {
        return events;
    }
    events
        .into_iter()
        .filter(|row| row.name.contains(needle))
        .collect()
}

/// The last [`MAX_ACTIVITY_ROWS`] events, oldest first. Pure: takes the
/// values in log order, returns the kept rows.
fn last_activity(events: &[serde_json::Value]) -> Vec<ActivityRow> {
    let mut rows: Vec<ActivityRow> = events.iter().filter_map(activity_row).collect();
    let start = rows.len().saturating_sub(MAX_ACTIVITY_ROWS);
    rows.split_off(start)
}

// ── Loaders ──────────────────────────────────────────────────────────────────

/// Build the Runs panel from [`list_runs`]: up to 20 rows, running first,
/// then the rest newest first. The start instant and the log's mtime come
/// from the run log itself, so the row's `age_s` and `liveness` are read
/// where `list_runs` sorted from.
fn load_runs(now: DateTime<Utc>) -> Vec<TopRunRow> {
    let summaries = list_runs();
    let mut rows: Vec<(TopRunRow, DateTime<Utc>, bool)> = Vec::new();
    for summary in &summaries {
        let Some(started) = run_started_at(&summary.id) else {
            continue;
        };
        let path = crate::daemon::run::turn_log_file(&summary.id);
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        let running = !summary.state.is_terminal();
        let row = TopRunRow {
            id: summary.id.as_str().to_string(),
            kind: summary.kind.label().to_string(),
            state: state_word(&summary.state),
            turns: summary.turns,
            age_s: (now - started).num_seconds(),
            liveness: liveness(running, mtime, now),
        };
        rows.push((row, started, running));
    }
    sort_rows(rows).into_iter().take(MAX_RUN_ROWS).collect()
}

/// The state word the Runs panel prints for a summary, matching `run list`'s
/// table (`src/daemon/run/list.rs`).
fn state_word(state: &crate::daemon::run::RunState) -> String {
    use crate::daemon::run::RunState;
    match state {
        RunState::Running => "running".to_string(),
        RunState::Complete => "complete".to_string(),
        RunState::HardFail(_) => "hard-fail".to_string(),
        RunState::BudgetExceeded(_) => "budget-exceeded".to_string(),
        RunState::Cancelled { .. } => "cancelled".to_string(),
    }
}

/// The instant the run log's `RunStarted` carries, or `None` when it cannot
/// be read or parsed.
fn run_started_at(id: &crate::daemon::run::RunId) -> Option<DateTime<Utc>> {
    let records = read_turn_log(id);
    let ts = records.iter().find_map(|r| match &r.event {
        crate::daemon::run::TurnLogEvent::RunStarted { .. } => Some(r.timestamp.clone()),
        _ => None,
    })?;
    DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Running runs first, then the rest newest first. Pure: takes the rows with
/// their start instants and running flags, returns the order.
fn sort_rows(mut rows: Vec<(TopRunRow, DateTime<Utc>, bool)>) -> Vec<TopRunRow> {
    rows.sort_by(|(_a, a_started, a_running), (_b, b_started, b_running)| {
        match (a_running, b_running) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => b_started.cmp(a_started),
            (false, false) => b_started.cmp(a_started),
        }
    });
    rows.into_iter().map(|(row, _, _)| row).collect()
}

/// Read today's event values for the Budget and Activity panels. Reads the
/// segments in the local-day window, like every other disk reader.
fn read_today_events() -> Vec<serde_json::Value> {
    let (from, to) = local_day_bounds(Local::now());
    let mut events = Vec::new();
    for_each_event_between(Some(from), Some(to), &mut |value| {
        events.push(value.clone());
    });
    events
}

/// Take one frame: the Runs panel, the Budget panel and the Activity rows
/// from disk, plus the config's cap — and the Shells panel from the daemon.
/// One `Request::ShellList`, nothing written.
pub async fn take_snapshot(config_cap: f64) -> TopSnapshot {
    let now = Utc::now();
    let events = read_today_events();
    let budget = fold_today(&events, now, config_cap);
    let shells = match super::ipc_client::request_once(crate::ipc::Request::ShellList).await {
        Ok(crate::ipc::Response::ShellList { shells }) => ShellsPanel::Rows(shells),
        // Any other reply is as good as no daemon: the panel says so either way.
        _ => ShellsPanel::DaemonDown,
    };
    TopSnapshot {
        taken_at: now,
        runs: load_runs(now),
        budget,
        activity: last_activity(&events),
        shells,
    }
}

// ── Fingerprint ──────────────────────────────────────────────────────────────

/// The stats-only fingerprint the live loop re-reads on: the runs directory's
/// entry count and newest mtime, today's segment's length and mtime. Reads
/// no file contents, so it costs nothing to poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TopFingerprint {
    /// Entries under `var/log/runs/`.
    pub runs_entries: u64,
    /// The newest entry's mtime, in seconds since the epoch; 0 when none.
    pub runs_newest_mtime_s: u64,
    /// Today's segment file's length in bytes; 0 when there is none.
    pub segment_len: u64,
    /// Today's segment file's mtime, in seconds since the epoch; 0 when none.
    pub segment_mtime_s: u64,
}

/// A segment file's length and mtime, or zeros when there is none.
fn file_stats(path: &std::path::Path) -> (u64, u64) {
    match std::fs::metadata(path) {
        Ok(meta) => (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

/// Read the runs directory's stats only: how many entries it holds and the
/// newest entry's mtime. Never opens a file.
fn runs_dir_stats() -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(crate::config::runs_dir()) else {
        return (0, 0);
    };
    let mut count: u64 = 0;
    let mut newest: u64 = 0;
    for entry in entries.flatten() {
        count += 1;
        if let Ok(meta) = entry.metadata()
            && let Ok(m) = meta.modified()
            && let Ok(d) = m.duration_since(std::time::UNIX_EPOCH)
        {
            newest = newest.max(d.as_secs());
        }
    }
    (count, newest)
}

/// Read the fingerprint from disk: stats only, never file contents.
pub fn fingerprint() -> TopFingerprint {
    let (runs_entries, runs_newest_mtime_s) = runs_dir_stats();
    let (segment_len, segment_mtime_s) = file_stats(&crate::config::current_event_segment_path());
    TopFingerprint {
        runs_entries,
        runs_newest_mtime_s,
        segment_len,
        segment_mtime_s,
    }
}

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
    lines.push(format!("rate: {:.1} tok/s", budget.tok_per_s));
    lines.push(format!("refused today: {}", budget.refused));
    lines.join("\n")
}

/// The Runs section's text. Pure.
pub fn runs_text(runs: &[TopRunRow]) -> String {
    if runs.is_empty() {
        return "none".to_string();
    }
    let id_w = runs.iter().map(|r| r.id.len()).max().unwrap_or(0).max(2);
    let kind_w = runs.iter().map(|r| r.kind.len()).max().unwrap_or(0).max(4);
    let state_w = runs.iter().map(|r| r.state.len()).max().unwrap_or(0).max(5);
    let live_w = runs
        .iter()
        .map(|r| r.liveness.label().len())
        .max()
        .unwrap_or(0)
        .max(8);
    let mut out = format!(
        "{:>id_w$}  {:<kind_w$}  {:<state_w$}  {:>5}  {:>8}  {:<live_w$}\n",
        "ID", "KIND", "STATE", "TURNS", "AGE", "LIVENESS"
    );
    for run in runs {
        out.push_str(&format!(
            "{:>id_w$}  {:<kind_w$}  {:<state_w$}  {:>5}  {:>8}  {:<live_w$}\n",
            run.id,
            run.kind,
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
fn format_age(secs: i64) -> String {
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

/// One `--once` frame as plain text: the four sections, each headed by its
/// name on its own line. Pure.
pub fn render_once(snapshot: &TopSnapshot, filter: &str) -> String {
    let activity = filter_activity(snapshot.activity.clone(), filter);
    format!(
        "Runs\n{}\n\nBudget\n{}\n\nShells\n{}\n\nActivity\n{}",
        runs_text(&snapshot.runs),
        budget_text(&snapshot.budget),
        shells_text(&snapshot.shells),
        activity_text(&activity),
    )
}

/// The Shells section as text: one row per live shell — id, owner,
/// foreground and last line — `daemon not running` when the daemon did not
/// answer, `none` when it is up with no shells. Pure.
fn shells_text(panel: &ShellsPanel) -> String {
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
                s.owner,
                s.foreground.as_deref().unwrap_or("-"),
                s.last_line.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// `daemoneye top [--once] [--filter <name>]`.
///
/// Read-only: nothing is written, the daemon is asked only to list its
/// shells. `--once` prints one frame and exits 0; without it the live view
/// is a later release and the command exits 2 naming `--once`.
pub async fn run_top(once: bool, filter: Option<String>) -> Result<()> {
    if !once {
        eprintln!("daemoneye top: the live view arrives in a later release; use --once");
        std::process::exit(2);
    }
    let needle = filter.unwrap_or_default();
    let cap = crate::config::Config::load()
        .map(|c| c.budget.max_cost_per_day)
        .unwrap_or(0.0);
    let snapshot = take_snapshot(cap).await;
    println!("{}", render_once(&snapshot, &needle));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── runs ordering ────────────────────────────────────────────────────────

    fn row(id: &str, running: bool) -> (TopRunRow, DateTime<Utc>, bool) {
        (
            TopRunRow {
                id: id.to_string(),
                kind: "ghost".to_string(),
                state: if running { "running" } else { "complete" }.to_string(),
                turns: 2,
                age_s: 10,
                liveness: if running {
                    Liveness::Live
                } else {
                    Liveness::Done
                },
            },
            Utc::now(),
            running,
        )
    }

    #[test]
    fn runs_put_running_first_then_newest() {
        let t = |offset_s: i64| Utc::now() - chrono::Duration::seconds(offset_s);
        let rows = vec![
            row("bbbb", false), // complete, older
            row("aaaa", false), // complete, newest of the finished
            row("cccc", true),  // running, oldest
            row("dddd", true),  // running, newest
        ];
        let mut rows = rows;
        // Give each row a distinct start instant: c older than d, b older than a.
        rows[0].1 = t(900);
        rows[1].1 = t(600);
        rows[2].1 = t(300);
        rows[3].1 = t(100);

        let ordered = sort_rows(rows);
        let ids: Vec<&str> = ordered.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["dddd", "cccc", "aaaa", "bbbb"],
            "running first (newest first within the group), then finished newest first"
        );
    }

    #[test]
    fn liveness_is_live_stale_or_done() {
        let now = Utc::now();
        let fresh = now - chrono::Duration::seconds(10);
        let old = now - chrono::Duration::seconds(600);
        assert_eq!(liveness(true, Some(fresh), now), Liveness::Live);
        assert_eq!(liveness(true, Some(old), now), Liveness::Stale);
        assert_eq!(liveness(false, Some(fresh), now), Liveness::Done);
        assert_eq!(liveness(false, None, now), Liveness::Done);
        assert_eq!(
            liveness(
                true,
                Some(now - chrono::Duration::seconds(LIVE_WINDOW)),
                now
            ),
            Liveness::Live,
            "exactly 30 s old is still live"
        );
        assert_eq!(
            liveness(
                true,
                Some(now - chrono::Duration::seconds(LIVE_WINDOW + 1)),
                now
            ),
            Liveness::Stale,
            "31 s old is stale"
        );
        assert_eq!(liveness(true, None, now), Liveness::Stale);
    }

    // ── budget fold ─────────────────────────────────────────────────────────

    fn ai_cost(ts: &str, cost: f64, tokens: u64) -> serde_json::Value {
        serde_json::json!({
            "ts": ts,
            "event": "ai_cost",
            "pid": 1,
            "agent_name": "chat",
            "model": "m",
            "tokens": {
                "input_tokens": tokens,
                "output_tokens": 0,
                "cache_read_tokens": 0,
                "cache_write_tokens": 0
            },
            "cost": {"total_cost_usd": cost}
        })
    }

    fn refused(ts: &str) -> serde_json::Value {
        serde_json::json!({
            "ts": ts,
            "event": "budget_refused",
            "pid": 1,
            "path": "chat",
            "spent_usd": 2.0,
            "cap_usd": 2.0
        })
    }

    #[test]
    fn budget_sums_todays_ai_cost_and_counts_refusals() {
        let now = Utc::now();
        let events = vec![
            ai_cost("2026-10-21T09:00:00Z", 0.25, 100),
            ai_cost("2026-10-21T09:05:00Z", 0.25, 150),
            refused("2026-10-21T09:06:00Z"),
            ai_cost("2026-10-21T09:07:00Z", 0.5, 200),
        ];
        let budget = fold_today(&events, now, 2.0);
        assert!(
            (budget.ai_cost - 1.0).abs() < 1e-9,
            "spend sums: {budget:?}"
        );
        assert_eq!(
            budget.tokens, 450,
            "tokens sum all four buckets: {budget:?}"
        );
        assert_eq!(budget.cap, 2.0);
        assert_eq!(budget.refused, 1, "exactly one refusal counted: {budget:?}");
    }

    #[test]
    fn tok_per_s_counts_only_the_last_minute() {
        let now = Utc::now();
        let in_window = now - chrono::Duration::seconds(30);
        let out_of_window = now - chrono::Duration::seconds(120);
        let events = vec![
            ai_cost(&in_window.to_rfc3339(), 0.1, 600),
            ai_cost(&out_of_window.to_rfc3339(), 0.1, 600),
        ];
        let budget = fold_today(&events, now, 0.0);
        assert!(
            (budget.tok_per_s - 10.0).abs() < 1e-6,
            "600 tokens in the window over 60 s is 10 tok/s: {budget:?}"
        );
    }

    // ── activity filter ─────────────────────────────────────────────────────

    #[test]
    fn filter_activity_matches_on_the_event_name() {
        let rows = vec![
            ActivityRow {
                time: "09:00:00".to_string(),
                name: "ai_cost".to_string(),
                summary: "chat m $0.2500".to_string(),
            },
            ActivityRow {
                time: "09:01:00".to_string(),
                name: "budget_refused".to_string(),
                summary: "chat refused: 2.00 of cap 2.00".to_string(),
            },
        ];
        let filtered = filter_activity(rows.clone(), "ai_cost");
        assert_eq!(
            filtered.len(),
            1,
            "ai_cost keeps only the ai_cost line: {filtered:?}"
        );
        assert_eq!(filtered[0].name, "ai_cost");
        let kept = filter_activity(rows, "");
        assert_eq!(kept.len(), 2, "an empty needle keeps everything: {kept:?}");
    }

    #[test]
    fn activity_keeps_every_event_kind() {
        let base = "2026-10-22T09:00:00Z";
        let mk = |event: &str, extra: serde_json::Value| {
            let mut obj = serde_json::Map::new();
            obj.insert("ts".to_string(), base.into());
            obj.insert("event".to_string(), event.into());
            obj.insert("pid".to_string(), 1.into());
            if let Some(extra) = extra.as_object() {
                for (k, v) in extra {
                    obj.insert(k.clone(), v.clone());
                }
            }
            serde_json::Value::Object(obj)
        };
        let events = [
            mk(
                "ai_cost",
                serde_json::json!({ "agent_name": "chat", "cost": { "total_cost_usd": 0.25 } }),
            ),
            mk(
                "budget_refused",
                serde_json::json!({ "path": "chat", "spent_usd": 2.0, "cap_usd": 2.0 }),
            ),
            mk("ghost_started", serde_json::json!({ "job_id": "j1" })),
            mk("webhook_alert", serde_json::json!({ "alert_name": "disk" })),
            mk("bare", serde_json::json!({})),
        ];
        let rows: Vec<ActivityRow> = events.iter().filter_map(activity_row).collect();
        assert_eq!(
            rows.len(),
            5,
            "every named event lands in the panel: {rows:?}"
        );
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"ghost_started") && names.contains(&"webhook_alert"),
            "kinds with no summary arm are kept: {names:?}"
        );
        assert!(
            rows.iter().all(|r| !r.summary.trim().is_empty()),
            "no summary is empty: {rows:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.name != "ai_cost" && r.summary.contains('$')),
            "generic summaries name no dollar amount: {rows:?}"
        );
        let ghost = &rows[names.iter().position(|n| *n == "ghost_started").unwrap()];
        assert_eq!(
            ghost.summary, "job_id=j1",
            "generic summary is key=value: {rows:?}"
        );
        assert_eq!(
            rows[names.iter().position(|n| *n == "bare").unwrap()].summary,
            "(no fields)",
            "an event with no fields gets a placeholder: {rows:?}"
        );
    }

    // ── fingerprint ─────────────────────────────────────────────────────────

    #[test]
    fn fingerprint_changes_when_the_segment_grows() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        std::fs::create_dir_all(crate::config::events_dir()).unwrap();
        std::fs::create_dir_all(crate::config::runs_dir()).unwrap();

        let before = fingerprint();
        let seg = crate::config::current_event_segment_path();
        std::fs::write(
            &seg,
            "{\"event\":\"test\",\"ts\":\"2026-10-21T00:00:00Z\"}\n",
        )
        .unwrap();

        let after = fingerprint();
        assert!(
            after.segment_len > before.segment_len,
            "a grown segment must raise segment_len: before {before:?} after {after:?}"
        );
        assert!(
            after.segment_mtime_s >= before.segment_mtime_s,
            "the mtime never goes back: {after:?}"
        );
    }

    // ── text frame ──────────────────────────────────────────────────────────

    #[test]
    fn budget_text_names_spend_cap_and_percent() {
        let budget = BudgetPanel {
            ai_cost: 0.5,
            tokens: 450,
            cap: 2.0,
            refused: 1,
            tok_per_s: 12.5,
        };
        let text = budget_text(&budget);
        assert!(text.contains("spend: $0.50"), "spend named: {text}");
        assert!(text.contains("$2.00"), "cap named: {text}");
        assert!(text.contains("25%"), "percentage named: {text}");
        assert!(text.contains("refused today: 1"), "refusals named: {text}");
    }

    #[test]
    fn budget_text_names_no_cap_when_the_cap_is_off() {
        let budget = BudgetPanel {
            ai_cost: 0.5,
            tokens: 450,
            cap: 0.0,
            refused: 0,
            tok_per_s: 0.0,
        };
        let text = budget_text(&budget);
        assert!(text.contains("no cap"), "cap-off wording: {text}");
        assert!(!text.contains("%"), "no percentage without a cap: {text}");
    }

    #[test]
    fn empty_sections_say_none() {
        assert_eq!(runs_text(&[]), "none");
        assert_eq!(activity_text(&[]), "none");
        assert_eq!(
            shells_text(&ShellsPanel::Rows(Vec::new())),
            "none",
            "an up daemon with no shells says none"
        );
    }

    #[test]
    fn shells_text_says_daemon_not_running() {
        assert_eq!(
            shells_text(&ShellsPanel::DaemonDown),
            "daemon not running",
            "the down mark is the whole section"
        );
    }

    #[test]
    fn shells_text_lists_each_shell() {
        let mut row_with_facts = crate::ipc::ShellInfo {
            id: "s1".to_string(),
            host: "local".to_string(),
            owner: "chat:deadbeef".to_string(),
            started_unix: 1_788_470_000,
            dedicated: false,
            exited_unix: None,
            last_command: Some("echo hi".to_string()),
            foreground: Some("running cargo test".to_string()),
            last_line: Some("100% done".to_string()),
        };
        let row_without = crate::ipc::ShellInfo {
            id: "s2".to_string(),
            foreground: None,
            last_line: None,
            ..row_with_facts.clone()
        };
        row_with_facts.id = "s1".to_string();
        let text = shells_text(&ShellsPanel::Rows(vec![row_with_facts, row_without]));
        assert!(
            text.contains("s1 chat:deadbeef running cargo test 100% done"),
            "the shell's facts on its row: {text}"
        );
        assert!(
            text.contains("s2 chat:deadbeef - -"),
            "missing facts print as -: {text}"
        );
    }
}
