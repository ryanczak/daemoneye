//! The structured value a finished run hands back (M24 phase 03): terminal
//! status, the four wire strings, a `Cancellation` record, the bounded
//! briefing render, and `RunResult` with `from_state`. Pure data — no I/O, no
//! clock, and no live caller yet (phases 05–08 construct it from a run).
//!
//! Ported from the rexyMCP anchors (`executor/src/phase/result.rs` and
//! `phase/briefing.rs`) in shape only: no MCP-boundary schema derives, no
//! phase artifacts, and every wire name spelled explicitly rather than via
//! `rename_all`.

use anyhow::bail;
use serde::{Deserialize, Serialize};

use super::{Briefing, Budget, RunId, RunKind, RunState};

/// Terminal status of a [`RunResult`]. The four wire strings are the phase's
/// contract; each name is spelled explicitly so a Rust rename cannot move the
/// wire.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    #[serde(rename = "complete")]
    Complete,
    #[serde(rename = "hard_fail")]
    HardFail,
    #[serde(rename = "budget_exceeded")]
    BudgetExceeded,
    #[serde(rename = "cancelled")]
    Cancelled,
}

impl RunStatus {
    /// The wire string for this status — the same string serde emits.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Complete => "complete",
            RunStatus::HardFail => "hard_fail",
            RunStatus::BudgetExceeded => "budget_exceeded",
            RunStatus::Cancelled => "cancelled",
        }
    }

    /// The terminal status a [`RunState`] maps to, or `None` for `Running` —
    /// a running run has no result. Total match, no `_` arm: adding a
    /// `RunState` variant is a compile error here rather than a silently
    /// wrong status.
    pub fn for_state(state: &RunState) -> Option<RunStatus> {
        match state {
            RunState::Running => None,
            RunState::Complete => Some(RunStatus::Complete),
            RunState::HardFail(_) => Some(RunStatus::HardFail),
            RunState::BudgetExceeded(_) => Some(RunStatus::BudgetExceeded),
            RunState::Cancelled { .. } => Some(RunStatus::Cancelled),
        }
    }
}

/// Who cancelled the run. Set by the caller layer (phase-04's stop path); the
/// run's own `from_state` mapping leaves it `None`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum CancelReason {
    #[serde(rename = "user_stop")]
    UserStop,
    #[serde(rename = "claude_stop")]
    ClaudeStop,
}

/// Cancellation details recorded when a run is aborted mid-run, mapped from
/// `RunState::Cancelled { stage, turns }`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Cancellation {
    /// Who cancelled. `None` here — phase-04's stop path sets it when it
    /// cancels a run; the run's own terminal state does not carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<CancelReason>,
    /// Where in the turn cycle cancellation was observed (e.g.
    /// `awaiting_model`).
    pub stage: String,
    /// Turns fully completed before cancellation — the same number
    /// `RunState::Cancelled.turns` carries.
    pub turns_done: u32,
}

/// The one structured value a finished run hands to whoever was waiting on
/// it: the ghost mailbox, the catch-up brief, a future `Response::RunFinished`,
/// `daemoneye runs`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub run_id: RunId,
    pub kind: RunKind,
    pub status: RunStatus,
    /// The run's final answer text. Empty on the failure paths until a later
    /// phase populates it.
    pub summary: String,
    /// Turns the run completed.
    pub turns: u32,
    pub budget: Budget,
    /// Present iff the run hard-failed or exhausted its budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// Present iff the run was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation: Option<Cancellation>,
}

impl RunResult {
    /// A run that finished under budget, carrying its final answer.
    pub fn complete(
        run_id: RunId,
        kind: RunKind,
        summary: String,
        turns: u32,
        budget: Budget,
    ) -> Self {
        RunResult::assemble(
            run_id,
            kind,
            RunStatus::Complete,
            summary,
            turns,
            budget,
            None,
            None,
        )
    }

    /// A run that could not finish, carrying the bounded briefing it ended
    /// with and no cancellation.
    pub fn hard_fail(
        run_id: RunId,
        kind: RunKind,
        briefing: Briefing,
        turns: u32,
        budget: Budget,
    ) -> Self {
        RunResult::assemble(
            run_id,
            kind,
            RunStatus::HardFail,
            String::new(),
            turns,
            budget,
            Some(briefing),
            None,
        )
    }

