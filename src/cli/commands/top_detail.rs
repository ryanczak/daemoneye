//! The run detail view: `load_run_detail` and `run_detail_text`.

use super::*;

// ── Run detail ───────────────────────────────────────────────────────────────

/// The turn-log records a detail view shows: the log's last this-many,
/// oldest first.
const DETAIL_LOG_TAIL: usize = 200;

/// What [`load_run_detail`] read for one run: its `run` record, when the run
/// has finished and left one, and its turn log, oldest first.
pub struct RunDetail {
    /// The run's `run` record from the dated events segments; `None` when
    /// the run has not finished (or left no record).
    pub record: Option<crate::daemon::run::RunRecord>,
    /// The run's turn log, oldest first.
    pub log: Vec<crate::daemon::run::TurnLogRecord>,
}

/// Read one run's turn log and its `run` record, searching the event
/// segments from the run's start instant to now. `None` when the run has no
/// turn log on disk — or one that names no start. The disk reads go through
/// the phase-02a log reader and the phase-03 record reader; nothing new is
/// parsed here.
pub fn load_run_detail(id: &crate::daemon::run::RunId) -> Option<RunDetail> {
    let log = read_turn_log(id);
    if log.is_empty() {
        return None;
    }
    let start = started_at_of(&log)?;
    let record = super::scorecard::run_records_between(Some(start), Some(Utc::now()))
        .into_iter()
        .rfind(|r| r.run_id == id.as_str());
    Some(RunDetail { record, log })
}

