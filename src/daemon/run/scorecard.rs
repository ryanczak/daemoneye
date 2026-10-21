//! The scorecard: a pure aggregation over `RunRecord`s (M30 phase 03).
//! Buckets finished runs by model and one secondary dimension and counts
//! `n_runs` per bucket, so a thin row reads as thin. No I/O, no clock —
//! records are supplied by the caller; `daemoneye scorecard` (phase 03),
//! `daemoneye profile` (phase 04) and the MCP surface (a later milestone)
//! reuse this aggregation.

use serde::Serialize;
use std::collections::BTreeMap;

use super::RunRecord;

/// The secondary dimension a scorecard buckets by. `Kind` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ScorecardBy {
    /// The run's kind label (`chat`, `ghost`, `scheduled`, `delegated`).
    #[default]
    Kind,
    /// The `agent` tag; a run with no agent buckets as `(none)`.
    Agent,
    /// One row per `k=v` tag the record carries, `kind=…` included.
    Tag,
}

impl ScorecardBy {
    /// The human word for the dimension — the column header and the label
    /// the header line names the `--by` by.
    pub fn label(&self) -> &'static str {
        match self {
            ScorecardBy::Kind => "kind",
            ScorecardBy::Agent => "agent",
            ScorecardBy::Tag => "tag",
        }
    }
}

/// One aggregated bucket: a model against one secondary key.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ScorecardRow {
    /// The configured model the run's calls went to.
    pub model: String,
    /// The bucket key: the kind label, the agent (or `(none)`), or one
    /// `k=v` tag.
    pub key: String,
    /// How many runs landed in the bucket.
    pub n_runs: usize,
    /// Runs ending `complete`, over `n_runs`.
    pub pass_rate: f64,
    /// Mean `tool_success_rate` over the runs whose rate is non-null;
    /// `None` when every run's rate is null. A null rate is not a zero.
    pub tool_success_rate_mean: Option<f64>,
    /// Mean `parse_failure_rate` over the runs whose rate is non-null;
    /// `None` when every run's rate is null.
    pub parse_failure_rate_mean: Option<f64>,
    /// Mean turns.
    pub turns_mean: f64,
    /// Mean wall-clock seconds.
    pub wall_clock_s_mean: f64,
    /// Mean cost in USD.
    pub cost_usd_mean: f64,
    /// Runs with `is_model_attributable`, over `n_runs`.
    pub attributable_failure_rate: f64,
}

/// The whole aggregation: the surviving rows plus what was filtered out.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct Scorecard {
    pub rows: Vec<ScorecardRow>,
    /// Records with no `model` — the `scheduled` job wrappers, which make
    /// no costed model call. Never counted in a row.
    pub skipped_no_model: usize,
    /// Rows dropped by `min_runs`.
    pub rows_dropped: usize,
}

/// The mutable per-bucket accumulator behind [`aggregate_scorecard`].
#[derive(Default)]
struct Bucket {
    model: String,
    key: String,
    n_runs: usize,
    passes: usize,
    attributable: usize,
    turns: u64,
    wall_clock_s: f64,
    cost_usd: f64,
    tool_success_rate_sum: f64,
    tool_success_rate_n: usize,
    parse_failure_rate_sum: f64,
    parse_failure_rate_n: usize,
}

impl Bucket {
    fn fold(&mut self, record: &RunRecord) {
        self.n_runs += 1;
        if record.status == crate::daemon::run::RunStatus::Complete {
            self.passes += 1;
        }
        if record.is_model_attributable {
            self.attributable += 1;
        }
        self.turns += u64::from(record.turns);
        self.wall_clock_s += record.wall_clock_s;
        self.cost_usd += record.cost_usd;
        if let Some(rate) = record.tool_success_rate {
            self.tool_success_rate_sum += rate;
            self.tool_success_rate_n += 1;
        }
        if let Some(rate) = record.parse_failure_rate {
            self.parse_failure_rate_sum += rate;
            self.parse_failure_rate_n += 1;
        }
    }

