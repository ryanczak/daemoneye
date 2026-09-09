//! Read-only projections over a run's turn log (M24 phase 02b): the last N
//! records, a case-insensitive substring search, and one turn's slice. These
//! back the `run_log_tail` / `run_log_search` / `get_turn` AI tools. They only
//! read what phase-02a wrote — same grammar, no new file formats.
//!
//! Limits are clamped **inside** these functions, so a future caller cannot
//! bypass the cap by passing an oversized value straight down. `get_turn` is
//! deliberately uncapped: its caller has already narrowed to a single turn
//! (design § 2.3, "clamped limits and one uncapped escape hatch").

use super::turn_log::{TurnLogEvent, TurnLogRecord, read_turn_log};
use super::{RunId, RunKind, RunState};

/// The largest number of records a clamped reader may return, whatever the
/// model asked for. A tool result that silently truncates is a diagnostic, so
/// the cap stays well above what one turn's observation actually needs.
pub const MAX_RUN_LOG_LIMIT: u64 = 200;

/// The limit a clamped reader applies when the caller omits one.
pub const DEFAULT_RUN_LOG_LIMIT: u64 = 50;

/// Fold the requested limit into `[1, MAX_RUN_LOG_LIMIT]`; `None` (or `0`,
/// nonsense for "show me the last N") takes the default.
pub fn clamp_run_log_limit(limit: Option<u64>) -> u64 {
    match limit {
        None | Some(0) => DEFAULT_RUN_LOG_LIMIT,
        Some(n) => n.min(MAX_RUN_LOG_LIMIT),
    }
}

/// Whether the requested limit was reduced, phrased so the tool result can
/// quote it — the model has to be able to tell a truncated answer from a
/// complete one.
fn clamp_note(requested: Option<u64>) -> Option<String> {
    match requested {
        Some(r) if r > MAX_RUN_LOG_LIMIT => Some(format!(
            "requested limit {r} exceeded the maximum of {MAX_RUN_LOG_LIMIT} and was clamped"
        )),
        _ => None,
    }
}

/// The one-line header naming the run, the total record count, and what the
/// reader is showing.
fn header(run_id: &RunId, total: usize, shown: usize, note: Option<&str>) -> String {
    let suffix = note.map(|n| format!(" ({n})")).unwrap_or_default();
    format!("Run {run_id} turn log: showing {shown} of {total} record(s){suffix}")
}

/// Render one record as a single line for a tool result.
fn render_record(rec: &TurnLogRecord) -> String {
    let event = match &rec.event {
        TurnLogEvent::RunStarted { kind } => match kind {
            RunKind::Chat { session } => format!("run_started(chat session={session})"),
            RunKind::Ghost { runbook, trigger } => {
                format!("run_started(ghost runbook={runbook} trigger={trigger})")
            }
            RunKind::Scheduled { job } => format!("run_started(scheduled job={job})"),
            RunKind::Delegated { agent, parent } => {
                format!("run_started(delegated agent={agent} parent={parent})")
            }
        },
        TurnLogEvent::TurnStarted { turn } => format!("turn_started(turn={turn})"),
        TurnLogEvent::Note { text } => format!("note: {text}"),
        TurnLogEvent::RunFinished { state } => {
            format!("run_finished(state={})", state_label(state))
        }
    };
    format!("[{}] {}", rec.timestamp, event)
}

fn state_label(state: &RunState) -> &'static str {
    match state {
        RunState::Running => "running",
        RunState::Complete => "complete",
        RunState::HardFail(_) => "hard_fail",
        RunState::BudgetExceeded(_) => "budget_exceeded",
        RunState::Cancelled { .. } => "cancelled",
    }
}

/// The text a record contributes to a search haystack: the event fields a
/// human could grep for (kind names, turn numbers, note text, result states),
/// plus the timestamp, minus the run id which the caller already pinned.
fn search_text(rec: &TurnLogRecord) -> String {
    format!("{} {}", rec.timestamp, render_record(rec))
}

/// The last `limit` records of a run, oldest-first in the returned slice.
/// The result is the rendered block the tool reports back: a one-line header
/// naming the run and how many records it shows — and, when the request was
/// clamped, saying so — then the records.
pub fn run_log_tail(run_id: &RunId, limit: Option<u64>) -> String {
    let applied = clamp_run_log_limit(limit);
    let note = clamp_note(limit);
    let records = read_turn_log(run_id);
    let total = records.len();
    if total == 0 {
        return empty_log_line(run_id, note.as_deref());
    }
    let skip = total.saturating_sub(applied as usize);
    let shown = &records[skip..];
    let mut out = header(run_id, total, shown.len(), note.as_deref());
    out.push('\n');
    for rec in shown {
        out.push_str(&render_record(rec));
        out.push('\n');
    }
    out
}

