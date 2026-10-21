//! `daemoneye profile <model>` CLI — one model's strengths and failure
//! classes over the same `event: "run"` records the scorecard reads. No
//! daemon round-trip: reads the events segments directly, so it works when
//! the daemon is down.

use anyhow::{Context, Result};
use chrono::Utc;

use crate::daemon::run::{FailureClassCount, ModelProfile, ScorecardBy, aggregate_profile};

use super::costs::parse_date_bound;
use super::scorecard::{render_scorecard_table, run_records_between};

/// `daemoneye profile <MODEL> [--since DATE] [--until DATE] [--by
/// kind|agent|tag] [--json]`.
pub fn run_profile(
    model: &str,
    since: Option<String>,
    until: Option<String>,
    by: ScorecardBy,
    json: bool,
) -> Result<()> {
    let since_dt = match &since {
        Some(s) => Some(
            parse_date_bound(s, true)
                .with_context(|| format!("Invalid --since date: {s} (expected YYYY-MM-DD)"))?,
        ),
        // Every retained segment, like the scorecard's default.
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
    let profile = aggregate_profile(&records, model, by);

    if json {
        let json_str = serde_json::to_string_pretty(&profile)?;
        println!("{json_str}");
        return Ok(());
    }

    let since_label = since.as_deref().unwrap_or("the first segment");
    let until_label = until.as_deref().unwrap_or("now");
    if profile.n_runs == 0 {
        println!("no runs for {model} in this window");
        if profile.other_models.is_empty() {
            println!("no runs at all");
        } else {
            println!(
                "other models with runs: {}",
                profile.other_models.join(", ")
            );
        }
        return Ok(());
    }

    print_profile(&profile, by, since_label, until_label);
    Ok(())
}

/// The human profile: the header, the strengths table (the scorecard's
/// columns without `MODEL`) and the failure classes, ranked, with the
/// non-attributable ones marked.
fn print_profile(profile: &ModelProfile, by: ScorecardBy, since: &str, until: &str) {
    println!(
        "Profile of {} — {} run(s), {since} – {until} (by {})",
        profile.model,
        profile.n_runs,
        by.label()
    );
    println!("{}", render_scorecard_table(&profile.strengths.rows, by));
    println!();
    println!("Failure classes");
    for line in failure_class_lines(&profile.failure_classes) {
        println!("{line}");
    }
}

/// One line per failure class, `class  count`, widest class first so the
/// counts line up; a class the model could not have avoided carries a
/// `(not the model)` marker. An empty list is `no failures`.
fn failure_class_lines(classes: &[FailureClassCount]) -> Vec<String> {
    if classes.is_empty() {
        return vec!["  no failures".to_string()];
    }
    let width = classes
        .iter()
        .map(|c| c.class.chars().count())
        .max()
        .unwrap_or(0);
    classes
        .iter()
        .map(|c| {
            let marker = if c.attributable {
                ""
            } else {
                "  (not the model)"
            };
            let padded = format!("{:<width$}", c.class, width = width);
            format!("  {padded}  {}{}", c.count, marker)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::{FailureClassCount, ModelProfile, Scorecard};

    #[test]
    fn failure_class_lines_rank_and_mark() {
        let profile = ModelProfile {
            model: "m".to_string(),
            n_runs: 3,
            strengths: Scorecard::default(),
            failure_classes: vec![
                FailureClassCount {
                    class: "tool_errors".to_string(),
                    count: 2,
                    attributable: true,
                },
                FailureClassCount {
                    class: "backend_error".to_string(),
                    count: 1,
                    attributable: false,
                },
            ],
            other_models: vec![],
        };

        let lines = failure_class_lines(&profile.failure_classes);
        assert_eq!(
            lines[0], "  tool_errors    2",
            "an attributable class carries no marker and lines up with the widest class"
        );
        assert_eq!(
            lines[1], "  backend_error  1  (not the model)",
            "a non-attributable class is marked"
        );
    }

    #[test]
    fn a_classless_profile_says_no_failures() {
        let lines = failure_class_lines(&[]);
        assert_eq!(lines, vec!["  no failures"]);
    }
}