    fn finish(self) -> ScorecardRow {
        let n = self.n_runs.max(1) as f64;
        ScorecardRow {
            model: self.model,
            key: self.key,
            n_runs: self.n_runs,
            pass_rate: self.passes as f64 / n,
            tool_success_rate_mean: (self.tool_success_rate_n > 0)
                .then(|| self.tool_success_rate_sum / self.tool_success_rate_n as f64),
            parse_failure_rate_mean: (self.parse_failure_rate_n > 0)
                .then(|| self.parse_failure_rate_sum / self.parse_failure_rate_n as f64),
            turns_mean: self.turns as f64 / n,
            wall_clock_s_mean: self.wall_clock_s / n,
            cost_usd_mean: self.cost_usd / n,
            attributable_failure_rate: self.attributable as f64 / n,
        }
    }
}

/// The keys a record contributes for one dimension: exactly one for `Kind`
/// and `Agent`, one per tag for `Tag`. The record is assumed to have a
/// model — the caller skips model-less records first.
fn keys_for(by: ScorecardBy, record: &RunRecord) -> Vec<String> {
    match by {
        ScorecardBy::Kind => vec![record.kind.clone()],
        ScorecardBy::Agent => vec![record.agent.clone().unwrap_or_else(|| NONE_KEY.to_string())],
        ScorecardBy::Tag => record
            .tags
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect(),
    }
}

/// The key an agent-less run buckets under.
pub const NONE_KEY: &str = "(none)";

