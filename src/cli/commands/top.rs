//! `daemoneye top` — a read-only dashboard over what is on disk (M30
//! phase 07). Three of the four panels, Runs, Budget and Activity, are read
//! straight from the run logs and the event segments; no daemon and no IPC.
//! `top --once` prints one frame as plain text; the live ratatui loop over
//! [`TopFingerprint`] is in `top_tui.rs`.

use std::io::IsTerminal;

use anyhow::Result;
use chrono::{DateTime, FixedOffset, Local, Utc};
use ratatui::layout::Rect;
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
    /// What the run is: the kind's subject — `chat:<8>`, `<runbook> ←
    /// <trigger>`, `job:<job>` or `<agent> ← <8>`; `-` when the log names
    /// no kind.
    pub subject: String,
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
    /// The event's `ts`, rendered `HH:MM:SS` in the viewer's local zone —
    /// the same clock every other panel prints.
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

/// The daemon facts the header names, from a read-only `Request::Status`.
/// `None` when no daemon answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonHeader {
    /// The daemon's process id.
    pub pid: u32,
    /// Seconds the daemon has been up.
    pub uptime_secs: u64,
    /// The provider the daemon's active model runs on.
    pub provider: String,
    /// The daemon's active model.
    pub model: String,
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
    /// The daemon facts the header names; `None` when the daemon is down.
    pub daemon: Option<DaemonHeader>,
}

/// The disk-read panels this phase builds: Runs, Budget and Activity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiskPanels {
    /// The Runs panel: running runs first, then the rest newest first.
    pub runs: Vec<TopRunRow>,
    /// The Budget panel, from today's events.
    pub budget: BudgetPanel,
    /// The Activity panel: today's last events, oldest first.
    pub activity: Vec<ActivityRow>,
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

/// The number of hex characters a chat session id carries — 8 random bytes,
/// the same span `RunId` and the ghost ids cut to.
const CHAT_SESSION_ID_LEN: usize = 16;

/// The number of hex characters a ghost session id carries after its
/// `ghost-<runbook>-` prefix.
const GHOST_SESSION_HEX_LEN: usize = 32;

/// A session id worth printing: a 16-hex chat id to its first 8, a
/// `ghost-<runbook>-<32 hex>` id to `ghost-<runbook>`, anything else (a
/// name, a `-`, a shorter id) unchanged. Pure.
pub fn short_session(id: &str) -> String {
    let hex = |n: usize, s: &str| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    if hex(CHAT_SESSION_ID_LEN, id) {
        return id.chars().take(8).collect();
    }
    if let Some(rest) = id.strip_prefix("ghost-")
        && let Some((runbook, tail)) = rest.rsplit_once('-')
        && hex(GHOST_SESSION_HEX_LEN, tail)
    {
        return format!("ghost-{runbook}");
    }
    id.to_string()
}

/// A command cut to at most this many characters, `…` when cut.
fn short_cmd(cmd: &str) -> String {
    const MAX: usize = 40;
    if cmd.chars().count() > MAX {
        format!("{}…", cmd.chars().take(MAX).collect::<String>())
    } else {
        cmd.to_string()
    }
}