/// Records whose rendered text contains `query` case-insensitively, capped at
/// `limit`. The result names how many matched and, when the cap cut matches
/// off, says so and how to see the rest.
pub fn run_log_search(run_id: &RunId, query: &str, limit: Option<u64>) -> String {
    let applied = clamp_run_log_limit(limit);
    let note = clamp_note(limit);
    let records = read_turn_log(run_id);
    let needle = query.to_lowercase();
    let matches: Vec<&TurnLogRecord> = records
        .iter()
        .filter(|rec| search_text(rec).to_lowercase().contains(&needle))
        .collect();
    if matches.is_empty() {
        return format!("No turn-log records in run {run_id} match the query.\n");
    }
    let total = records.len();
    let shown_n = matches.len().min(applied as usize);
    let mut out = format!(
        "{} matching \"{}\"",
        header(run_id, total, shown_n, note.as_deref()),
        query
    );
    out.push('\n');
    for rec in matches.iter().take(applied as usize) {
        out.push_str(&render_record(rec));
        out.push('\n');
    }
    let more = matches.len() - shown_n;
    if more > 0 {
        out.push_str(&format!(
            "... {more} further match(es) not shown — raise the limit up to \
             {MAX_RUN_LOG_LIMIT} to see them.\n"
        ));
    }
    out
}

/// Everything turn `turn` logged up to the next `turn_started`. Uncapped by
/// design (design § 2.3): the caller has narrowed to a single turn, so nothing
/// clamps it. Returns the rendered block naming the run and the record count,
/// or a "not found" line the model can adapt to. Never an `Err`.
pub fn get_turn(run_id: &RunId, turn: u32) -> String {
    let records = read_turn_log(run_id);
    if records.is_empty() {
        return empty_log_line(run_id, None);
    }
    let Some(start) = find_turn_start(&records, turn) else {
        return format!("No turn {turn} found in run {run_id}\n");
    };
    let end = records[start + 1..]
        .iter()
        .position(|rec| matches!(rec.event, TurnLogEvent::TurnStarted { .. }))
        .map_or(records.len(), |off| start + 1 + off);
    let slice = &records[start..end];
    let mut out = format!("Run {run_id} turn {turn}: {} record(s).\n", slice.len());
    for rec in slice {
        out.push_str(&render_record(rec));
        out.push('\n');
    }
    out
}

/// Index of the first `TurnStarted { turn }` record — or `None`.
fn find_turn_start(records: &[TurnLogRecord], turn: u32) -> Option<usize> {
    records
        .iter()
        .position(|rec| matches!(rec.event, TurnLogEvent::TurnStarted { turn: t } if t == turn))
}

fn empty_log_line(run_id: &RunId, note: Option<&str>) -> String {
    let suffix = note.map(|n| format!(" ({n})")).unwrap_or_default();
    format!("Run {run_id} has no turn-log records{suffix}\n")
}

