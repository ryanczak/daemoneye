//! A single model's profile (M30 phase 04): the scorecard rows it earns and
//! the failure classes it hit, ranked. Pure data — no I/O, no clock; records
//! are supplied by the caller. `daemoneye profile` (phase 04) is the reader;
//! the MCP surface is a later milestone.

use serde::Serialize;

use super::FailureClass;
use super::RunRecord;
use super::ScorecardBy;
use super::aggregate_scorecard;

/// One failure class a model hit, with how often and whether the model owns
/// it. `attributable` mirrors [`FailureClass::is_model_attributable`], so an
/// outage (`backend_error`) is not read as a weakness.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct FailureClassCount {
    /// The class, in its wire form (`tool_errors`, `backend_error`, …).
    pub class: String,
    /// How many of the model's runs ended in this class.
    pub count: usize,
    /// Whether the model could have avoided it.
    pub attributable: bool,
}

/// The whole profile: one model's strengths and its failures, ranked.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct ModelProfile {
    /// The profiled model, as the caller spelled it.
    pub model: String,
    /// How many of the supplied records belong to this model.
    pub n_runs: usize,
    /// The model's scorecard rows, bucketed by the caller's dimension with
    /// no `min_runs`.
    pub strengths: super::Scorecard,
    /// Every failure class the model's runs hit except `none`, ranked by
    /// count descending then class name ascending.
    pub failure_classes: Vec<FailureClassCount>,
    /// Every other model that has runs, sorted and unique — what `daemoneye
    /// profile` names when this model has none.
    pub other_models: Vec<String>,
}

/// Profile one model over `records`: keep only its runs (an exact `model`
/// match — `m1` is not `m10`), aggregate their strengths with the caller's
/// `by` and no minimum, and rank its failure classes. Pure: no I/O, no clock.
pub fn aggregate_profile(records: &[RunRecord], model: &str, by: ScorecardBy) -> ModelProfile {
    let kept: Vec<&RunRecord> = records
        .iter()
        .filter(|r| r.model.as_deref() == Some(model))
        .collect();

    let owned: Vec<RunRecord> = kept.iter().map(|r| (*r).clone()).collect();
    let strengths = aggregate_scorecard(&owned, by, 0);

    let mut classes: Vec<FailureClassCount> = Vec::new();
    for record in &kept {
        if record.failure_class == FailureClass::None {
            continue;
        }
        let class = record.failure_class.as_str().to_string();
        if let Some(entry) = classes.iter_mut().find(|e| e.class == class) {
            entry.count += 1;
        } else {
            classes.push(FailureClassCount {
                class,
                count: 1,
                attributable: record.failure_class.is_model_attributable(),
            });
        }
    }
    classes.sort_by(|a, b| b.count.cmp(&a.count).then(a.class.cmp(&b.class)));

    let mut other_models: Vec<String> = records
        .iter()
        .filter_map(|r| r.model.as_ref())
        .filter(|m| *m != model)
        .cloned()
        .collect();
    other_models.sort();
    other_models.dedup();

    ModelProfile {
        model: model.to_string(),
        n_runs: kept.len(),
        strengths,
        failure_classes: classes,
        other_models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::RunStatus;

    fn record(run_id: &str, kind: &str, model: Option<&str>) -> RunRecord {
        RunRecord {
            run_id: run_id.to_string(),
            kind: kind.to_string(),
            model: model.map(str::to_string),
            ..RunRecord::default()
        }
    }

    fn failed(record: &mut RunRecord, class: FailureClass) {
        record.status = RunStatus::HardFail;
        record.failure_class = class.clone();
        record.is_model_attributable = class.is_model_attributable();
    }

    #[test]
    fn profile_keeps_only_the_named_models_runs() {
        let records = vec![
            record("a1", "chat", Some("m1")),
            record("a10", "chat", Some("m10")),
            record("a2", "chat", Some("m1")),
            record("b1", "ghost", Some("m2")),
        ];

        let profile = aggregate_profile(&records, "m1", ScorecardBy::Kind);

        assert_eq!(profile.n_runs, 2, "m1 is not m10: only m1's runs count");
        assert_eq!(profile.strengths.rows.len(), 1, "one chat bucket");
        assert_eq!(profile.strengths.rows[0].n_runs, 2);
        assert_eq!(profile.strengths.rows[0].model, "m1");
        assert_eq!(profile.model, "m1");
    }

    #[test]
    fn failure_classes_rank_by_count_then_name() {
        let mut r1 = record("a1", "chat", Some("m1"));
        failed(&mut r1, FailureClass::ToolErrors);
        let mut r2 = record("a2", "chat", Some("m1"));
        failed(&mut r2, FailureClass::ToolErrors);
        let mut r3 = record("a3", "chat", Some("m1"));
        failed(&mut r3, FailureClass::BackendError);
        let mut r4 = record("a4", "chat", Some("m1"));
        failed(&mut r4, FailureClass::ParseFormat);

        let profile = aggregate_profile(&[r1, r2, r3, r4], "m1", ScorecardBy::Kind);

        let names: Vec<(&str, usize)> = profile
            .failure_classes
            .iter()
            .map(|e| (e.class.as_str(), e.count))
            .collect();
        assert_eq!(
            names,
            vec![
                ("tool_errors", 2),
                ("backend_error", 1),
                ("parse_format", 1)
            ],
            "count descending, then class name ascending"
        );
    }

    #[test]
    fn none_is_excluded_from_failure_classes() {
        let mut complete = record("a1", "chat", Some("m1"));
        complete.status = RunStatus::Complete;
        let records = vec![complete, record("a2", "chat", Some("m1"))];

        let profile = aggregate_profile(&records, "m1", ScorecardBy::Kind);
        assert_eq!(profile.n_runs, 2);
        assert!(
            profile.failure_classes.is_empty(),
            "an all-complete model has no classes: {:?}",
            profile.failure_classes
        );
    }

    #[test]
    fn non_attributable_classes_are_marked() {
        let mut backend = record("a1", "chat", Some("m1"));
        failed(&mut backend, FailureClass::BackendError);
        let mut cancelled = record("a2", "chat", Some("m1"));
        cancelled.status = RunStatus::Cancelled;
        cancelled.failure_class = FailureClass::Cancelled;
        cancelled.is_model_attributable = false;
        let mut tools = record("a3", "chat", Some("m1"));
        failed(&mut tools, FailureClass::ToolErrors);

        let profile = aggregate_profile(&[backend, cancelled, tools], "m1", ScorecardBy::Kind);

        let by_class = |name: &str| {
            profile
                .failure_classes
                .iter()
                .find(|e| e.class == name)
                .unwrap_or_else(|| panic!("{name} must be present"))
        };
        assert!(!by_class("backend_error").attributable);
        assert!(!by_class("cancelled").attributable);
        assert!(by_class("tool_errors").attributable);
    }

    #[test]
    fn an_unknown_model_has_an_empty_profile() {
        let records = vec![
            record("a1", "chat", Some("m1")),
            record("b1", "chat", Some("m2")),
            record("b2", "chat", Some("m2")),
        ];

        let profile = aggregate_profile(&records, "nope", ScorecardBy::Kind);

        assert_eq!(profile.n_runs, 0);
        assert!(profile.strengths.rows.is_empty());
        assert!(profile.failure_classes.is_empty());
        assert_eq!(
            profile.other_models,
            vec!["m1".to_string(), "m2".to_string()],
            "the models that do have runs, sorted and unique"
        );
    }
}