/// Bucket `records` by model and one secondary dimension, drop rows below
/// `min_runs` (`0` = no minimum) and sort by `key`, then `n_runs`
/// descending, then `model`. Pure: no I/O, no clock.
pub fn aggregate_scorecard(records: &[RunRecord], by: ScorecardBy, min_runs: usize) -> Scorecard {
    let mut buckets: BTreeMap<(String, String), Bucket> = BTreeMap::new();
    let mut scorecard = Scorecard::default();

    for record in records {
        let Some(model) = record.model.as_ref() else {
            scorecard.skipped_no_model += 1;
            continue;
        };
        for key in keys_for(by, record) {
            let bucket = buckets
                .entry((key.clone(), model.clone()))
                .or_insert_with(|| Bucket {
                    model: model.clone(),
                    key,
                    ..Bucket::default()
                });
            bucket.fold(record);
        }
    }

    let mut rows: Vec<ScorecardRow> = buckets
        .into_values()
        .map(Bucket::finish)
        .filter(|row| {
            if row.n_runs < min_runs {
                scorecard.rows_dropped += 1;
                false
            } else {
                true
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then(b.n_runs.cmp(&a.n_runs))
            .then(a.model.cmp(&b.model))
    });

    scorecard.rows = rows;
    scorecard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::{FailureClass, RunStatus};

    fn record(run_id: &str, kind: &str, model: Option<&str>) -> RunRecord {
        RunRecord {
            run_id: run_id.to_string(),
            kind: kind.to_string(),
            model: model.map(str::to_string),
            ..RunRecord::default()
        }
    }

    #[test]
    fn rows_bucket_by_model_and_kind() {
        let mut attributable = record("a3", "chat", Some("m1"));
        attributable.status = RunStatus::HardFail;
        attributable.failure_class = FailureClass::ToolErrors;
        attributable.is_model_attributable = true;

        let records = vec![
            record("a1", "chat", Some("m1")),
            record("a2", "chat", Some("m1")),
            attributable,
            record("b1", "ghost", Some("m2")),
        ];
        let scorecard = aggregate_scorecard(&records, ScorecardBy::Kind, 0);

        assert_eq!(scorecard.rows.len(), 2, "one row per (kind, model)");
        assert_eq!(scorecard.rows[0].key, "chat");
        assert_eq!(scorecard.rows[0].model, "m1");
        assert_eq!(scorecard.rows[0].n_runs, 3);
        assert!(
            (scorecard.rows[0].pass_rate - 2.0 / 3.0).abs() < 1e-9,
            "two complete of three: {}",
            scorecard.rows[0].pass_rate
        );
        assert!(
            (scorecard.rows[0].attributable_failure_rate - 1.0 / 3.0).abs() < 1e-9,
            "one attributable of three: {}",
            scorecard.rows[0].attributable_failure_rate
        );
        assert_eq!(scorecard.rows[1].key, "ghost");
        assert_eq!(scorecard.rows[1].model, "m2");
        assert_eq!(scorecard.rows[1].n_runs, 1);
        assert_eq!(scorecard.skipped_no_model, 0);
    }

    #[test]
    fn min_runs_drops_thin_rows() {
        let records = vec![
            record("a1", "chat", Some("m1")),
            record("a2", "chat", Some("m1")),
            record("b1", "ghost", Some("m2")),
        ];

        let dropped = aggregate_scorecard(&records, ScorecardBy::Kind, 2);
        assert_eq!(dropped.rows.len(), 1, "the one-run row is dropped");
        assert_eq!(dropped.rows[0].key, "chat");
        assert_eq!(dropped.rows_dropped, 1);

        let kept = aggregate_scorecard(&records, ScorecardBy::Kind, 0);
        assert_eq!(kept.rows.len(), 2, "0 means no minimum");
        assert_eq!(kept.rows_dropped, 0);
    }

    #[test]
    fn rate_means_skip_null_rates() {
        let mut with_rate = record("a1", "chat", Some("m1"));
        with_rate.tool_success_rate = Some(1.0);
        with_rate.parse_failure_rate = Some(0.5);
        let mut without = record("a2", "chat", Some("m1"));
        without.tool_success_rate = None;
        without.parse_failure_rate = None;

        let mixed = aggregate_scorecard(&[with_rate.clone(), without], ScorecardBy::Kind, 0);
        let row = &mixed.rows[0];
        assert_eq!(
            row.tool_success_rate_mean,
            Some(1.0),
            "a null rate must not drag the mean down"
        );
        assert_eq!(row.parse_failure_rate_mean, Some(0.5));

        let mut all_null = record("b1", "chat", Some("m2"));
        all_null.tool_success_rate = None;
        all_null.parse_failure_rate = None;
        let empty = aggregate_scorecard(&[all_null], ScorecardBy::Kind, 0);
        assert_eq!(
            empty.rows[0].tool_success_rate_mean, None,
            "all-null gives None, not 0.0"
        );
        assert_eq!(empty.rows[0].parse_failure_rate_mean, None);
    }

    #[test]
    fn by_agent_buckets_a_missing_agent_as_none() {
        let mut agented = record("a1", "ghost", Some("m1"));
        agented.agent = Some("analyst".to_string());
        let records = vec![agented, record("a2", "chat", Some("m1"))];

        let scorecard = aggregate_scorecard(&records, ScorecardBy::Agent, 0);
        assert_eq!(scorecard.rows.len(), 2);
        assert_eq!(scorecard.rows[0].key, NONE_KEY);
        assert_eq!(scorecard.rows[1].key, "analyst");
    }

    #[test]
    fn by_tag_counts_a_run_once_per_tag() {
        let mut tagged = record("b1", "ghost", Some("m1"));
        tagged.tags = [
            ("kind".to_string(), "ghost".to_string()),
            ("runbook".to_string(), "r".to_string()),
        ]
        .into_iter()
        .collect();

        let scorecard = aggregate_scorecard(&[tagged], ScorecardBy::Tag, 0);
        assert_eq!(scorecard.rows.len(), 2, "one row per tag");
        assert_eq!(scorecard.rows[0].key, "kind=ghost");
        assert_eq!(scorecard.rows[0].n_runs, 1);
        assert_eq!(scorecard.rows[1].key, "runbook=r");
        assert_eq!(scorecard.rows[1].n_runs, 1);
    }

    #[test]
    fn runs_without_a_model_are_skipped_and_counted() {
        let records = vec![
            record("c1", "scheduled", None),
            record("c2", "scheduled", None),
            record("a1", "chat", Some("m1")),
        ];
        let scorecard = aggregate_scorecard(&records, ScorecardBy::Kind, 0);
        assert_eq!(scorecard.rows.len(), 1, "only the modelled run rows");
        assert_eq!(scorecard.rows[0].key, "chat");
        assert_eq!(scorecard.skipped_no_model, 2, "skipped runs are counted");
    }

    #[test]
    fn rows_sort_by_key_then_runs_then_model() {
        let records = vec![
            record("a1", "chat", Some("m2")),
            record("a2", "chat", Some("m2")),
            record("a3", "chat", Some("m1")),
            record("b1", "ghost", Some("m1")),
        ];
        let scorecard = aggregate_scorecard(&records, ScorecardBy::Kind, 0);

        let keys: Vec<(&str, &str)> = scorecard
            .rows
            .iter()
            .map(|r| (r.key.as_str(), r.model.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("chat", "m2"), // same key: n_runs 2 sorts before 1
                ("chat", "m1"),
                ("ghost", "m1"),
            ],
            "key ascending, then n_runs descending, then model"
        );
    }
}