    /// A run that ran out of budget, carrying the bounded briefing it ended
    /// with and no cancellation.
    pub fn budget_exceeded(
        run_id: RunId,
        kind: RunKind,
        briefing: Briefing,
        turns: u32,
        budget: Budget,
    ) -> Self {
        RunResult::assemble(
            run_id,
            kind,
            RunStatus::BudgetExceeded,
            String::new(),
            turns,
            budget,
            Some(briefing),
            None,
        )
    }

    /// A run that was stopped before it finished, carrying the cancellation
    /// record (with `reason` still `None` until phase-04 sets it) and no
    /// briefing. `turns` is the run's own completed-turn count;
    /// `cancellation.turns_done` is what the terminal state recorded and can
    /// trail it.
    pub fn cancelled(
        run_id: RunId,
        kind: RunKind,
        cancellation: Cancellation,
        turns: u32,
        budget: Budget,
    ) -> Self {
        RunResult::assemble(
            run_id,
            kind,
            RunStatus::Cancelled,
            String::new(),
            turns,
            budget,
            None,
            Some(cancellation),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        run_id: RunId,
        kind: RunKind,
        status: RunStatus,
        summary: String,
        turns: u32,
        budget: Budget,
        briefing: Option<Briefing>,
        cancellation: Option<Cancellation>,
    ) -> Self {
        RunResult {
            run_id,
            kind,
            status,
            summary,
            turns,
            budget,
            briefing,
            cancellation,
        }
    }

    /// Build the outward-facing result from a run's terminal state, so the
    /// two can never disagree. `Running` is not a result: it errors. The
    /// briefing/cancellation pairing is decided here, not by the caller.
    pub fn from_state(
        run_id: RunId,
        kind: RunKind,
        state: RunState,
        summary: String,
        turns: u32,
        budget: Budget,
    ) -> anyhow::Result<RunResult> {
        match state {
            RunState::Running => bail!("Run {run_id} is still running; there is no result yet"),
            RunState::Complete => Ok(RunResult::complete(run_id, kind, summary, turns, budget)),
            RunState::HardFail(briefing) => {
                Ok(RunResult::hard_fail(run_id, kind, briefing, turns, budget))
            }
            RunState::BudgetExceeded(briefing) => Ok(RunResult::budget_exceeded(
                run_id, kind, briefing, turns, budget,
            )),
            RunState::Cancelled { stage, turns: t } => Ok(RunResult::cancelled(
                run_id,
                kind,
                Cancellation {
                    reason: None,
                    stage,
                    turns_done: t,
                },
                turns,
                budget,
            )),
        }
    }
}

impl Briefing {
    /// Render the canonical six-section markdown whoever reads a failure
    /// brief sees. Every section is emitted, headed, in order — an empty
    /// section renders its explicit `(none)` placeholder rather than
    /// vanishing, so it stays distinguishable from a section that was never
    /// written.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# Goal\n\n");
        push_block(&mut out, &self.goal);
        out.push_str("# Criteria\n\n");
        match &self.criteria {
            Some(text) => push_block(&mut out, text),
            None => out.push_str("(none)\n\n"),
        }
        out.push_str("# Files in the working set\n\n");
        if self.working_files.is_empty() {
            out.push_str("(none)\n\n");
        } else {
            for file in &self.working_files {
                out.push_str(&format!("- {file}\n"));
            }
            out.push('\n');
        }
        out.push_str("# What was tried\n\n");
        if self.attempts.is_empty() {
            out.push_str("(no prior attempts)\n\n");
        } else {
            for (i, attempt) in self.attempts.iter().enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, attempt));
            }
            out.push('\n');
        }
        out.push_str("# Current blocker\n\n");
        match &self.blocker {
            Some(text) => push_block(&mut out, text),
            None => out.push_str("(none)\n\n"),
        }
        out.push_str("# Budget\n\n");
        match self.budget_left {
            Some(dim) => out.push_str(&format!("ended on: {}\n\n", dim.label())),
            None => out.push_str("(within budget)\n\n"),
        }
        out
    }
}

