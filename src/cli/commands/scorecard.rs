//! `daemoneye scorecard` CLI — a pure aggregation over the `event: "run"`
//! records in the dated events segments.
//!
//! No daemon round-trip — reads the segments directly, like `daemoneye
//! costs`. Works even when the daemon is down.
use anyhow::{Context, Result};
use chrono::Utc;

use crate::daemon::run::{RunRecord, Scorecard, ScorecardBy, aggregate_scorecard};

use super::costs::parse_date_bound;

/// Every `event == "run"` record in the `[since, until)` window, in segment
/// order. A line that does not parse as a `RunRecord` is skipped; lines of
/// any other event are ignored. Neither count is returned — the reader's
/// only contract is the records it found.
pub fn run_records_between(
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
) -> Vec<RunRecord> {
    let mut records = Vec::new();
    crate::daemon::utils::for_each_event_between(since, until, &mut |value| {
        if value.get("event").and_then(|v| v.as_str()) != Some("run") {
            return;
        }
        if let Ok(record) = serde_json::from_value::<RunRecord>(value.clone()) {
            records.push(record);
        }
    });
    records
}

/// Render one rate as a percentage, or `—` when the mean is `None`.
fn pct(rate: Option<f64>) -> String {
    match rate {
        Some(v) => format!("{:.1}%", v * 100.0),
        None => "—".to_string(),
    }
}

const MODEL_W: usize = 6;
const KEY_W: usize = 10;
const RUNS_W: usize = 4;
const PASS_W: usize = 5;
const TOOL_W: usize = 7;
const PARSE_W: usize = 10;
const TURNS_W: usize = 5;
const WALL_W: usize = 6;
const COST_W: usize = 6;
const ATTR_W: usize = 9;

fn print_human(scorecard: &Scorecard, by: ScorecardBy, since: &str, until: &str) {
    println!("Run scorecard {since} – {until} (by {})", by.label());
    println!(
        "{:<MODEL_W$}  {:<KEY_W$}  {:>RUNS_W$}  {:>PASS_W$}  {:>TOOL_W$}  {:>PARSE_W$}  {:>TURNS_W$}  {:>WALL_W$}  {:>COST_W$}  {:>ATTR_W$}",
        "MODEL",
        by.label().to_uppercase(),
        "RUNS",
        "PASS",
        "TOOL-OK",
        "PARSE-FAIL",
        "TURNS",
        "WALL",
        "COST",
        "ATTR-FAIL",
    );
    for row in &scorecard.rows {
        println!(
            "{:<MODEL_W$}  {:<KEY_W$}  {:>RUNS_W$}  {:>PASS_W$}  {:>TOOL_W$}  {:>PARSE_W$}  {:>TURNS_W$}  {:>WALL_W$}  {:>COST_W$}  {:>ATTR_W$}",
            row.model,
            row.key,
            row.n_runs,
            pct(Some(row.pass_rate)),
            pct(row.tool_success_rate_mean),
            pct(row.parse_failure_rate_mean),
            format!("{:.1}", row.turns_mean),
            format!("{:.1}s", row.wall_clock_s_mean),
            format!("${:.4}", row.cost_usd_mean),
            pct(Some(row.attributable_failure_rate)),
        );
    }
    if scorecard.rows_dropped > 0 || scorecard.skipped_no_model > 0 {
        println!(
            "\n{} row(s) below --min-runs; {} run(s) with no model skipped",
            scorecard.rows_dropped, scorecard.skipped_no_model
        );
    }
}

pub fn run_scorecard(
    since: Option<String>,
    until: Option<String>,
    by: ScorecardBy,
    min_runs: usize,
    json: bool,
) -> Result<()> {
    let since_dt = match &since {
        Some(s) => Some(
            parse_date_bound(s, true)
                .with_context(|| format!("Invalid --since date: {s} (expected YYYY-MM-DD)"))?,
        ),
        // Every retained segment, not a seven-day window: a scorecard wants
        // every run it can get.
        None => None,
    };
    let until_dt = match &until {
        Some(s) => Some(
            parse_date_bound(s, false)
                .with_context(|| format!("Invalid --until date: {s} (expected YYYY-MM-DD)"))?,
        ),
        None => Some(Utc::now()),
    };

    let records = run_records_between(since_dt, until_dt);
    let scorecard = aggregate_scorecard(&records, by, min_runs);

    if json {
        let json_str = serde_json::to_string_pretty(&scorecard)?;
        println!("{json_str}");
    } else {
        // The human header names the window: the raw bounds or the words for
        // the unbounded side.
        let since_label = since.as_deref().unwrap_or("the first segment");
        let until_label = until.as_deref().unwrap_or("now");
        if scorecard.rows.is_empty() {
            println!("no runs in this window");
            return Ok(());
        }
        print_human(&scorecard, by, since_label, until_label);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_records_are_read_from_the_segments_only() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();
        let seg = crate::config::current_event_segment_path();
        let ts = "2026-10-21T00:00:00Z";
        let run = |extra: &str| format!(r#"{{"ts":"{ts}","event":"run","pid":1,{extra}}}"#);
        std::fs::write(
            &seg,
            format!(
                "{}\n{}\n{}\n{}\n",
                run(r#""run_id":"a1","kind":"chat","model":"m1","status":"complete""#),
                run(r#""run_id":"a2","kind":"chat","model":"m1","status":"hard_fail""#),
                run(r#""run_id":"b1","kind":"chat","model":"m1","status":"complete""nope""#),
                r#"{"ts":"2026-10-21T00:00:00Z","event":"ai_cost","pid":1,"run_id":"a1"}"#
            ),
        )
        .unwrap();

        let records = run_records_between(None, None);
        assert_eq!(
            records.len(),
            2,
            "only the two parseable run lines: {:?}",
            records
        );
        assert_eq!(records[0].run_id, "a1");
        assert_eq!(records[1].run_id, "a2");
        assert!(
            records.iter().all(|r| r.run_id != "b1"),
            "the malformed run line must be skipped"
        );
        // The `ai_cost` line is an ignored event, not a record; the malformed
        // run line is skipped by the same read the daemon feeds.
        assert!(
            records.iter().all(|r| r.kind != "ai_cost"),
            "non-run events are ignored"
        );
    }
}
