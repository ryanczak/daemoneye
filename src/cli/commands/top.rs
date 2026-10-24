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

use crate::cli::commands::top_tui::run_tui;
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
            subject: run_subject(&summary.kind),
            state: state_word(&summary.state),
            turns: summary.turns,
            age_s: (now - started).num_seconds(),
            liveness: liveness(running, mtime, now),
        };
        rows.push((row, started, running));
    }
    sort_rows(rows).into_iter().take(MAX_RUN_ROWS).collect()
}

/// What the run is, from the `RunStarted` kind: `chat:<8>`,
/// `<runbook> ← <trigger>`, `job:<job>` or `<agent> ← <8>`; a `40`-character
/// subject is cut to `32` with `…`; `-` is never a kind, so this is only
/// reached with a real one.
fn run_subject(kind: &crate::daemon::run::RunKind) -> String {
    use crate::daemon::run::RunKind;
    const MAX_SUBJECT: usize = 32;
    let raw = match kind {
        RunKind::Chat { session } => format!("chat:{}", short_session(session)),
        RunKind::Ghost { runbook, trigger } => format!("{runbook} ← {trigger}"),
        RunKind::Scheduled { job } => format!("job:{job}"),
        RunKind::Delegated { agent, parent } => {
            format!("{agent} ← {}", short_session(parent.as_str()))
        }
    };
    if raw.chars().count() > MAX_SUBJECT {
        format!("{}…", raw.chars().take(MAX_SUBJECT).collect::<String>())
    } else {
        raw
    }
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

/// Read the disk panels: the Runs panel, the Budget panel and the Activity
/// rows. No daemon involved.
pub fn disk_panels(config_cap: f64) -> DiskPanels {
    let now = Utc::now();
    let events = read_today_events();
    DiskPanels {
        runs: load_runs(now),
        budget: fold_today(&events, now, config_cap),
        activity: last_activity(&events),
    }
}

/// Ask the daemon for its live PTY shells. One read-only `Request::ShellList`,
/// nothing written. A daemon that answers anything other than a shell list is
/// as good as a daemon that is not running: the panel says so either way.
pub async fn shells_panel() -> ShellsPanel {
    match super::ipc_client::request_once(crate::ipc::Request::ShellList).await {
        Ok(crate::ipc::Response::ShellList { shells }) => ShellsPanel::Rows(shells),
        _ => ShellsPanel::DaemonDown,
    }
}

/// Ask the daemon for the header facts: pid, uptime, provider and model. One
/// read-only `Request::Status`, nothing written. `None` when no daemon answers.
pub async fn daemon_header() -> Option<DaemonHeader> {
    match super::ipc_client::request_once(crate::ipc::Request::Status).await {
        Ok(crate::ipc::Response::DaemonStatus {
            pid,
            uptime_secs,
            provider,
            model,
            ..
        }) => Some(DaemonHeader {
            pid,
            uptime_secs,
            provider,
            model,
        }),
        _ => None,
    }
}

/// Take one frame: the disk panels plus the Shells panel from the daemon.
pub async fn take_snapshot(config_cap: f64) -> TopSnapshot {
    let now = Utc::now();
    let (shells, daemon) = tokio::join!(shells_panel(), daemon_header());
    let DiskPanels {
        runs,
        budget,
        activity,
    } = disk_panels(config_cap);
    TopSnapshot {
        taken_at: now,
        runs,
        budget,
        activity,
        shells,
        daemon,
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
/// instead of panicking.
fn rect_or_area(area: Rect, x: u16, y: u16, w: usize, h: usize) -> Rect {
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

/// `daemoneye top [--once] [--filter <name>]`.
///
/// Read-only: nothing is written, the daemon is asked only to list its
/// shells. `--once` prints one frame and exits 0. Without it, a stdout that
/// is not a terminal is refused with exit 2; otherwise the live ratatui
/// screen runs until `q`.
pub async fn run_top(once: bool, filter: Option<String>) -> Result<()> {
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
    run_tui(cap, filter.unwrap_or_default()).await
}

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
}