/// Append `body` as its own blank-line-delimited block, guaranteeing exactly
/// one trailing newline before the blank line whatever `body` ended with.
fn push_block(out: &mut String, body: &str) {
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::BudgetDimension;

    const RUN_ID_STR: &str = "2190b361c94e7a45";

    fn run_id() -> RunId {
        RunId::parse(RUN_ID_STR).expect("fixture id is valid hex")
    }

    fn sample_briefing() -> Briefing {
        Briefing::new(
            "investigate the disk-full alert",
            Some("df reports under 90% used".to_string()),
            vec!["src/main.rs".to_string(), "var/log/alerts".to_string()],
            vec!["checked dmesg".to_string(), "tried logrotate".to_string()],
            Some("logrotate cannot write its state file".to_string()),
            Some(BudgetDimension::Turns),
        )
    }

    #[test]
    fn run_status_serialises_to_the_four_contract_strings() {
        for (status, wire) in [
            (RunStatus::Complete, "complete"),
            (RunStatus::HardFail, "hard_fail"),
            (RunStatus::BudgetExceeded, "budget_exceeded"),
            (RunStatus::Cancelled, "cancelled"),
        ] {
            assert_eq!(status.as_str(), wire, "as_str must be the contract string");
            let json = serde_json::to_string(&status).expect("status serialises");
            assert_eq!(json, format!("\"{wire}\""), "wire string must be exact");
            let back: RunStatus = serde_json::from_str(&json).expect("round-trips");
            assert_eq!(back, status, "round-trip must agree");
        }
    }

    #[test]
    fn run_status_for_state_maps_every_terminal_state() {
        let briefing = sample_briefing();
        assert_eq!(
            RunStatus::for_state(&RunState::Complete),
            Some(RunStatus::Complete)
        );
        assert_eq!(
            RunStatus::for_state(&RunState::HardFail(briefing.clone())),
            Some(RunStatus::HardFail)
        );
        assert_eq!(
            RunStatus::for_state(&RunState::BudgetExceeded(briefing)),
            Some(RunStatus::BudgetExceeded)
        );
        assert_eq!(
            RunStatus::for_state(&RunState::Cancelled {
                stage: "model_call".into(),
                turns: 3
            }),
            Some(RunStatus::Cancelled)
        );
    }

    #[test]
    fn run_status_for_state_returns_none_for_running() {
        assert_eq!(RunStatus::for_state(&RunState::Running), None);
    }

    #[test]
    fn cancel_reason_serialises_to_user_stop_and_claude_stop() {
        let user = serde_json::to_string(&CancelReason::UserStop).expect("serialises");
        let claude = serde_json::to_string(&CancelReason::ClaudeStop).expect("serialises");
        assert_eq!(user, "\"user_stop\"");
        assert_eq!(claude, "\"claude_stop\"");
        let user_back: CancelReason = serde_json::from_str(&user).expect("round-trips");
        let claude_back: CancelReason = serde_json::from_str(&claude).expect("round-trips");
        assert_eq!(user_back, CancelReason::UserStop);
        assert_eq!(claude_back, CancelReason::ClaudeStop);
    }

    #[test]
    fn cancellation_omits_a_none_reason_from_json() {
        let cancellation = Cancellation {
            reason: None,
            stage: "awaiting_model".into(),
            turns_done: 7,
        };
        let json = serde_json::to_value(&cancellation).expect("serialises to JSON");
        assert!(
            json.get("reason").is_none(),
            "missing reason must not appear as a key: {json}"
        );
        assert_eq!(json["stage"], "awaiting_model");
        assert_eq!(json["turns_done"], 7);
        let back: Cancellation = serde_json::from_value(json).expect("deserialises");
        assert_eq!(back, cancellation, "round-trips without the reason key");
        let with_reason = Cancellation {
            reason: Some(CancelReason::UserStop),
            stage: "stopped".into(),
            turns_done: 2,
        };
        let json = serde_json::to_value(&with_reason).expect("serialises to JSON");
        assert_eq!(
            json["reason"], "user_stop",
            "a present reason must still serialise: {json}"
        );
    }

    #[test]
    fn run_result_from_state_errors_on_running() {
        let err = RunResult::from_state(
            run_id(),
            RunKind::Scheduled {
                job: "nightly".into(),
            },
            RunState::Running,
            String::new(),
            0,
            Budget::unlimited(),
        )
        .expect_err("Running must be an error, not a result");
        let message = err.to_string();
        assert!(
            message.contains(RUN_ID_STR),
            "the message must name the run id: {message}"
        );
    }

    #[test]
    fn run_result_complete_carries_neither_briefing_nor_cancellation() {
        let budget = Budget::from_limits(3, 0, 0, 0, 0);
        let result = RunResult::from_state(
            run_id(),
            RunKind::Scheduled {
                job: "nightly".into(),
            },
            RunState::Complete,
            "the disk recovered".to_string(),
            3,
            budget,
        )
        .expect("complete is a result");
        assert_eq!(result.status, RunStatus::Complete);
        assert_eq!(result.summary, "the disk recovered");
        assert_eq!(result.turns, 3);
        assert_eq!(result.briefing, None, "complete must carry no briefing");
        assert_eq!(
            result.cancellation, None,
            "complete must carry no cancellation"
        );
    }

    #[test]
    fn run_result_hard_fail_carries_the_briefing_and_no_cancellation() {
        let briefing = sample_briefing();
        let result = RunResult::from_state(
            run_id(),
            RunKind::Ghost {
                runbook: "disk".into(),
                trigger: "alert".into(),
            },
            RunState::HardFail(briefing.clone()),
            String::new(),
            4,
            Budget::unlimited(),
        )
        .expect("hard fail is a result");
        assert_eq!(result.status, RunStatus::HardFail);
        assert_eq!(
            result.briefing,
            Some(briefing),
            "the briefing must survive the mapping with its goal intact"
        );
        assert_eq!(
            result.briefing.as_ref().expect("briefing").goal,
            "investigate the disk-full alert"
        );
        assert_eq!(result.cancellation, None, "a hard fail is not a cancel");
        assert_eq!(result.turns, 4);
    }

    #[test]
    fn run_result_cancelled_carries_stage_and_turns_but_no_briefing() {
        let result = RunResult::from_state(
            run_id(),
            RunKind::Chat {
                session: "s1".into(),
            },
            RunState::Cancelled {
                stage: "awaiting_model".into(),
                turns: 7,
            },
            String::new(),
            9,
            Budget::unlimited(),
        )
        .expect("cancelled is a result");
        assert_eq!(result.status, RunStatus::Cancelled);
        assert_eq!(
            result.turns, 9,
            "the result's own turns comes from the caller, not the state"
        );
        assert_eq!(
            result.cancellation,
            Some(Cancellation {
                reason: None,
                stage: "awaiting_model".into(),
                turns_done: 7,
            }),
            "RunState::Cancelled's stage/turns map onto Cancellation"
        );
        assert_eq!(
            result.briefing, None,
            "a cancelled run did not fail, so it carries no briefing"
        );
    }

    #[test]
    fn briefing_render_emits_all_six_sections_in_order() {
        let out = sample_briefing().render();
        let goal = out.find("# Goal\n\n").expect("goal heading present");
        let criteria = out
            .find("# Criteria\n\n")
            .expect("criteria heading present");
        let files = out
            .find("# Files in the working set\n\n")
            .expect("files heading present");
        let tried = out
            .find("# What was tried\n\n")
            .expect("what-was-tried heading present");
        let blocker = out
            .find("# Current blocker\n\n")
            .expect("blocker heading present");
        let budget = out.find("# Budget\n\n").expect("budget heading present");
        assert!(
            goal < criteria
                && criteria < files
                && files < tried
                && tried < blocker
                && blocker < budget,
            "the six headings must appear in contract order"
        );
        assert!(out.contains("investigate the disk-full alert"));
        assert!(out.contains("df reports under 90% used"));
        assert!(out.contains("- src/main.rs"));
        assert!(out.contains("- var/log/alerts"));
        assert!(out.contains("1. checked dmesg"), "attempts are numbered");
        assert!(out.contains("2. tried logrotate"));
        assert!(out.contains("logrotate cannot write its state file"));
        assert!(
            out.contains("ended on: turns"),
            "budget_left names the dimension, never an amount: {out}"
        );
        assert!(
            !out.contains("(none)"),
            "a full briefing renders every section's real body"
        );
        assert!(
            !out.contains("(no prior attempts)") && !out.contains("(within budget)"),
            "no placeholder leaks into a populated section"
        );
    }

    #[test]
    fn briefing_render_marks_empty_sections_explicitly() {
        let out = Briefing::new("the one goal", None, vec![], vec![], None, None).render();
        let mut last = 0usize;
        for heading in [
            "# Goal\n\n",
            "# Criteria\n\n",
            "# Files in the working set\n\n",
            "# What was tried\n\n",
            "# Current blocker\n\n",
            "# Budget\n\n",
        ] {
            let at = out
                .find(heading)
                .unwrap_or_else(|| panic!("heading missing: {heading:?}"));
            assert!(at >= last, "headings must stay in order");
            last = at;
        }
        assert!(out.contains("the one goal"));
        assert!(out.contains("(none)"));
        assert!(out.contains("(no prior attempts)"));
        assert!(out.contains("(within budget)"));
        assert!(
            !out.contains("ended on:"),
            "an unset budget_left must not name a dimension: {out}"
        );
        let criteria_body = out
            .split("# Criteria\n\n")
            .nth(1)
            .expect("criteria section");
        assert!(
            criteria_body.starts_with("(none)"),
            "an empty section renders its placeholder, not blank: {out}"
        );
    }
}