/// Turn a rejected run id (or other malformed input) into the `String` the
/// tool reports back. An unknown or malformed run id is a tool **result**, not
/// an `Err` — the model sees the wording and adapts.
pub fn run_log_input_rejected(reason: &str) -> String {
    format!("Error reading run log: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::turn_log::append_turn_event;
    use crate::daemon::run::{Briefing, BudgetDimension};

    /// RAII-guarded HOME pointing at a fresh tempdir, so a test that writes a
    /// turn log never touches the developer's real `~/.daemoneye`.
    struct HomeGuard {
        _lock: crate::TestHomeGuard,
        _tmp: tempfile::TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let _lock = crate::test_home_guard();
            let tmp = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", tmp.path()) };
            HomeGuard { _lock, _tmp: tmp }
        }
    }

    fn log(run_id: &RunId, events: Vec<TurnLogEvent>) {
        for (i, event) in events.into_iter().enumerate() {
            append_turn_event(&TurnLogRecord {
                timestamp: format!("2026-09-09T20:01:{i:02}Z"),
                run_id: run_id.clone(),
                event,
            });
        }
    }

    fn note(text: &str) -> TurnLogEvent {
        TurnLogEvent::Note {
            text: text.to_string(),
        }
    }

    fn turn(n: u32) -> TurnLogEvent {
        TurnLogEvent::TurnStarted { turn: n }
    }

    #[test]
    fn run_log_tail_returns_the_last_n_records() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071801").unwrap();
        log(
            &id,
            vec![
                note("one"),
                note("two"),
                note("three"),
                note("four"),
                note("five"),
            ],
        );
        let out = run_log_tail(&id, Some(2));
        assert!(out.contains("showing 2 of 5 record(s)"), "header: {out}");
        assert!(!out.contains("note: one"), "tail of 2 must drop the first");
        assert!(!out.contains("note: two"));
        assert!(!out.contains("note: three"));
        assert!(out.contains("note: four"), "the last two must come back");
        assert!(out.contains("note: five"));
        let four = out.find("note: four").unwrap();
        let five = out.find("note: five").unwrap();
        assert!(four < five, "oldest-first order in the returned slice");
        assert!(!out.contains("was clamped"), "2 < cap: no clamp note");
    }

    #[test]
    fn run_log_tail_clamps_an_oversized_limit() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071802").unwrap();
        let mut events = Vec::new();
        for i in 0..MAX_RUN_LOG_LIMIT + 10 {
            events.push(note(&format!("rec-{i:03}")));
        }
        let total = events.len();
        log(&id, events);
        let out = run_log_tail(&id, Some(10_000));
        assert!(
            out.contains(&format!("showing {MAX_RUN_LOG_LIMIT} of {total}")),
            "header must name the clamp: {out}"
        );
        assert!(
            out.contains("was clamped"),
            "the header must say the request was clamped"
        );
        assert!(!out.contains("note: rec-000"), "the head must fall off");
        assert!(out.contains("note: rec-099"), "the capped tail begins here");
        assert!(out.contains("note: rec-199"), "the last record must appear");
        assert!(out.matches("note: rec-").count() == MAX_RUN_LOG_LIMIT as usize);
    }

    #[test]
    fn run_log_tail_uses_the_default_when_limit_is_omitted() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071803").unwrap();
        let mut events = vec![note("early-marker")];
        for i in 0..(DEFAULT_RUN_LOG_LIMIT + 5) {
            events.push(note(&format!("body-{i:03}")));
        }
        log(&id, events);
        let out = run_log_tail(&id, None);
        assert!(
            out.contains(&format!("showing {DEFAULT_RUN_LOG_LIMIT} of")),
            "the default must apply when limit is omitted: {out}"
        );
        assert!(!out.contains("was clamped"), "the default is not a clamp");
        assert!(!out.contains("note: early-marker"));
        assert!(out.contains(&format!("note: body-{:03}", DEFAULT_RUN_LOG_LIMIT + 4)));
    }

    #[test]
    fn run_log_search_matches_case_insensitively() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071804").unwrap();
        log(&id, vec![note("Disk Full"), note("all clear")]);
        let out = run_log_search(&id, "disk full", None);
        assert!(
            out.contains("note: Disk Full"),
            "the match must render: {out}"
        );
        assert!(!out.contains("note: all clear"));
    }

    #[test]
    fn run_log_search_returns_nothing_for_no_match() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071805").unwrap();
        log(&id, vec![note("alpha"), note("beta")]);
        let out = run_log_search(&id, "gamma", None);
        assert!(
            out.contains("No turn-log records in run"),
            "a no-match must read as a result, not an error: {out}"
        );
        assert!(!out.contains("note: alpha"));
        assert!(!out.contains("note: beta"));
    }

    #[test]
    fn run_log_search_respects_the_clamp() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071806").unwrap();
        let mut events = Vec::new();
        for i in 0..MAX_RUN_LOG_LIMIT + 10 {
            events.push(note(&format!("match-{i}")));
        }
        log(&id, events);
        let out = run_log_search(&id, "match-", Some(10_000));
        assert!(
            out.contains("was clamped"),
            "search must say it clamped: {out}"
        );
        assert!(out.contains("10 further match(es) not shown"));
        assert_eq!(
            out.matches("note: match-").count(),
            MAX_RUN_LOG_LIMIT as usize
        );
    }

    #[test]
    fn get_turn_returns_only_that_turns_records() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071807").unwrap();
        log(
            &id,
            vec![
                turn(1),
                note("t1-probe"),
                turn(2),
                note("t2-probe"),
                note("t2-second"),
                turn(3),
                note("t3-probe"),
                TurnLogEvent::RunFinished {
                    state: RunState::Complete,
                },
            ],
        );
        let out = get_turn(&id, 2);
        assert!(out.contains("turn 2: 3 record(s)"), "header: {out}");
        assert!(out.contains("note: t2-probe"));
        assert!(out.contains("note: t2-second"));
        assert!(!out.contains("t1-probe"), "turn 1's records must not leak");
        assert!(!out.contains("t3-probe"), "turn 3's records must not leak");
        assert!(
            !out.contains("complete"),
            "turn 2 ends at the next turn_started"
        );
        // The uncapped escape hatch: one turn, however many records.
        let mut many = vec![turn(4)];
        for i in 0..MAX_RUN_LOG_LIMIT + 5 {
            many.push(note(&format!("big-{i:03}")));
        }
        many.push(turn(5));
        log(&id, many);
        let uncapped = get_turn(&id, 4);
        assert!(
            uncapped.contains(&format!("turn 4: {} record(s)", MAX_RUN_LOG_LIMIT + 5 + 1)),
            "get_turn must not clamp: {uncapped}"
        );
        assert!(uncapped.contains("note: big-000"));
        assert!(uncapped.contains(&format!("note: big-{MAX_RUN_LOG_LIMIT:03}")));
    }

    #[test]
    fn get_turn_reports_an_unknown_turn() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f6071808").unwrap();
        log(&id, vec![turn(1), note("only-turn-one")]);
        let out = get_turn(&id, 9);
        assert!(
            out.contains("No turn 9 found in run"),
            "an unknown turn must read as a result, not an error: {out}"
        );
        assert!(!out.contains("only-turn-one"));
    }

    #[test]
    fn run_log_tools_reject_an_unknown_run_as_a_result_not_an_error() {
        let _g = HomeGuard::new();
        let missing = RunId::parse("b2c3d4e5f6071809").unwrap();
        for (name, out) in [
            ("run_log_tail", run_log_tail(&missing, None)),
            (
                "run_log_search",
                run_log_search(&missing, "query-with-no-match", None),
            ),
            ("get_turn", get_turn(&missing, 1)),
        ] {
            assert!(
                out.contains("b2c3d4e5f6071809"),
                "{name} on a run with no log must name the run and stay a readable result, not an error: {out}"
            );
            assert!(
                !out.starts_with("Error") || out.contains("Error reading run log"),
                "{name} must never surface as a bare error: {out}"
            );
        }
        let parse_err = RunId::parse("NOT-HEX-AT-ALL").err().map(|e| e.to_string());
        assert!(
            parse_err.is_some(),
            "setup sanity: a malformed id must be rejected by RunId::parse"
        );
        let as_result = run_log_input_rejected(&parse_err.unwrap());
        assert!(
            as_result.contains("Invalid run id"),
            "the parse message must survive into the tool result: {as_result}"
        );
        assert!(
            !as_result.contains("\n"),
            "single line: no accidental formatting"
        );
    }

    #[test]
    fn run_finished_states_render_their_labels() {
        let _g = HomeGuard::new();
        let id = RunId::parse("b2c3d4e5f607180a").unwrap();
        let briefing = Briefing::new(
            "goal",
            Some("criteria".into()),
            vec!["src/main.rs".into()],
            vec!["tried X".into()],
            Some("blocked".into()),
            Some(BudgetDimension::Turns),
        );
        log(
            &id,
            vec![
                TurnLogEvent::RunStarted {
                    kind: RunKind::Ghost {
                        runbook: "investigate-disk".into(),
                        trigger: "cron".into(),
                    },
                },
                TurnLogEvent::RunFinished {
                    state: RunState::BudgetExceeded(briefing),
                },
            ],
        );
        let out = run_log_tail(&id, None);
        assert!(out.contains("run_started(ghost runbook=investigate-disk trigger=cron)"));
        assert!(out.contains("run_finished(state=budget_exceeded)"));
        assert!(out.contains("[2026-09-09T20:01:00Z]"), "timestamps render");
    }

    #[test]
    fn clamp_rejects_zero_by_falling_back_to_the_default() {
        assert_eq!(clamp_run_log_limit(None), DEFAULT_RUN_LOG_LIMIT);
        assert_eq!(clamp_run_log_limit(Some(0)), DEFAULT_RUN_LOG_LIMIT);
        assert_eq!(clamp_run_log_limit(Some(1)), 1);
        assert_eq!(
            clamp_run_log_limit(Some(MAX_RUN_LOG_LIMIT)),
            MAX_RUN_LOG_LIMIT
        );
        assert_eq!(clamp_run_log_limit(Some(u64::MAX)), MAX_RUN_LOG_LIMIT);
    }
}