/// The instant the records' first `RunStarted` carries, or `None`. Pure, so
/// `load_run_detail` and the row builder read one log, not two.
fn started_at_of(records: &[crate::daemon::run::TurnLogRecord]) -> Option<DateTime<Utc>> {
    let ts = records.iter().find_map(|r| match &r.event {
        crate::daemon::run::TurnLogEvent::RunStarted { .. } => Some(r.timestamp.clone()),
        _ => None,
    })?;
    DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// The Runs row one run's detail heading is built from: kind, subject, state
/// and age from its turn log, the same words the Runs panel prints. `None`
/// when the log names no kind or start — the rule `list_runs` applies.
pub(crate) fn detail_row(
    id: &crate::daemon::run::RunId,
    log: &[crate::daemon::run::TurnLogRecord],
    now: DateTime<Utc>,
) -> Option<TopRunRow> {
    let summary = crate::daemon::run::summarize(id, log)?;
    let started = started_at_of(log)?;
    let mtime = std::fs::metadata(crate::daemon::run::turn_log_file(id))
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    let running = !summary.state.is_terminal();
    Some(TopRunRow {
        id: id.as_str().to_string(),
        kind: summary.kind.label().to_string(),
        subject: run_subject(&summary.kind),
        state: state_word(&summary.state),
        turns: summary.turns,
        age_s: (now - started).num_seconds(),
        liveness: liveness(running, mtime, now),
    })
}

/// One `RunFinished` record's state, or `None` when the log shows no finish.
fn finished_state(
    log: &[crate::daemon::run::TurnLogRecord],
) -> Option<&crate::daemon::run::RunState> {
    log.iter().rev().find_map(|r| match &r.event {
        crate::daemon::run::TurnLogEvent::RunFinished { state } => Some(state),
        _ => None,
    })
}

/// A turn-log record's `HH:MM:SS` in `offset`, `--:--:--` when the stamp
/// does not parse. Pure.
fn log_time(ts: &str, offset: FixedOffset) -> String {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&offset).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

/// One turn-log record as the detail's tail shows it: the time in `offset`,
/// then the event — `started <kind label>`, `turn <n>`, `note <text>` or
/// `finished <state word>`. A note's later lines follow it indented by two
/// spaces, and every note is masked. Pure.
fn log_line(rec: &crate::daemon::run::TurnLogRecord, offset: FixedOffset) -> String {
    use crate::daemon::run::TurnLogEvent;
    let time = log_time(&rec.timestamp, offset);
    match &rec.event {
        TurnLogEvent::RunStarted { kind } => format!("{time} started {}", kind.label()),
        TurnLogEvent::TurnStarted { turn } => format!("{time} turn {turn}"),
        TurnLogEvent::Note { text } => {
            let masked = crate::ai::filter::mask_sensitive(text);
            let mut lines = masked.lines();
            let first = lines.next().unwrap_or_default();
            let mut out = format!("{time} note {first}");
            for line in lines {
                out.push_str(&format!("\n  {line}"));
            }
            out
        }
        TurnLogEvent::RunFinished { state } => {
            format!("{time} finished {}", state_word(state))
        }
    }
}

/// The state word the log tail prints for a `RunFinished` — the Runs panel's
/// words, on the log's own state.
/// A run's detail text: the heading (the Runs row's fields), the `run`
/// record's fields when there is one or the running state, how the run
/// ended, and the turn log's tail. Pure: takes the row, the record, the log
/// and the zone the log times render in.
pub fn run_detail_text_in(
    row: &TopRunRow,
    record: Option<&crate::daemon::run::RunRecord>,
    log: &[crate::daemon::run::TurnLogRecord],
    offset: FixedOffset,
) -> String {
    let mut lines = vec![format!(
        "{}  {}  {}  {}  {}",
        row.id,
        row.kind,
        row.subject,
        row.state,
        format_age(row.age_s)
    )];
    match record {
        Some(r) => {
            lines.push(format!("status: {}", r.status.as_str()));
            let failure = if r.is_model_attributable {
                r.failure_class.as_str().to_string()
            } else {
                format!("{} (not the model)", r.failure_class.as_str())
            };
            lines.push(format!("failure: {failure}"));
            let model = match (&r.provider, &r.model) {
                (Some(p), Some(m)) => format!("{p}/{m}"),
                _ => "-".to_string(),
            };
            lines.push(format!("model: {model}"));
            lines.push(format!("turns: {}", r.turns));
            let rate = match r.tool_success_rate {
                Some(v) => format!("{:.1}% ok", v * 100.0),
                None => "-".to_string(),
            };
            lines.push(format!("tool calls: {} · {rate}", r.tool_calls));
            let parse = match r.parse_failure_rate {
                Some(v) => format!("{:.1}%", v * 100.0),
                None => "-".to_string(),
            };
            lines.push(format!("parse failures: {parse}"));
            lines.push(format!(
                "tokens: {} in · {} out · {} cache read · {} cache write",
                r.tokens.input_tokens,
                r.tokens.output_tokens,
                r.tokens.cache_read_tokens,
                r.tokens.cache_write_tokens
            ));
            lines.push(format!("cost: ${:.4}", r.cost_usd));
            lines.push(format!("wall clock: {}", format_age(r.wall_clock_s as i64)));
        }
        None => {
            lines.push("no run record yet".to_string());
            let turns = log
                .iter()
                .filter_map(|r| match &r.event {
                    crate::daemon::run::TurnLogEvent::TurnStarted { turn } => Some(*turn),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            lines.push(format!("turns: {turns}"));
        }
    }
    match finished_state(log) {
        Some(crate::daemon::run::RunState::HardFail(b))
        | Some(crate::daemon::run::RunState::BudgetExceeded(b)) => {
            if !b.goal.is_empty() {
                lines.push(format!("goal: {}", b.goal));
            }
            if let Some(blocker) = &b.blocker
                && !blocker.is_empty()
            {
                lines.push(format!("blocker: {blocker}"));
            }
            if let Some(dim) = &b.budget_left {
                lines.push(format!("budget left: {}", dim.label()));
            }
        }
        Some(crate::daemon::run::RunState::Cancelled { stage, turns }) => {
            lines.push(format!("stage: {stage}"));
            lines.push(format!("turns: {turns}"));
        }
        _ => {}
    }
    lines.push("turn log".to_string());
    let start = log.len().saturating_sub(DETAIL_LOG_TAIL);
    for rec in &log[start..] {
        lines.push(log_line(rec, offset));
    }
    lines.join("\n")
}

/// The detail text in the process's local zone — what `top --once --run`
/// prints and the live screen draws.
pub fn run_detail_text(
    row: &TopRunRow,
    record: Option<&crate::daemon::run::RunRecord>,
    log: &[crate::daemon::run::TurnLogRecord],
) -> String {
    run_detail_text_in(row, record, log, *Local::now().offset())
}
