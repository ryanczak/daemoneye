//! The resume seed (M24 phase 09a): what a *resumed* run starts from. Given
//! a finished run's id and a fresh human directive, the text a new run is
//! seeded with is the kind the run began as, the terminal [`Briefing`] it
//! ended with when it has one, and the directive — design § 2.3,
//! "briefing-seeded resume, not transcript rehydration". The seed is built
//! from the log's structure (first `RunStarted`, last `RunFinished`), never
//! by replaying turns.
//!
//! Pure split: [`terminal_outcome`] reads a slice of records with no I/O,
//! [`render_seed`] is pure text over a kind/briefing/directive, and
//! [`resume_seed`] is the only place that touches a turn log. The CLI, IPC
//! and re-spawn half is phase-09b; nothing in production calls
//! [`resume_seed`] yet.

use super::{Briefing, RunId, RunKind, RunState, TurnLogEvent, TurnLogRecord, read_turn_log};

/// Why a run cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeError {
    /// No turn log exists for this id, or it holds no records.
    NoRunLog,
    /// The log has no `RunFinished` — the run is still in flight.
    StillRunning,
}

impl std::fmt::Display for ResumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResumeError::NoRunLog => write!(f, "no run log for that id"),
            ResumeError::StillRunning => write!(f, "that run is still running; stop it first"),
        }
    }
}

impl std::error::Error for ResumeError {}

/// The kind a run started with and the briefing it ended with, read from its
/// turn-log records. Pure — takes the records, does no I/O; the records come
/// from the turn log, fetched by [`resume_seed`].
///
/// A run that ended `HardFail` or `BudgetExceeded` yields the briefing it
/// carried out; `Complete` and `Cancelled { .. }` carry none, and are still
/// resumable — the directive alone is then the instruction.
pub fn terminal_outcome(
    records: &[TurnLogRecord],
) -> Result<(RunKind, Option<Briefing>), ResumeError> {
    let kind = records
        .iter()
        .find_map(|r| match &r.event {
            TurnLogEvent::RunStarted { kind } => Some(kind.clone()),
            _ => None,
        })
        .ok_or(ResumeError::NoRunLog)?;
    let ended = records
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            TurnLogEvent::RunFinished { state } => Some(state),
            _ => None,
        })
        .ok_or(ResumeError::StillRunning)?;
    let briefing = match ended {
        RunState::HardFail(b) | RunState::BudgetExceeded(b) => Some(b.clone()),
        RunState::Complete | RunState::Cancelled { .. } | RunState::Running => None,
    };
    Ok((kind, briefing))
}

