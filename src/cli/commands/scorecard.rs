//! `daemoneye scorecard` CLI — a pure aggregation over the `event: "run"`
//! records in the dated events segments.
//!
//! No daemon round-trip — reads the segments directly, like `daemoneye
//! costs`. Works even when the daemon is down.
use anyhow::{Context, Result};
use chrono::Utc;

use crate::daemon::run::{RunRecord, Scorecard, ScorecardBy, ScorecardRow, aggregate_scorecard};

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

/// One cell of a rendered table column: the header and each row's value,
/// already formatted.
struct Column {
    header: String,
    cells: Vec<String>,
    right: bool,
}

impl Column {
    /// The column's width: its widest cell, header included, so the table
    /// stays aligned however long a model or key is.
    fn width(&self) -> usize {
        std::iter::once(&self.header)
            .chain(self.cells.iter())
            .map(|c| c.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// One line of the column at `width`, left- or right-padded. Widths are
    /// computed from `.chars().count()` — the header contains `—`, which is
    /// one character but two bytes, so a byte-based pad would skew the table.
    fn render(&self, width: usize) -> Vec<String> {
        let pad = |c: &str| {
            let fill = width - c.chars().count();
            if self.right {
                format!("{}{c}", " ".repeat(fill))
            } else {
                format!("{c}{}", " ".repeat(fill))
            }
        };
        std::iter::once(pad(&self.header))
            .chain(self.cells.iter().map(|c| pad(c)))
            .collect()
    }
}

/// The scorecard's table — the header row and one line per row — sized to
/// its content: every column is as wide as its widest cell, header included,
/// so the columns line up however long a model or key is. Shared with the
/// profile's strengths table.
pub fn render_scorecard_table(rows: &[ScorecardRow], by: ScorecardBy) -> String {
    let columns = [
        Column {
            header: "MODEL".to_string(),
            cells: rows.iter().map(|r| r.model.clone()).collect(),
            right: false,
        },
        Column {
            header: by.label().to_uppercase(),
            cells: rows.iter().map(|r| r.key.clone()).collect(),
            right: false,
        },
        Column {
            header: "RUNS".to_string(),
            cells: rows.iter().map(|r| r.n_runs.to_string()).collect(),
            right: true,
        },
        Column {
            header: "PASS".to_string(),
            cells: rows.iter().map(|r| pct(Some(r.pass_rate))).collect(),
            right: true,
        },
        Column {
            header: "TOOL-OK".to_string(),
            cells: rows.iter().map(|r| pct(r.tool_success_rate_mean)).collect(),
            right: true,
        },
        Column {
            header: "PARSE-FAIL".to_string(),
            cells: rows
                .iter()
                .map(|r| pct(r.parse_failure_rate_mean))
                .collect(),
            right: true,
        },
        Column {
            header: "TURNS".to_string(),
            cells: rows
                .iter()
                .map(|r| format!("{:.1}", r.turns_mean))
                .collect(),
            right: true,
        },
        Column {
            header: "WALL".to_string(),
            cells: rows
                .iter()
                .map(|r| format!("{:.1}s", r.wall_clock_s_mean))
                .collect(),
            right: true,
        },
        Column {
            header: "COST".to_string(),
            cells: rows
                .iter()
                .map(|r| format!("${:.4}", r.cost_usd_mean))
                .collect(),
            right: true,
        },
        Column {
            header: "ATTR-FAIL".to_string(),
            cells: rows
                .iter()
                .map(|r| pct(Some(r.attributable_failure_rate)))
                .collect(),
            right: true,
        },
    ];

    let widths: Vec<usize> = columns.iter().map(Column::width).collect();
    let lines: Vec<Vec<String>> = columns
        .iter()
        .zip(&widths)
        .map(|(col, w)| col.render(*w))
        .collect();

    (0..rows.len() + 1)
        .map(|i| {
            lines
                .iter()
                .map(|col| col[i].as_str())
                .collect::<Vec<&str>>()
                .join("  ")
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn print_human(scorecard: &Scorecard, by: ScorecardBy, since: &str, until: &str) {
    println!("Run scorecard {since} – {until} (by {})", by.label());
    println!("{}", render_scorecard_table(&scorecard.rows, by));
    if scorecard.rows_dropped > 0 || scorecard.skipped_no_model > 0 {
        println!(
            "\n{} row(s) below --min-runs; {} run(s) with no model skipped",
            scorecard.rows_dropped, scorecard.skipped_no_model
        );
    }
}

/// `daemoneye scorecard`: read the records and render either the JSON or the
/// human table.
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
    fn long_model_names_keep_columns_aligned() {
        let row = |model: &str, n: usize| ScorecardRow {
            model: model.to_string(),
            key: "chat".to_string(),
            n_runs: n,
            pass_rate: 1.0,
            tool_success_rate_mean: None,
            parse_failure_rate_mean: None,
            turns_mean: 3.0,
            wall_clock_s_mean: 12.5,
            cost_usd_mean: 0.08,
            attributable_failure_rate: 0.0,
        };
        let rows = vec![row("claude-sonnet-4-6", 4), row("m2", 1)];
        let by = ScorecardBy::Kind;

        let table = render_scorecard_table(&rows, by);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3, "header plus two rows");

        // The columns the renderer must produce, in order: header and the
        // cells of each row. The test recomputes the widths from the same
        // inputs so an off-by-one in the renderer cannot hide behind them.
        let headers = [
            "MODEL",
            "KIND",
            "RUNS",
            "PASS",
            "TOOL-OK",
            "PARSE-FAIL",
            "TURNS",
            "WALL",
            "COST",
            "ATTR-FAIL",
        ];
        let cell_lists: Vec<Vec<String>> = vec![
            rows.iter().map(|r| r.model.clone()).collect(),
            rows.iter().map(|r| r.key.clone()).collect(),
            rows.iter().map(|r| r.n_runs.to_string()).collect(),
            rows.iter().map(|r| pct(Some(r.pass_rate))).collect(),
            rows.iter().map(|r| pct(r.tool_success_rate_mean)).collect(),
            rows.iter()
                .map(|r| pct(r.parse_failure_rate_mean))
                .collect(),
            rows.iter()
                .map(|r| format!("{:.1}", r.turns_mean))
                .collect(),
            rows.iter()
                .map(|r| format!("{:.1}s", r.wall_clock_s_mean))
                .collect(),
            rows.iter()
                .map(|r| format!("${:.4}", r.cost_usd_mean))
                .collect(),
            rows.iter()
                .map(|r| pct(Some(r.attributable_failure_rate)))
                .collect(),
        ];
        let widths: Vec<usize> = headers
            .iter()
            .zip(&cell_lists)
            .map(|(h, cells)| {
                cells
                    .iter()
                    .map(|c| c.chars().count())
                    .chain([h.chars().count()])
                    .max()
                    .unwrap_or(0)
            })
            .collect();

        let starts: Vec<usize> = widths
            .iter()
            .enumerate()
            .scan(0usize, |acc, (i, w)| {
                let start = *acc;
                *acc += w + 2;
                Some((i, start))
            })
            .map(|(_, start)| start)
            .collect();

        let rights = [false, false, true, true, true, true, true, true, true, true];
        for (i, (header, cells)) in headers.iter().zip(&cell_lists).enumerate() {
            let (start, width, right) = (starts[i], widths[i], rights[i]);
            for (line_no, line) in lines.iter().enumerate() {
                let at = match line_no {
                    0 => header.to_string(),
                    n => cells[n - 1].clone(),
                };
                let fill = width - at.chars().count();
                let expected = if right {
                    format!("{}{at}", " ".repeat(fill))
                } else {
                    format!("{at}{}", " ".repeat(fill))
                };
                let slice: String = line.chars().skip(start).take(width).collect();
                assert_eq!(
                    slice, expected,
                    "column {header} of line {line_no} must sit at offset {start}: `{line}`"
                );
            }
        }
    }

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
