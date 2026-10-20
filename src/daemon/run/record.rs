//! The one `event: "run"` record a finished run leaves (M30 phase 02), and
//! the `failure_class` computed from its terminal state and tally. Pure
//! data: built by [`crate::daemon::run::RunLogGuard`] at drop, appended to
//! the dated events segment beside `ai_cost`, read by the scorecard
//! (phase 03) and the profile (phase 04).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{RunMetrics, RunState, RunStatus};

/// A `run` record's failure class, in the wire vocabulary the scorecard
/// (phase 03) ranks. Every variant's wire name is spelled explicitly so a
/// Rust rename cannot move the wire.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub enum FailureClass {
    /// The run finished; any failed tools it had are not a class of its own.
    #[serde(rename = "none")]
    #[default]
    None,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "budget_exceeded")]
    BudgetExceeded,
    #[serde(rename = "backend_error")]
    BackendError,
    #[serde(rename = "tool_errors")]
    ToolErrors,
    /// Reserved for the governor (a later milestone); nothing produces it yet.
    #[serde(rename = "stall")]
    Stall,
    #[serde(rename = "parse_format")]
    ParseFormat,
    /// A hard fail whose tally names nothing specific: a ghost internal
    /// error, a failed script job, a missed outcome.
    #[serde(rename = "other")]
    Other,
}

/// Every failure class a `run` record may carry, in wire order. `stall` has
/// no producer yet but is reserved so a later producer cannot rename it
/// under old readers.
pub const FAILURE_CLASSES: [FailureClass; 8] = [
    FailureClass::None,
    FailureClass::Cancelled,
    FailureClass::BudgetExceeded,
    FailureClass::BackendError,
    FailureClass::ToolErrors,
    FailureClass::Stall,
    FailureClass::ParseFormat,
    FailureClass::Other,
];

impl FailureClass {
    /// The wire string — the same string serde emits.
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureClass::None => "none",
            FailureClass::Cancelled => "cancelled",
            FailureClass::BudgetExceeded => "budget_exceeded",
            FailureClass::BackendError => "backend_error",
            FailureClass::ToolErrors => "tool_errors",
            FailureClass::Stall => "stall",
            FailureClass::ParseFormat => "parse_format",
            FailureClass::Other => "other",
        }
    }

    /// Whether a run ending in this class is a failure the model could have
    /// avoided: failed tool calls, malformed tool arguments, or (once the
    /// governor exists) a stall. Not attributable: a run that finished, was
    /// stopped, ran out of budget, whose backend errored, or failed some
    /// other way.
    pub fn is_model_attributable(&self) -> bool {
        matches!(
            self,
            FailureClass::ToolErrors | FailureClass::ParseFormat | FailureClass::Stall
        )
    }
}

/// `failure_class` from the terminal state and the tally, in the order the
/// record's contract names: a terminal state that is not `HardFail` wins
/// outright — its tally is ignored, so a complete run with failed tools is
/// still `none` — and a `HardFail` is classified by what went wrong first:
/// backend errors, then parse failures, then failed tool calls, then
/// anything else.
pub fn failure_class(state: &RunState, metrics: &RunMetrics) -> FailureClass {
    match state {
        RunState::Running | RunState::Complete => FailureClass::None,
        RunState::Cancelled { .. } => FailureClass::Cancelled,
        RunState::BudgetExceeded(_) => FailureClass::BudgetExceeded,
        RunState::HardFail(_) => {
            if metrics.backend_errors > 0 {
                FailureClass::BackendError
            } else if metrics.parse_failures > 0 {
                FailureClass::ParseFormat
            } else if metrics.tool_successes < metrics.tool_calls {
                FailureClass::ToolErrors
            } else {
                FailureClass::Other
            }
        }
    }
}

/// The free-function form the scorecard (phase 03) reads; delegates to
/// [`FailureClass::is_model_attributable`].
pub fn is_model_attributable(class: FailureClass) -> bool {
    class.is_model_attributable()
}