/// The first field named `key` as a string, `fallback` when absent or not a
/// string. Reads a top-level field.
fn str_field(value: &serde_json::Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

/// The first field named `key` as a number, `fallback` when absent. Reads a
/// top-level field.
fn num_field(value: &serde_json::Value, key: &str, fallback: i64) -> i64 {
    value.get(key).and_then(|v| v.as_i64()).unwrap_or(fallback)
}

/// One Activity row from an event value, or `None` when the event names no
/// subject worth a line. Pure. [`activity_row`] is the same row with the
/// process's local zone.
fn activity_row_in(value: &serde_json::Value, offset: chrono::FixedOffset) -> Option<ActivityRow> {
    let name = value.get("event").and_then(|v| v.as_str())?;
    let time = event_ts(value)
        .map(|ts| ts.with_timezone(&offset).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string());
    let summary = match name {
        "ai_cost" => {
            let agent = str_field(value, "agent_name", "unknown");
            let model = str_field(value, "model", "unknown");
            let cost = value
                .get("cost")
                .and_then(|c| c.get("total_cost_usd"))
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            format!("{agent} {model} ${cost:.4}")
        }
        "budget_refused" => {
            let path = str_field(value, "path", "unknown");
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
            let run_id = str_field(value, "run_id", "-");
            let kind = str_field(value, "kind", "-");
            let status = str_field(value, "status", "-");
            format!("{run_id} {kind} {status}")
        }
        "daemon_start" => {
            let version = str_field(value, "version", "-");
            let socket = str_field(value, "socket", "-");
            let file = std::path::Path::new(&socket)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&socket);
            format!("v{version} socket {file}")
        }
        "daemon_stop" => str_field(value, "reason", "-"),
        "ai_turn" => {
            let session = short_session(&str_field(value, "session", "-"));
            let prompt = num_field(value, "prompt_tokens", 0);
            let completion = num_field(value, "completion_tokens", 0);
            format!("{session} {prompt}+{completion} tok")
        }
        "command_approval" => {
            let session = short_session(&str_field(value, "session", "-"));
            let decision = str_field(value, "decision", "-");
            let cmd = short_cmd(&str_field(value, "cmd", "-"));
            format!("{session} {decision} {cmd}")
        }
        "command" => {
            let session = short_session(&str_field(value, "session", "-"));
            let shell = str_field(value, "shell", "-");
            let cmd = short_cmd(&str_field(value, "cmd", "-"));
            // `exit -` when the field is absent: a tmux-era line carried no
            // exit code.
            let exit = match value.get("exit") {
                Some(v) => v.to_string(),
                None => "-".to_string(),
            };
            format!("{session} {shell} {cmd} exit {exit}")
        }
        "webhook_alert" => {
            let alert = str_field(value, "alert_name", "-");
            let severity = str_field(value, "severity", "-");
            let status = str_field(value, "status", "-");
            format!("{alert} {severity} {status}")
        }
        "webhook_analysis" => {
            let alert = str_field(value, "alert_name", "-");
            let ghost = value
                .get("ghost_trigger")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let runbook = str_field(value, "runbook", "-");
            format!(
                "{alert} ghost={} {runbook}",
                if ghost { "yes" } else { "no" }
            )
        }
        "ghost_start" => {
            let alert = str_field(value, "alert_name", "-");
            let session = short_session(&str_field(value, "session_id", "-"));
            let depth = num_field(value, "spawn_depth", 0);
            format!("{alert} {session} depth {depth}")
        }
        "ghost_turn" => {
            let session = short_session(&str_field(value, "session_id", "-"));
            let turn = num_field(value, "turn", 0);
            let tools: Vec<String> = value
                .get("tools")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                        .map(|n| n.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let joined = if tools.is_empty() {
                "no tools".to_string()
            } else {
                tools.join(", ")
            };
            format!("{session} turn {turn}: {joined}")
        }
        "ghost_complete" => {
            let session = short_session(&str_field(value, "session_id", "-"));
            let turns = num_field(value, "turns_used", 0);
            format!("{session} {turns} turns")
        }
        "ghost_shells_closed" => {
            let session = short_session(&str_field(value, "session", "-"));
            let shells: Vec<String> = value
                .get("shells")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            format!("{session} {}", shells.join(", "))
        }
        "ghost_lifecycle" => {
            let content = str_field(value, "content", "");
            // The session log path rides after an em-dash separator; the
            // panel wants the words, not where they are filed.
            content
                .split_once(" — ")
                .map(|(head, _)| head.trim().to_string())
                .unwrap_or_else(|| content.trim().to_string())
        }
        "job_scheduled" => {
            let job_name = str_field(value, "job_name", "-");
            let kind = str_field(value, "kind", "-");
            format!("{job_name} {kind}")
        }
        "memory_retrieved" => {
            let session = short_session(&str_field(value, "session_id", "-"));
            let n = value
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{session} {n} keys")
        }
        "cache_refresh_error" => str_field(value, "error", "-"),
        _ => generic_summary(value),
    };
    Some(ActivityRow {
        time,
        name: name.to_string(),
        summary,
    })
}

/// One Activity row from an event value, in the process's local zone.
fn activity_row(value: &serde_json::Value) -> Option<ActivityRow> {
    activity_row_in(value, *Local::now().offset())
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

/// Merge each run of *consecutive* rows with the same event name and
/// summary into one row that keeps the last time and appends ` ×N` (N > 1;
/// a single row keeps its summary unchanged). Non-adjacent duplicates are
/// not merged. Pure. Runs after the filter, so the filter still sees every
/// row.
pub fn collapse_activity(rows: Vec<ActivityRow>) -> Vec<ActivityRow> {
    let mut out: Vec<ActivityRow> = Vec::new();
    let mut run_len: usize = 0;
    for row in rows {
        let same = run_len > 0
            && out
                .last()
                .is_some_and(|last| last.name == row.name && last.summary == row.summary);
        if same {
            if let Some(last) = out.last_mut() {
                last.time = row.time;
            }
            run_len += 1;
        } else {
            if run_len > 1
                && let Some(last) = out.last_mut()
            {
                last.summary.push_str(&format!(" ×{run_len}"));
            }
            out.push(row);
            run_len = 1;
        }
    }
    if run_len > 1
        && let Some(last) = out.last_mut()
    {
        last.summary.push_str(&format!(" ×{run_len}"));
    }
    out
}

#[path = "top_load.rs"]
mod load;
pub use load::*;
#[path = "top_text.rs"]
mod text_frame;
pub use text_frame::*;
// The children reach these sibling commands as `super::…`, through this import.
use super::{ipc_client, scorecard};

/// `daemoneye top [--once] [--filter <name>] [--run <id>]`.
///
/// Read-only: nothing is written, the daemon is asked only to list its
/// shells. `--once` prints one frame and exits 0. `--run <id>` prints that
/// run's detail (`--once`) or opens the live screen on it; an id with no
/// turn log is refused with exit 1. Without `--once`, a stdout that is not a
/// terminal is refused with exit 2; otherwise the live ratatui screen runs
/// until `q`.
pub async fn run_top(once: bool, filter: Option<String>, run: Option<String>) -> Result<()> {
    if let Some(id) = run {
        let run_id = match crate::daemon::run::RunId::parse(&id) {
            Ok(id) => id,
            Err(_) => {
                eprintln!("run {id} not found");
                std::process::exit(1);
            }
        };
        let Some(detail) = load_run_detail(&run_id) else {
            eprintln!("run {id} not found");
            std::process::exit(1);
        };
        let Some(row) = detail_row(&run_id, &detail.log, Utc::now()) else {
            eprintln!("run {id} not found");
            std::process::exit(1);
        };
        if once {
            println!(
                "{}",
                run_detail_text(&row, detail.record.as_ref(), &detail.log)
            );
            return Ok(());
        }
        if !std::io::stdout().is_terminal() {
            eprintln!("daemoneye top: not a terminal; use --once");
            std::process::exit(2);
        }
        let cap = crate::config::Config::load()
            .map(|c| c.budget.max_cost_per_day)
            .unwrap_or(0.0);
        return crate::cli::commands::top_tui::run_tui(
            cap,
            filter.unwrap_or_default(),
            Some(run_id),
        )
        .await;
    }
    if once {
        let needle = filter.unwrap_or_default();
        let cap = crate::config::Config::load()
            .map(|c| c.budget.max_cost_per_day)
            .unwrap_or(0.0);
        let snapshot = take_snapshot(cap).await;
        println!("{}", render_once(&snapshot, &needle));
        return Ok(());
    }
    if !std::io::stdout().is_terminal() {
        eprintln!("daemoneye top: not a terminal; use --once");
        std::process::exit(2);
    }
    let cap = crate::config::Config::load()
        .map(|c| c.budget.max_cost_per_day)
        .unwrap_or(0.0);
    crate::cli::commands::top_tui::run_tui(cap, filter.unwrap_or_default(), None).await
}

#[path = "top_detail.rs"]
mod detail;
pub use detail::*;

#[cfg(test)]
mod tests {
    use super::*;

    // ── header_text ──────────────────────────────────────────────────────────

    #[test]
    fn header_names_pid_uptime_model_host_and_time() {
        let daemon = DaemonHeader {
            pid: 4242,
            uptime_secs: 3725,
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
        };
        let now = "2026-10-22T09:00:00+05:00"
            .parse::<DateTime<FixedOffset>>()
            .expect("a fixed time");
        let text = header_text(Some(&daemon), "pinky", now);
        assert!(
            text.contains("daemoneye · pid 4242"),
            "the pid is named: {text}"
        );
        assert!(text.contains("up 1h 2m 5s"), "the uptime is named: {text}");
        assert!(
            text.contains("anthropic/claude-opus-4-6"),
            "provider and model are named: {text}"
        );
        assert!(text.contains("pinky"), "the host is named: {text}");
        assert!(text.contains("09:00:00"), "the local time is named: {text}");
    }

    #[test]
    fn header_names_the_daemon_down() {
        let now = "2026-10-22T09:00:00+00:00"
            .parse::<DateTime<FixedOffset>>()
            .expect("a fixed time");
        let text = header_text(None, "pinky", now);
        assert_eq!(
            text, "daemoneye · daemon not running · pinky · 09:00:00",
            "a down daemon names itself: {text}"
        );
    }

    // ── plan_layout ──────────────────────────────────────────────────────────

    #[test]
    fn layout_puts_runs_and_budget_side_by_side_when_wide() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let r = plan_layout(area, 20, 6, 8);
        assert_eq!(r.runs.y, r.budget.y, "the panels share a row");
        assert_eq!(
            r.runs.x + r.runs.width,
            r.budget.x,
            "Runs is left of Budget with no gap"
        );
        assert_eq!(r.budget.width, 40, "Budget takes 40 columns on the right");
        assert_eq!(r.runs.width, 80, "Runs takes the rest of the row");
        assert!(r.runs.right() <= area.width, "Runs stays inside the area");
        assert!(
            r.budget.right() <= area.width,
            "Budget stays inside the area"
        );
    }

    #[test]
    fn layout_stacks_panels_when_narrow() {
        let area = ratatui::layout::Rect::new(0, 0, 119, 40);
        let r = plan_layout(area, 20, 6, 8);
        assert_eq!(
            r.budget.y,
            r.runs.y + r.runs.height,
            "Budget sits directly below Runs"
        );
        assert_eq!(
            r.shells.y,
            r.budget.y + r.budget.height,
            "Shells sits directly below Budget"
        );
        assert_eq!(
            r.activity.y,
            r.shells.y + r.shells.height,
            "Activity sits directly below Shells"
        );
        assert_eq!(r.runs.width, 119, "each panel takes the full width");
    }

    #[test]
    fn layout_sizes_runs_and_shells_to_content_up_to_a_cap() {
        let area = ratatui::layout::Rect::new(0, 0, 100, 60);
        let small = plan_layout(area, 3, 4, 3);
        assert_eq!(small.runs.height, 5, "3 body lines plus 2 borders");
        assert_eq!(small.shells.height, 5, "3 body lines plus 2 borders");
        let big = plan_layout(area, 40, 4, 40);
        assert_eq!(big.runs.height, 14, "12 body lines plus 2 borders");
        assert_eq!(big.shells.height, 8, "6 body lines plus 2 borders");
    }

    #[test]
    fn layout_gives_activity_the_rest() {
        let area = ratatui::layout::Rect::new(0, 0, 80, 40);
        let r = plan_layout(area, 10, 4, 4);
        let used =
            1 + r.runs.height as usize + r.budget.height as usize + r.shells.height as usize + 1;
        assert_eq!(
            r.activity.height as usize,
            area.height as usize - used,
            "Activity takes every row the other rects leave"
        );
        assert_eq!(
            r.activity.bottom(),
            r.footer.y,
            "Activity ends at the footer"
        );
        assert_eq!(r.footer.y, 39, "the footer is the last row");
    }

    #[test]
    fn layout_survives_a_tiny_terminal() {
        for (w, h) in [(40u16, 10u16), (1u16, 1u16)] {
            let area = ratatui::layout::Rect::new(0, 0, w, h);
            let r = plan_layout(area, 20, 6, 8);
            for (name, rect) in [
                ("header", r.header),
                ("runs", r.runs),
                ("budget", r.budget),
                ("shells", r.shells),
                ("activity", r.activity),
                ("footer", r.footer),
            ] {
                assert!(
                    rect.x >= area.x
                        && rect.y >= area.y
                        && rect.right() <= area.right()
                        && rect.bottom() <= area.bottom(),
                    "{name} lies inside {w}x{h}: {rect:?}"
                );
            }
        }
    }

    // ── budget_bar ───────────────────────────────────────────────────────────

    #[test]
    fn budget_bar_levels_at_80_and_100_percent() {
        let (_, level) = budget_bar(79.9, 100.0, 20).expect("a bar");
        assert_eq!(level, BarLevel::Ok, "79.9% is under the warn mark");
        let (_, level) = budget_bar(80.0, 100.0, 20).expect("a bar");
        assert_eq!(level, BarLevel::Warn, "80% is the warn mark");
        let (_, level) = budget_bar(100.0, 100.0, 20).expect("a bar");
        assert_eq!(level, BarLevel::Over, "100% is over");
        let (text, _) = budget_bar(150.0, 100.0, 20).expect("a bar");
        assert!(
            text.contains("150%"),
            "the percentage is printed as it is: {text}"
        );
        assert_eq!(
            text.matches('█').count(),
            20,
            "an over-cap spend fills the bar: {text}"
        );
        assert!(budget_bar(50.0, 0.0, 20).is_none(), "no cap, no bar");
    }

    // ── run_fades ────────────────────────────────────────────────────────────

    #[test]
    fn finished_runs_fade_after_five_minutes() {
        let mut r = row("dddd", false).0;
        r.age_s = 299;
        assert!(!run_fades(&r), "a fresh finished run is not dimmed");
        r.age_s = 300;
        assert!(run_fades(&r), "a finished run dims at five minutes");
        r.liveness = Liveness::Live;
        r.age_s = 3600;
        assert!(!run_fades(&r), "a live run never dims");
        r.liveness = Liveness::Stale;
        assert!(!run_fades(&r), "a stale run never dims");
    }

    // ── runs ordering ────────────────────────────────────────────────────────

    fn row(id: &str, running: bool) -> (TopRunRow, DateTime<Utc>, bool) {
        (
            TopRunRow {
                id: id.to_string(),
                kind: "ghost".to_string(),
                subject: "disk-full-l4 ← webhook".to_string(),
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

    // ── subject ─────────────────────────────────────────────────────────────

    fn kind_of(variant: &str) -> crate::daemon::run::RunKind {
        use crate::daemon::run::RunKind;
        match variant {
            "chat" => RunKind::Chat {
                session: "5175faf67d501ce8".into(),
            },
            "ghost" => RunKind::Ghost {
                runbook: "disk-full-l4".into(),
                trigger: "webhook".into(),
            },
            "scheduled" => RunKind::Scheduled {
                job: "nightly".into(),
            },
            "delegated" => RunKind::Delegated {
                agent: "triage".into(),
                parent: crate::daemon::run::RunId::parse("7eb9151d45fa49f1")
                    .unwrap_or_else(|_| unreachable!()),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn subject_names_each_run_kind() {
        let chat = run_subject(&kind_of("chat"));
        assert_eq!(
            chat, "chat:5175faf6",
            "chat subject is chat:<first 8>: {chat}"
        );
        let ghost = run_subject(&kind_of("ghost"));
        assert_eq!(
            ghost, "disk-full-l4 ← webhook",
            "ghost subject is runbook ← trigger: {ghost}"
        );
        let scheduled = run_subject(&kind_of("scheduled"));
        assert_eq!(
            scheduled, "job:nightly",
            "scheduled subject is job:<job>: {scheduled}"
        );
        let delegated = run_subject(&kind_of("delegated"));
        assert_eq!(
            delegated, "triage ← 7eb9151d",
            "delegated subject is agent ← <first 8 of parent>: {delegated}"
        );
        let long = run_subject(&crate::daemon::run::RunKind::Ghost {
            runbook: "a".repeat(40),
            trigger: "webhook".into(),
        });
        assert_eq!(
            long.chars().count(),
            32 + 1,
            "a 40-character runbook is cut to 32 with a trailing …: {long}"
        );
        assert!(long.ends_with('…'), "the cut subject ends with …: {long}");
    }

    #[test]
    fn runs_text_has_a_subject_column() {
        let rows = vec![TopRunRow {
            id: "7eb9151d45fa49f1".to_string(),
            kind: "ghost".to_string(),
            subject: "disk-full-l4 ← webhook".to_string(),
            state: "running".to_string(),
            turns: 2,
            age_s: 10,
            liveness: Liveness::Live,
        }];
        let text = runs_text(&rows);
        let header = text.lines().next().unwrap_or_default().to_string();
        assert!(
            header.contains("KIND") && header.contains("SUBJECT") && header.contains("STATE"),
            "the header holds SUBJECT between KIND and STATE: {header}"
        );
        let kind_pos = header.find("KIND").unwrap_or_default();
        let subject_pos = header.find("SUBJECT").unwrap_or_default();
        let state_pos = header.find("STATE").unwrap_or_default();
        assert!(
            kind_pos < subject_pos && subject_pos < state_pos,
            "SUBJECT sits between KIND and STATE: {header}"
        );
        let ghost = text.lines().nth(1).unwrap_or_default().to_string();
        assert!(
            ghost.contains("disk-full-l4 ← webhook"),
            "a ghost row's line holds its subject: {ghost}"
        );
    }

    // ── local time ──────────────────────────────────────────────────────────

    #[test]
    fn activity_time_is_rendered_in_the_given_zone() {
        let value = serde_json::json!({
            "ts": "2026-10-22T14:36:20Z",
            "event": "daemon_stop",
            "reason": "SIGTERM",
        });
        let west = activity_row_in(&value, chrono::FixedOffset::west_opt(5 * 3600).unwrap())
            .unwrap_or_else(|| unreachable!());
        assert_eq!(
            west.time, "09:36:20",
            "west offset 5 h turns 14:36:20Z into 09:36:20: {west:?}"
        );
        let utc = activity_row_in(&value, chrono::FixedOffset::east_opt(0).unwrap())
            .unwrap_or_else(|| unreachable!());
        assert_eq!(
            utc.time, "14:36:20",
            "offset 0 keeps the UTC wall time: {utc:?}"
        );
    }

    // ── short_session ───────────────────────────────────────────────────────

    #[test]
    fn short_session_trims_chat_and_ghost_ids() {
        let hex16 = "5175faf67d501ce8";
        assert_eq!(
            short_session(hex16),
            "5175faf6",
            "a 16-hex chat id is cut to its first 8"
        );
        let hex32 = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
        assert_eq!(
            short_session(&format!("ghost-disk-full-l4-{hex32}")),
            "ghost-disk-full-l4",
            "a ghost session id keeps its runbook and drops the hex tail"
        );
        assert_eq!(short_session("matt"), "matt", "a name is unchanged");
        assert_eq!(
            short_session("5175faf"),
            "5175faf",
            "a 7-hex string is not a chat id and is unchanged"
        );
    }

    // ── shaped summaries ────────────────────────────────────────────────────

    #[test]
    fn shaped_summaries_for_daemon_events() {
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
            (
                "daemon_start",
                mk(
                    "daemon_start",
                    serde_json::json!({ "version": "2.2.0", "socket": "/home/m/.daemoneye/var/run/daemoneye.sock" }),
                ),
                "v2.2.0 socket daemoneye.sock",
            ),
            (
                "daemon_stop",
                mk("daemon_stop", serde_json::json!({ "reason": "SIGTERM" })),
                "SIGTERM",
            ),
            (
                "ai_turn",
                mk(
                    "ai_turn",
                    serde_json::json!({ "session": "5175faf67d501ce8", "prompt_tokens": 120, "completion_tokens": 34 }),
                ),
                "5175faf6 120+34 tok",
            ),
            (
                "command_approval",
                mk(
                    "command_approval",
                    serde_json::json!({ "session": "5175faf67d501ce8", "decision": "approved", "cmd": "hostname" }),
                ),
                "5175faf6 approved hostname",
            ),
            (
                "command",
                mk(
                    "command",
                    serde_json::json!({ "session": "5175faf67d501ce8", "shell": "s0", "cmd": "hostname", "exit": 0 }),
                ),
                "5175faf6 s0 hostname exit 0",
            ),
            (
                "command",
                mk(
                    "command",
                    serde_json::json!({ "session": "5175faf67d501ce8", "shell": "s0", "cmd": "hostname" }),
                ),
                "5175faf6 s0 hostname exit -",
            ),
            (
                "webhook_alert",
                mk(
                    "webhook_alert",
                    serde_json::json!({ "alert_name": "disk-full-l4", "severity": "critical", "status": "firing" }),
                ),
                "disk-full-l4 critical firing",
            ),
            (
                "webhook_analysis",
                mk(
                    "webhook_analysis",
                    serde_json::json!({ "alert_name": "disk-full-l4", "ghost_trigger": true, "runbook": "disk-full-l4" }),
                ),
                "disk-full-l4 ghost=yes disk-full-l4",
            ),
            (
                "ghost_start",
                mk(
                    "ghost_start",
                    serde_json::json!({ "alert_name": "disk-full-l4", "session_id": "ghost-disk-full-l4-a1b2c3d4e5f60718293a4b5c6d7e8f90", "spawn_depth": 2 }),
                ),
                "disk-full-l4 ghost-disk-full-l4 depth 2",
            ),
            (
                "ghost_turn",
                mk(
                    "ghost_turn",
                    serde_json::json!({ "session_id": "ghost-disk-full-l4-a1b2c3d4e5f60718293a4b5c6d7e8f90", "turn": 3, "tools": [ { "name": "run_terminal_command" }, { "name": "read_shell" } ] }),
                ),
                "ghost-disk-full-l4 turn 3: run_terminal_command, read_shell",
            ),
            (
                "ghost_turn",
                mk(
                    "ghost_turn",
                    serde_json::json!({ "session_id": "ghost-disk-full-l4-a1b2c3d4e5f60718293a4b5c6d7e8f90", "turn": 3, "tools": [] }),
                ),
                "ghost-disk-full-l4 turn 3: no tools",
            ),
            (
                "ghost_complete",
                mk(
                    "ghost_complete",
                    serde_json::json!({ "session_id": "ghost-disk-full-l4-a1b2c3d4e5f60718293a4b5c6d7e8f90", "turns_used": 7 }),
                ),
                "ghost-disk-full-l4 7 turns",
            ),
            (
                "ghost_shells_closed",
                mk(
                    "ghost_shells_closed",
                    serde_json::json!({ "session": "ghost-disk-full-l4-a1b2c3d4e5f60718293a4b5c6d7e8f90", "shells": ["s0", "s1"] }),
                ),
                "ghost-disk-full-l4 s0, s1",
            ),
            (
                "ghost_lifecycle",
                mk(
                    "ghost_lifecycle",
                    serde_json::json!({ "content": "Ghost completed: disk below 90% — /home/m/.daemoneye/var/log/sessions/x.jsonl" }),
                ),
                "Ghost completed: disk below 90%",
            ),
            (
                "job_scheduled",
                mk(
                    "job_scheduled",
                    serde_json::json!({ "job_name": "nightly", "kind": "cron: 0 3 * * *" }),
                ),
                "nightly cron: 0 3 * * *",
            ),
            (
                "memory_retrieved",
                mk(
                    "memory_retrieved",
                    serde_json::json!({ "session_id": "5175faf67d501ce8", "keys": ["a", "b", "c"] }),
                ),
                "5175faf6 3 keys",
            ),
            (
                "cache_refresh_error",
                mk(
                    "cache_refresh_error",
                    serde_json::json!({ "error": "Failed to get active pane for session 'm30'" }),
                ),
                "Failed to get active pane for session 'm30'",
            ),
            (
                "run",
                mk(
                    "run",
                    serde_json::json!({ "run_id": "7eb9151d45fa49f1", "kind": "ghost", "status": "complete" }),
                ),
                "7eb9151d45fa49f1 ghost complete",
            ),
            (
                "ghost_started",
                mk("ghost_started", serde_json::json!({ "job_id": "j1" })),
                "job_id=j1",
            ),
        ];
        for (name, value, want) in events {
            let row = activity_row(&value).unwrap_or_else(|| unreachable!());
            assert_eq!(
                row.summary, want,
                "the {name} arm must produce its exact summary: {row:?}"
            );
        }
    }

    // ── collapse ────────────────────────────────────────────────────────────

    #[test]
    fn collapse_merges_only_consecutive_identical_rows() {
        let mk = |time: &str| ActivityRow {
            time: time.to_string(),
            name: "cache_refresh_error".to_string(),
            summary: "Failed to get active pane for session 'm30'".to_string(),
        };
        let other = ActivityRow {
            time: "09:00:03".to_string(),
            name: "ai_cost".to_string(),
            summary: "chat m $0.2500".to_string(),
        };
        // five identical, one different, two identical
        let rows = vec![
            mk("09:00:00"),
            mk("09:00:01"),
            mk("09:00:02"),
            mk("09:00:03"),
            mk("09:00:04"),
            other.clone(),
            mk("09:00:05"),
            mk("09:00:06"),
        ];
        let out = collapse_activity(rows);
        assert_eq!(
            out.len(),
            3,
            "two runs of identical rows collapse to one row each: {out:?}"
        );
        assert_eq!(
            out[0].summary, "Failed to get active pane for session 'm30' ×5",
            "the first run of five is marked ×5: {out:?}"
        );
        assert_eq!(
            out[0].time, "09:00:04",
            "the merged row keeps the last time of its run: {out:?}"
        );
        assert_eq!(
            out[1].summary, "chat m $0.2500",
            "a row with no run partner keeps its summary unchanged: {out:?}"
        );
        assert_eq!(
            out[2].summary, "Failed to get active pane for session 'm30' ×2",
            "the second run of two is marked ×2: {out:?}"
        );
        // two identical separated by a different row stay three rows
        let separated = vec![mk("09:00:00"), other.clone(), mk("09:00:02")];
        let out = collapse_activity(separated);
        assert_eq!(
            out.len(),
            3,
            "non-adjacent duplicates are not merged: {out:?}"
        );
        assert!(
            out.iter().all(|r| !r.summary.contains('×')),
            "no merged marker on a run of one: {out:?}"
        );
    }

    // ── short_owner ─────────────────────────────────────────────────────────

    #[test]
    fn short_owner_trims_ghost_and_chat_owners() {
        assert_eq!(
            short_owner("chat:5175faf67d501ce8"),
            "chat:5175faf6",
            "a chat owner key is cut to 8 hex"
        );
        assert_eq!(
            short_owner("ghost:ghost-disk-full-l4-a1b2c3d4e5f60718293a4b5c6d7e8f90"),
            "ghost:disk-full-l4",
            "a ghost owner key drops the hex tail of its session id"
        );
        assert_eq!(
            short_owner("user:matt"),
            "user:matt",
            "any other owner key is unchanged"
        );
    }

    // ── budget rate label ───────────────────────────────────────────────────

    #[test]
    fn budget_text_names_the_rate_window() {
        let budget = BudgetPanel {
            ai_cost: 0.25,
            cap: 2.0,
            tokens: 600,
            tok_per_s: 10.0,
            refused: 1,
        };
        let text = budget_text(&budget);
        let rate = text
            .lines()
            .find(|l| l.starts_with("rate"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            rate, "rate (60 s): 10.0 tok/s",
            "the rate line names its 60-second window: {text}"
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

    // ── run detail text ──────────────────────────────────────────────────────

    use crate::daemon::run::{
        Briefing, BudgetDimension, RunKind, RunState, TurnLogEvent, TurnLogRecord,
    };

    fn detail_run_id() -> crate::daemon::run::RunId {
        crate::daemon::run::RunId::parse("7eb9151d45fa49f1").expect("a valid fixture id")
    }

    fn utc_offset() -> FixedOffset {
        FixedOffset::east_opt(0).expect("utc")
    }

    fn detail_row(state: &str) -> TopRunRow {
        TopRunRow {
            id: "7eb9151d45fa49f1".to_string(),
            kind: "ghost".to_string(),
            subject: "disk-full-l4 ← webhook".to_string(),
            state: state.to_string(),
            turns: 7,
            age_s: 120,
            liveness: Liveness::Done,
        }
    }

    fn detail_record() -> crate::daemon::run::RunRecord {
        crate::daemon::run::RunRecord {
            run_id: "7eb9151d45fa49f1".to_string(),
            kind: "ghost".to_string(),
            agent: None,
            provider: Some("ollama".to_string()),
            model: Some("glm-test".to_string()),
            status: crate::daemon::run::RunStatus::HardFail,
            failure_class: crate::daemon::run::FailureClass::Stall,
            is_model_attributable: true,
            turns: 7,
            tool_calls: 12,
            tool_success_rate: Some(0.9166),
            parse_failure_rate: Some(0.0),
            tokens: crate::ai::types::TokenBreakdown {
                input_tokens: 1000,
                output_tokens: 234,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            cost_usd: 0.0123,
            wall_clock_s: 83.0,
            tags: Default::default(),
        }
    }

    fn detail_log(ts: &str, event: TurnLogEvent) -> TurnLogRecord {
        TurnLogRecord {
            timestamp: ts.to_string(),
            run_id: detail_run_id(),
            event,
        }
    }

    #[test]
    fn detail_text_names_the_record_fields() {
        let row = detail_row("hard-fail");
        let record = detail_record();
        let text = run_detail_text_in(&row, Some(&record), &[], utc_offset());
        let heading = text.lines().next().expect("a heading line");
        assert_eq!(
            heading, "7eb9151d45fa49f1  ghost  disk-full-l4 ← webhook  hard-fail  2m",
            "the heading is the Runs row's fields: {text}"
        );
        for line in [
            "status: hard_fail",
            "failure: stall",
            "model: ollama/glm-test",
            "turns: 7",
            "tool calls: 12 · 91.7% ok",
            "parse failures: 0.0%",
            "tokens: 1000 in · 234 out · 0 cache read · 0 cache write",
            "cost: $0.0123",
            "wall clock: 1m",
        ] {
            assert!(text.contains(line), "missing {line:?} in:\n{text}");
        }
        assert!(
            !text.contains("no run record yet"),
            "a recorded run never says it: {text}"
        );
    }

    #[test]
    fn detail_text_names_a_non_attributable_failure() {
        let row = detail_row("hard-fail");
        let mut record = detail_record();
        record.failure_class = crate::daemon::run::FailureClass::BackendError;
        record.is_model_attributable = false;
        let text = run_detail_text_in(&row, Some(&record), &[], utc_offset());
        assert!(
            text.contains("failure: backend_error (not the model)"),
            "a non-attributable failure is marked: {text}"
        );
    }

    #[test]
    fn detail_text_without_tool_calls_prints_dashes() {
        let row = detail_row("complete");
        let mut record = detail_record();
        record.tool_calls = 0;
        record.tool_success_rate = None;
        record.parse_failure_rate = None;
        let text = run_detail_text_in(&row, Some(&record), &[], utc_offset());
        assert!(
            text.contains("tool calls: 0 · -"),
            "the dash stands in for the rate: {text}"
        );
        assert!(
            text.contains("parse failures: -"),
            "the dash stands in for the parse rate: {text}"
        );
        assert!(
            !text.contains("NaN"),
            "a None rate never prints NaN: {text}"
        );
    }

    #[test]
    fn detail_text_for_a_running_run_has_no_record() {
        let row = detail_row("running");
        let log = vec![
            detail_log(
                "2026-01-01T14:00:00Z",
                TurnLogEvent::RunStarted {
                    kind: RunKind::Ghost {
                        runbook: "disk-full-l4".into(),
                        trigger: "webhook".into(),
                    },
                },
            ),
            detail_log(
                "2026-01-01T14:00:10Z",
                TurnLogEvent::TurnStarted { turn: 1 },
            ),
            detail_log(
                "2026-01-01T14:00:20Z",
                TurnLogEvent::TurnStarted { turn: 2 },
            ),
            detail_log(
                "2026-01-01T14:00:30Z",
                TurnLogEvent::Note {
                    text: "checking df".into(),
                },
            ),
        ];
        let text = run_detail_text_in(&row, None, &log, utc_offset());
        assert!(
            text.contains("no run record yet"),
            "a running run says so: {text}"
        );
        assert!(
            text.contains("turns: 2"),
            "the turns the log shows are named: {text}"
        );
        assert!(
            !text.contains("status:"),
            "no record, no record fields: {text}"
        );
        assert!(!text.contains("cost:"), "no record, no cost: {text}");
        assert!(
            text.contains("checking df"),
            "the log is still shown: {text}"
        );
    }

    #[test]
    fn detail_text_names_the_briefing_and_the_cancel() {
        let row = detail_row("hard-fail");
        let hard = detail_log(
            "2026-01-01T14:00:00Z",
            TurnLogEvent::RunFinished {
                state: RunState::HardFail(Briefing::new(
                    "free disk on /var",
                    None,
                    vec![],
                    vec![],
                    Some("df still at 99%".into()),
                    None,
                )),
            },
        );
        let text = run_detail_text_in(&row, None, &[hard], utc_offset());
        assert!(text.contains("goal: free disk on /var"), "goal: {text}");
        assert!(text.contains("blocker: df still at 99%"), "blocker: {text}");
        assert!(
            !text.contains("budget left"),
            "no dimension, no budget-left line: {text}"
        );

        let budget = detail_log(
            "2026-01-01T14:00:00Z",
            TurnLogEvent::RunFinished {
                state: RunState::BudgetExceeded(Briefing::new(
                    "sweep the alerts",
                    None,
                    vec![],
                    vec![],
                    None,
                    Some(BudgetDimension::Turns),
                )),
            },
        );
        let text = run_detail_text_in(&row, None, &[budget], utc_offset());
        assert!(
            text.contains("budget left: turns"),
            "the dimension is named: {text}"
        );
        assert!(
            !text.contains("blocker:"),
            "no blocker, no blocker line: {text}"
        );

        let row = detail_row("cancelled");
        let cancel = detail_log(
            "2026-01-01T14:00:00Z",
            TurnLogEvent::RunFinished {
                state: RunState::Cancelled {
                    stage: "awaiting_model".into(),
                    turns: 3,
                },
            },
        );
        let text = run_detail_text_in(&row, None, &[cancel], utc_offset());
        assert!(text.contains("stage: awaiting_model"), "stage: {text}");
        assert!(text.contains("turns: 3"), "the cancel's turns: {text}");
    }

    #[test]
    fn detail_tail_keeps_the_last_200_records() {
        let row = detail_row("running");
        let log: Vec<TurnLogRecord> = (1..=300)
            .map(|i| {
                detail_log(
                    "2026-01-01T14:00:00Z",
                    TurnLogEvent::Note {
                        text: format!("rec-{i:03}"),
                    },
                )
            })
            .collect();
        let text = run_detail_text_in(&row, None, &log, utc_offset());
        assert!(
            !text.contains("rec-001") && !text.contains("rec-100"),
            "the first 100 records fall off the tail: {text}"
        );
        assert!(
            text.contains("rec-101"),
            "record 101 is the first the tail shows: {text}"
        );
        assert!(text.contains("rec-300"), "record 300 is the last: {text}");
        let first = text.find("rec-101").expect("101 present");
        let last = text.find("rec-300").expect("300 present");
        assert!(first < last, "the tail is oldest first");
    }

    #[test]
    fn detail_masks_notes_and_indents_their_lines() {
        let row = detail_row("running");
        let log = vec![detail_log(
            "2026-01-01T14:00:00Z",
            TurnLogEvent::Note {
                text: "found key AKIAIOSFODNN7EXAMPLE in env\nsecond line of the note".into(),
            },
        )];
        let text = run_detail_text_in(&row, None, &log, utc_offset());
        assert!(text.contains("<AWS_KEY"), "the key is masked: {text}");
        assert!(!text.contains("AKIA"), "no key leaks: {text}");
        let second = text
            .lines()
            .find(|l| l.contains("second line"))
            .expect("the note's second line");
        assert!(
            second.starts_with("  "),
            "the note's later line is indented two spaces: {second:?}"
        );
    }

    #[test]
    fn detail_times_use_the_given_zone() {
        let row = detail_row("running");
        let log = vec![detail_log(
            "2026-10-25T14:36:20Z",
            TurnLogEvent::RunStarted {
                kind: RunKind::Ghost {
                    runbook: "disk-full-l4".into(),
                    trigger: "webhook".into(),
                },
            },
        )];
        let offset = FixedOffset::west_opt(5 * 3600).expect("a west offset");
        let text = run_detail_text_in(&row, None, &log, offset);
        assert!(
            text.contains("09:36:20 started ghost"),
            "the time is rendered in the given zone: {text}"
        );
        assert!(
            !text.contains("14:36:20"),
            "the UTC clock does not leak: {text}"
        );
    }
}