/// The text a resumed run starts from: what it was, what it learned, and what
/// it is now being told to do.
///
/// Takes no transcript. Resume seeds from the briefing and the directive —
/// design § 2.3, "not transcript rehydration" — and this signature is what
/// makes that structural rather than aspirational: there is no record
/// argument to reach for.
pub fn render_seed(kind: &RunKind, briefing: Option<&Briefing>, directive: &str) -> String {
    let mut out = String::new();
    out.push_str("# Resumed run\n\n");
    let identifying = match kind {
        RunKind::Chat { session } => format!("session {session}"),
        RunKind::Ghost { runbook, trigger } => format!("runbook {runbook} (trigger: {trigger})"),
        RunKind::Scheduled { job } => format!("job {job}"),
        RunKind::Delegated { agent, parent } => format!("agent {agent} (parent run {parent})"),
    };
    out.push_str(&format!(
        "this run resumes a {} run — {identifying}\n\n",
        kind.label()
    ));
    out.push_str("# Prior briefing\n\n");
    match briefing {
        Some(b) => out.push_str(&b.render()),
        None => out.push_str("(no briefing — the run ended without one)\n\n"),
    }
    out.push_str("# Directive\n\n");
    match directive {
        "" => out.push_str("(none given)\n"),
        text => {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Read a run's turn log and build its resume seed.
pub fn resume_seed(run_id: &RunId, directive: &str) -> Result<String, ResumeError> {
    let records = read_turn_log(run_id);
    let (kind, briefing) = terminal_outcome(&records)?;
    Ok(render_seed(&kind, briefing.as_ref(), directive))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
    use crate::daemon::run::{BudgetDimension, TurnLogEvent, append_turn_event};

    fn record(id: &RunId, event: TurnLogEvent) -> TurnLogRecord {
        TurnLogRecord {
            timestamp: "2026-09-12T14:00:00Z".to_string(),
            run_id: id.clone(),
            event,
        }
    }

    fn started(id: &RunId, kind: RunKind) -> TurnLogRecord {
        record(id, TurnLogEvent::RunStarted { kind })
    }

    fn finished(id: &RunId, state: RunState) -> TurnLogRecord {
        record(id, TurnLogEvent::RunFinished { state })
    }

    fn turn(id: &RunId, n: u32) -> TurnLogRecord {
        record(id, TurnLogEvent::TurnStarted { turn: n })
    }

    fn note(id: &RunId, text: &str) -> TurnLogRecord {
        record(id, TurnLogEvent::Note { text: text.into() })
    }

    fn any_id() -> RunId {
        fresh_run_id()
    }

    fn hard_fail_briefing(blocker: &str) -> Briefing {
        Briefing::new(
            "investigate the disk-full alert",
            Some("df reports under 90% used".to_string()),
            vec!["var/log/alerts".to_string()],
            vec!["checked dmesg".to_string(), "tried logrotate".to_string()],
            Some(blocker.to_string()),
            Some(BudgetDimension::Turns),
        )
    }

    fn log(records: Vec<TurnLogRecord>) {
        for record in &records {
            append_turn_event(record);
        }
    }

    #[test]
    fn terminal_outcome_returns_the_kind_and_the_hard_fail_briefing() {
        let kind = RunKind::Ghost {
            runbook: "investigate-disk".into(),
            trigger: "alert".into(),
        };
        let briefing = hard_fail_briefing("logrotate cannot write its state file");
        let outcome = terminal_outcome(&[
            started(&any_id(), kind.clone()),
            turn(&any_id(), 1),
            finished(&any_id(), RunState::HardFail(briefing.clone())),
        ])
        .ok();
        assert_eq!(
            outcome,
            Some((kind, Some(briefing))),
            "a hard-failed run yields its kind and its briefing"
        );
    }

    #[test]
    fn terminal_outcome_returns_the_budget_exceeded_briefing() {
        let kind = RunKind::Scheduled {
            job: "nightly".into(),
        };
        let briefing = hard_fail_briefing("ran out of turns");
        let outcome = terminal_outcome(&[
            started(&any_id(), kind.clone()),
            finished(&any_id(), RunState::BudgetExceeded(briefing.clone())),
        ])
        .ok();
        assert_eq!(
            outcome,
            Some((kind, Some(briefing))),
            "a budget-exceeded run yields its kind and its briefing"
        );
    }

    #[test]
    fn terminal_outcome_on_a_complete_run_has_no_briefing() {
        let kind = RunKind::Scheduled {
            job: "nightly".into(),
        };
        let outcome = terminal_outcome(&[
            started(&any_id(), kind.clone()),
            finished(&any_id(), RunState::Complete),
        ])
        .ok();
        assert_eq!(
            outcome,
            Some((kind, None)),
            "a complete run is resumable on the directive alone"
        );
    }

    #[test]
    fn terminal_outcome_on_a_cancelled_run_has_no_briefing() {
        let kind = RunKind::Chat {
            session: "s1".into(),
        };
        let outcome = terminal_outcome(&[
            started(&any_id(), kind.clone()),
            finished(
                &any_id(),
                RunState::Cancelled {
                    stage: "model_call".into(),
                    turns: 3,
                },
            ),
        ])
        .ok();
        assert_eq!(
            outcome,
            Some((kind, None)),
            "a cancelled run is resumable on the directive alone"
        );
    }

    #[test]
    fn terminal_outcome_on_an_empty_log_is_no_run_log() {
        assert_eq!(
            terminal_outcome(&[]),
            Err(ResumeError::NoRunLog),
            "no records at all cannot name a kind"
        );
    }

    #[test]
    fn terminal_outcome_without_a_run_started_is_no_run_log() {
        let id = any_id();
        let outcome = terminal_outcome(&[turn(&id, 1), note(&id, "half a log")]);
        assert_eq!(
            outcome,
            Err(ResumeError::NoRunLog),
            "a log without its opening record cannot name a kind"
        );
    }

    #[test]
    fn terminal_outcome_without_a_run_finished_is_still_running() {
        let id = any_id();
        let kind = RunKind::Ghost {
            runbook: "investigate-disk".into(),
            trigger: "cron".into(),
        };
        let outcome = terminal_outcome(&[started(&id, kind), turn(&id, 1)]);
        assert_eq!(
            outcome,
            Err(ResumeError::StillRunning),
            "no RunFinished means the run is in flight and not resumable"
        );
    }

    #[test]
    fn terminal_outcome_takes_the_first_start_and_the_last_finish() {
        let id = any_id();
        let first = RunKind::Chat {
            session: "first".into(),
        };
        let second = RunKind::Chat {
            session: "second".into(),
        };
        let briefing = hard_fail_briefing("the last blocker wins");
        let outcome = terminal_outcome(&[
            started(&id, first.clone()),
            finished(&id, RunState::Complete),
            started(&id, second),
            note(&id, "appended again"),
            finished(&id, RunState::HardFail(briefing.clone())),
        ])
        .ok();
        assert_eq!(
            outcome,
            Some((first, Some(briefing))),
            "kind comes from the first RunStarted, briefing from the last RunFinished"
        );
    }

    #[test]
    fn render_seed_embeds_the_briefing_and_the_directive() {
        let kind = RunKind::Ghost {
            runbook: "investigate-disk".into(),
            trigger: "alert".into(),
        };
        let briefing = hard_fail_briefing("logrotate cannot write its state file");
        let rendered = render_seed(&kind, Some(&briefing), "try the other mount");
        assert!(
            rendered.contains("# Directive"),
            "the directive must have its own headed section:\n{rendered}"
        );
        assert!(
            rendered.contains("try the other mount"),
            "the directive text must appear:\n{rendered}"
        );
        assert!(
            rendered.contains("logrotate cannot write its state file"),
            "the briefing's blocker must be carried verbatim:\n{rendered}"
        );
        assert!(
            rendered.contains("# Prior briefing"),
            "the briefing must sit under its own heading:\n{rendered}"
        );
        assert!(
            rendered.contains("ghost") && rendered.contains("runbook investigate-disk"),
            "the resumed run's kind and runbook must be named:\n{rendered}"
        );
    }

    #[test]
    fn render_seed_without_a_briefing_says_so_and_still_carries_the_directive() {
        let kind = RunKind::Scheduled {
            job: "nightly".into(),
        };
        let rendered = render_seed(&kind, None, "check the rotated logs");
        assert!(
            rendered.contains("(no briefing — the run ended without one)"),
            "a None briefing must render the placeholder, not vanish:\n{rendered}"
        );
        assert!(
            rendered.contains("check the rotated logs"),
            "the directive must survive a briefing-less seed:\n{rendered}"
        );
        let empty_directive = render_seed(&kind, None, "");
        assert!(
            empty_directive.contains("(none given)"),
            "an empty directive must still show its section placeholder:\n{empty_directive}"
        );
        assert!(
            empty_directive.contains("# Directive"),
            "the directive heading is emitted even when the directive is empty:\n{empty_directive}"
        );
    }

    #[test]
    fn resume_seed_reads_a_real_log_and_returns_the_seed() {
        let home = HomeGuard::new();
        let id = fresh_run_id();
        let kind = RunKind::Ghost {
            runbook: "investigate-disk".into(),
            trigger: "alert".into(),
        };
        log(vec![
            started(&id, kind),
            turn(&id, 2),
            finished(
                &id,
                RunState::HardFail(hard_fail_briefing("logrotate cannot write its state file")),
            ),
        ]);
        let seed = resume_seed(&id, "try the other mount").ok();
        let expected = hard_fail_briefing("logrotate cannot write its state file");
        assert_eq!(
            seed,
            Some(render_seed(
                &RunKind::Ghost {
                    runbook: "investigate-disk".into(),
                    trigger: "alert".into(),
                },
                Some(&expected),
                "try the other mount",
            )),
            "the I/O wrapper must feed exactly what the pure halves produce"
        );
        assert!(
            seed.as_ref().is_some_and(|s| {
                s.contains("try the other mount")
                    && s.contains("logrotate cannot write its state file")
            }),
            "the seed read back from disk carries the blocker and the directive:\n{seed:?}"
        );
        assert!(
            home.tmp.path().join(".daemoneye/var/log/runs").exists(),
            "the log was written under the guarded temp HOME, not the real one"
        );
    }

    #[test]
    fn resume_seed_reports_still_running_for_a_run_with_no_finish() {
        let _home = HomeGuard::new();
        let id = fresh_run_id();
        log(vec![
            started(
                &id,
                RunKind::Chat {
                    session: "live".into(),
                },
            ),
            turn(&id, 1),
        ]);
        assert_eq!(
            resume_seed(&id, "pick up where you left off"),
            Err(ResumeError::StillRunning),
            "an in-flight run must not be resumable"
        );
    }

    #[test]
    fn resume_seed_reports_no_run_log_for_an_unknown_id() {
        let _home = HomeGuard::new();
        let id = fresh_run_id();
        assert_eq!(
            resume_seed(&id, "anything"),
            Err(ResumeError::NoRunLog),
            "no file means no run, not an empty seed"
        );
    }

    #[test]
    fn resume_error_displays_for_the_cli() {
        assert_eq!(
            ResumeError::NoRunLog.to_string(),
            "no run log for that id",
            "the CLI-facing message for a missing log"
        );
        assert_eq!(
            ResumeError::StillRunning.to_string(),
            "that run is still running; stop it first",
            "the CLI-facing message for an in-flight run"
        );
    }
}