/// The one record a finished run leaves in the dated events segment.
/// `#[serde(default)]` so a later reader parses an older or partial line:
/// every field missing from the line falls back to the default record
/// rather than failing the parse. `ts`, `event` and `pid` are deliberately
/// absent — `log_event` stamps them.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct RunRecord {
    pub run_id: String,
    /// The [`crate::daemon::run::RunKind::label`] — `chat`, `ghost`,
    /// `scheduled` or `delegated`.
    pub kind: String,
    /// The `agent` tag, when the run was delegated or the ghost named an
    /// agent.
    pub agent: Option<String>,
    /// The configured provider of the run's model, when it made a model call.
    pub provider: Option<String>,
    /// The configured model, when it made a model call.
    pub model: Option<String>,
    pub status: RunStatus,
    pub failure_class: FailureClass,
    pub is_model_attributable: bool,
    pub turns: u32,
    pub tool_calls: u32,
    /// `None` (JSON `null`) when the run executed no tool call.
    pub tool_success_rate: Option<f64>,
    /// `None` (JSON `null`) when the run executed no tool call.
    pub parse_failure_rate: Option<f64>,
    pub tokens: crate::ai::types::TokenBreakdown,
    pub cost_usd: f64,
    pub wall_clock_s: f64,
    /// Kind, runbook, agent, job and host tags, sorted so the wire form is
    /// stable.
    pub tags: BTreeMap<String, String>,
}

/// `PartialEq` is hand-written field by field: `TokenBreakdown` derives no
/// `PartialEq`, so the tokens are compared by their JSON form while every
/// scalar field compares directly.
impl PartialEq for RunRecord {
    fn eq(&self, other: &Self) -> bool {
        let tokens_equal = match (
            serde_json::to_string(&self.tokens),
            serde_json::to_string(&other.tokens),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => true,
        };
        tokens_equal
            && self.run_id == other.run_id
            && self.kind == other.kind
            && self.agent == other.agent
            && self.provider == other.provider
            && self.model == other.model
            && self.status == other.status
            && self.failure_class == other.failure_class
            && self.is_model_attributable == other.is_model_attributable
            && self.turns == other.turns
            && self.tool_calls == other.tool_calls
            && self.tool_success_rate == other.tool_success_rate
            && self.parse_failure_rate == other.parse_failure_rate
            && self.cost_usd == other.cost_usd
            && self.wall_clock_s == other.wall_clock_s
            && self.tags == other.tags
    }
}

/// `RunStatus` has no `Default` (a record has no neutral status), so the
/// default record is written by hand: an empty shell whose only non-zero
/// reading is `status: complete` — a later reader meeting an older line
/// without the field reads a complete run rather than failing the parse.
impl Default for RunRecord {
    fn default() -> Self {
        RunRecord {
            run_id: String::new(),
            kind: String::new(),
            agent: None,
            provider: None,
            model: None,
            status: RunStatus::Complete,
            failure_class: FailureClass::None,
            is_model_attributable: false,
            turns: 0,
            tool_calls: 0,
            tool_success_rate: None,
            parse_failure_rate: None,
            tokens: crate::ai::types::TokenBreakdown::default(),
            cost_usd: 0.0,
            wall_clock_s: 0.0,
            tags: BTreeMap::new(),
        }
    }
}

impl RunRecord {
    /// Build the record from what a run ended with. Pure: no I/O, no clock
    /// — the wall clock is supplied in seconds by the caller (the guard).
    pub fn new(
        run_id: String,
        kind: String,
        state: &RunState,
        metrics: &RunMetrics,
        wall_clock_s: f64,
        tags: BTreeMap<String, String>,
    ) -> Self {
        let status = RunStatus::for_state(state)
            .unwrap_or(RunStatus::HardFail)
            .clone();
        let class = failure_class(state, metrics);
        RunRecord {
            run_id,
            kind,
            agent: tags.get("agent").cloned(),
            provider: metrics.provider.clone(),
            model: metrics.model.clone(),
            status,
            failure_class: class.clone(),
            is_model_attributable: class.is_model_attributable(),
            turns: metrics.turns,
            tool_calls: metrics.tool_calls,
            tool_success_rate: metrics.tool_success_rate(),
            parse_failure_rate: metrics.parse_failure_rate(),
            tokens: metrics.tokens.clone(),
            cost_usd: metrics.cost_usd,
            wall_clock_s,
            tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::{Briefing, RunState};

    fn failed_tools_tally() -> RunMetrics {
        RunMetrics {
            tool_calls: 3,
            tool_successes: 1,
            ..RunMetrics::default()
        }
    }

    fn hard_fail() -> RunState {
        RunState::HardFail(Briefing::new("g", None, vec![], vec![], None, None))
    }

    fn budget_exceeded() -> RunState {
        RunState::BudgetExceeded(Briefing::new("g", None, vec![], vec![], None, None))
    }

    #[test]
    fn terminal_states_map_to_their_class() {
        for (state, class) in [
            (RunState::Complete, FailureClass::None),
            (
                RunState::Cancelled {
                    stage: "awaiting_model".into(),
                    turns: 2,
                },
                FailureClass::Cancelled,
            ),
            (budget_exceeded(), FailureClass::BudgetExceeded),
        ] {
            assert_eq!(
                failure_class(&state, &failed_tools_tally()),
                class,
                "the tally must be ignored for {class:?}"
            );
        }
    }

    #[test]
    fn hard_fail_is_classified_by_the_tally() {
        let backend = RunMetrics {
            parse_failures: 2,
            backend_errors: 1,
            ..RunMetrics::default()
        };
        assert_eq!(
            failure_class(&hard_fail(), &backend),
            FailureClass::BackendError,
            "a backend error wins over parse failures"
        );

        let parse = RunMetrics {
            tool_calls: 2,
            tool_successes: 1,
            parse_failures: 1,
            ..RunMetrics::default()
        };
        assert_eq!(
            failure_class(&hard_fail(), &parse),
            FailureClass::ParseFormat,
            "parse failures win over failed tools"
        );

        assert_eq!(
            failure_class(&hard_fail(), &failed_tools_tally()),
            FailureClass::ToolErrors,
            "failed tools alone are tool_errors"
        );

        assert_eq!(
            failure_class(&hard_fail(), &RunMetrics::default()),
            FailureClass::Other,
            "a clean tally behind a hard fail is other"
        );
    }

    #[test]
    fn only_model_failures_are_attributable() {
        let attributable = [
            FailureClass::ToolErrors,
            FailureClass::ParseFormat,
            FailureClass::Stall,
        ];
        let not_attributable = [
            FailureClass::None,
            FailureClass::Cancelled,
            FailureClass::BudgetExceeded,
            FailureClass::BackendError,
            FailureClass::Other,
        ];
        for class in &FAILURE_CLASSES {
            if attributable.contains(class) {
                assert!(
                    is_model_attributable(class.clone()),
                    "{class:?} must be attributable"
                );
            } else {
                assert!(
                    not_attributable.contains(class),
                    "the two lists must cover every class together: {class:?}"
                );
                assert!(
                    !is_model_attributable(class.clone()),
                    "{class:?} must not be attributable"
                );
            }
        }
        assert_eq!(FAILURE_CLASSES.len(), 8, "every class is in the list");
    }

    #[test]
    fn a_run_record_round_trips_and_old_lines_parse() {
        let record = RunRecord::new(
            "6ac1a6b2e5f60701".to_string(),
            "ghost".to_string(),
            &hard_fail(),
            &failed_tools_tally(),
            12.5,
            [("runbook".to_string(), "disk".to_string())].into(),
        );
        let json = serde_json::to_string(&record).expect("serialises");
        let back: RunRecord = serde_json::from_str(&json).expect("parses back");
        assert_eq!(back, record, "serialize then parse is equal");

        let old: RunRecord = serde_json::from_str("{}").expect("an empty line parses");
        assert_eq!(
            old,
            RunRecord::default(),
            "an empty line parses to the default record"
        );
    }

    #[test]
    fn a_run_record_without_tool_calls_has_null_rates() {
        let record = RunRecord::new(
            "6ac1a6b2e5f60701".to_string(),
            "chat".to_string(),
            &RunState::Complete,
            &RunMetrics::default(),
            1.0,
            BTreeMap::new(),
        );
        let json = serde_json::to_string(&record).expect("serialises");
        assert!(
            json.contains(r#""tool_success_rate":null"#),
            "no tool calls must read null, not 0.0: {json}"
        );
        assert!(
            json.contains(r#""parse_failure_rate":null"#),
            "no tool calls must read null, not 0.0: {json}"
        );
    }

    #[test]
    fn budget_exceeded_names_its_dimension_in_a_record() {
        let state = RunState::BudgetExceeded(Briefing::new(
            "g",
            None,
            vec![],
            vec![],
            None,
            Some(crate::daemon::run::BudgetDimension::Turns),
        ));
        let record = RunRecord::new(
            "6ac1a6b2e5f60701".to_string(),
            "ghost".to_string(),
            &state,
            &RunMetrics::default(),
            5.0,
            BTreeMap::new(),
        );
        assert_eq!(record.status, RunStatus::BudgetExceeded);
        assert_eq!(record.failure_class, FailureClass::BudgetExceeded);
        assert!(!record.is_model_attributable);
    }
}
