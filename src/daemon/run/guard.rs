//! The chat turn's run-log guard (M24 phase 06a): writes a run's
//! `RunStarted` on construction and its `RunFinished` on drop, so a chat turn
//! cannot silently leave its turn log open.
//!
//! `run_conversation_loop` is a long function with six exit paths. Rather
//! than trust six hand-placed writes, the guard records whatever outcome was
//! last set via [`RunLogGuard::finish`] and, if none was ever set, records a
//! `HardFail` naming the missed path — a forgotten exit becomes a visible
//! wrong answer in the log instead of a run that appears to never end.

use super::{RunId, RunKind, RunState, TurnLogEvent, TurnLogRecord, append_turn_event};

/// The blocker string a run's log carries when the loop exited without ever
/// recording an outcome. Quoted verbatim in the tests.
const UNRECORDED_BLOCKER: &str = "the run loop exited without recording an outcome";

/// Writes a run's `RunStarted` on construction and its `RunFinished` on drop.
pub struct RunLogGuard {
    run_id: RunId,
    kind_label: &'static str,
    state: Option<RunState>,
}

impl RunLogGuard {
    /// Write `RunStarted { kind }` and take ownership of the run's id.
    pub fn start(run_id: RunId, kind: RunKind) -> Self {
        let kind_label = kind.label();
        append_turn_event(&TurnLogRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            run_id: run_id.clone(),
            event: TurnLogEvent::RunStarted { kind },
        });
        RunLogGuard {
            run_id,
            kind_label,
            state: None,
        }
    }

    /// Record the outcome this run ended with. Calling it again **replaces**
    /// the recorded outcome: the last call before drop is what is written, so
    /// an inner path may set `Complete` and an outer one override it.
    pub fn finish(&mut self, state: RunState) {
        self.state = Some(state);
    }

    /// The run id whose turn log this guard owns.
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

impl Drop for RunLogGuard {
    fn drop(&mut self) {
        let label = self.kind_label;
        let state = self.state.take().unwrap_or_else(|| {
            RunState::HardFail(super::Briefing::new(
                format!("one {label} run"),
                None,
                Vec::new(),
                Vec::new(),
                Some(UNRECORDED_BLOCKER.to_string()),
                None,
            ))
        });
        append_turn_event(&TurnLogRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            run_id: self.run_id.clone(),
            event: TurnLogEvent::RunFinished { state },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::{read_turn_log, turn_log_file};

    /// RAII-guarded HOME pointing at a fresh tempdir, so a test that writes a
    /// turn log never touches the developer's real `~/.daemoneye`. Same shape
    /// as `run::cancel`'s `HomeGuard`.
    struct HomeGuard {
        _lock: crate::TestHomeGuard,
        #[expect(dead_code)]
        tmp: tempfile::TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let _lock = crate::test_home_guard();
            let tmp = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", tmp.path()) };
            HomeGuard { _lock, tmp }
        }
    }

    /// A fixed run id inside the guard tests: it must be a valid id (16 hex
    /// chars) and unique per test so a shared file cannot alias assertions.
    const RUN_ID_STR: &str = "6ac1a6b2e5f60701";

    fn run_id() -> RunId {
        RunId::parse(RUN_ID_STR).unwrap()
    }

    fn chat_kind() -> RunKind {
        RunKind::Chat {
            session: "sess-1".into(),
        }
    }

    #[test]
    fn guard_start_writes_run_started_with_the_kind() {
        let _home = HomeGuard::new();
        let id = run_id();
        let _guard = RunLogGuard::start(id.clone(), chat_kind());
        let records = read_turn_log(&id);
        assert_eq!(
            records.len(),
            1,
            "one start call must be exactly one record so far"
        );
        match &records[0].event {
            TurnLogEvent::RunStarted { kind } => assert_eq!(
                kind,
                &RunKind::Chat {
                    session: "sess-1".into()
                },
                "the kind round-trips verbatim"
            ),
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn guard_drop_after_finish_writes_that_state() {
        let _home = HomeGuard::new();
        let id = run_id();
        {
            let mut guard = RunLogGuard::start(id.clone(), chat_kind());
            guard.finish(RunState::Complete);
        }
        let records = read_turn_log(&id);
        assert_eq!(records.len(), 2, "start + finish, nothing more");
        match &records[1].event {
            TurnLogEvent::RunFinished { state } => {
                assert_eq!(state, &RunState::Complete, "the recorded state is written")
            }
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }

    #[test]
    fn guard_drop_without_finish_records_the_missed_outcome() {
        let _home = HomeGuard::new();
        let id = run_id();
        {
            let _guard = RunLogGuard::start(id.clone(), chat_kind());
        }
        let records = read_turn_log(&id);
        assert_eq!(
            records.len(),
            2,
            "a guard with no finish call must still write start and finish"
        );
        let last = records.last().expect("a finish record must exist at all");
        match &last.event {
            TurnLogEvent::RunFinished { state } => match state {
                RunState::HardFail(b) => {
                    assert_eq!(
                        b.blocker.as_deref(),
                        Some("the run loop exited without recording an outcome"),
                        "records the missed outcome"
                    );
                }
                other => panic!("expected HardFail for the missed path, got {other:?}"),
            },
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }

    #[test]
    fn guard_writes_exactly_one_run_finished() {
        let _home = HomeGuard::new();
        let id = run_id();
        {
            let mut guard = RunLogGuard::start(id.clone(), chat_kind());
            guard.finish(RunState::Complete);
            guard.finish(RunState::Running);
            guard.finish(RunState::Cancelled {
                stage: "x".into(),
                turns: 0,
            });
        }
        let records = read_turn_log(&id);
        let finishes = records
            .iter()
            .filter(|r| matches!(r.event, TurnLogEvent::RunFinished { .. }))
            .count();
        assert_eq!(
            finishes, 1,
            "exactly one RunFinished must be written, not one per finish call"
        );
    }

    #[test]
    fn guard_last_finish_wins() {
        let _home = HomeGuard::new();
        let id = run_id();
        {
            let mut guard = RunLogGuard::start(id.clone(), chat_kind());
            guard.finish(RunState::Complete);
            guard.finish(RunState::Cancelled {
                stage: "awaiting_model".into(),
                turns: 2,
            });
        }
        let records = read_turn_log(&id);
        let last = records.last().unwrap();
        match &last.event {
            TurnLogEvent::RunFinished { state } => {
                assert_eq!(
                    state,
                    &RunState::Cancelled {
                        stage: "awaiting_model".into(),
                        turns: 2
                    },
                    "the last finish wins, not the first"
                );
            }
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }

    #[test]
    fn guard_records_a_cancelled_state_with_its_stage_and_turns() {
        let _home = HomeGuard::new();
        let id = run_id();
        {
            let mut guard = RunLogGuard::start(id.clone(), chat_kind());
            guard.finish(RunState::Cancelled {
                stage: "awaiting_model".into(),
                turns: 3,
            });
        }
        let records = read_turn_log(&id);
        let last = records.last().unwrap();
        match &last.event {
            TurnLogEvent::RunFinished { state } => match state {
                RunState::Cancelled { stage, turns } => {
                    assert_eq!(stage, "awaiting_model", "the stage name round-trips");
                    assert_eq!(*turns, 3, "the turn count round-trips");
                }
                other => panic!("expected Cancelled, got {other:?}"),
            },
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }

    #[test]
    fn guard_log_lands_under_the_runs_dir_for_its_id() {
        let _home = HomeGuard::new();
        let id = run_id();
        {
            let guard = RunLogGuard::start(id.clone(), chat_kind());
            let path = turn_log_file(guard.run_id());
            assert!(
                path.starts_with(crate::config::runs_dir()),
                "the log must sit under runs_dir, got {path:?}"
            );
            std::fs::write(&path, "seed line for the existence check\n").unwrap();
            assert!(
                path.exists(),
                "the file for *this* run id must exist, got {path:?}"
            );
        }
        let other = RunId::parse("6ac1a6b2e5f60702").unwrap();
        assert!(
            !turn_log_file(&other).exists(),
            "no file must be created for any run id other than the guard's"
        );
    }

    #[test]
    fn guard_missed_exit_goal_names_the_run_kind() {
        let _home = HomeGuard::new();
        let id = RunId::parse("6ac1a6b2e5f60703").unwrap();
        {
            let _guard = RunLogGuard::start(
                id.clone(),
                RunKind::Ghost {
                    runbook: "investigate-disk".into(),
                    trigger: "webhook".into(),
                },
            );
        }
        let records = read_turn_log(&id);
        let last = records.last().expect("a finish record must exist at all");
        match &last.event {
            TurnLogEvent::RunFinished { state } => match state {
                RunState::HardFail(b) => {
                    assert_eq!(
                        b.goal, "one ghost run",
                        "the missed-exit goal must name the run kind, not hardcode chat"
                    );
                    assert_eq!(
                        b.blocker.as_deref(),
                        Some("the run loop exited without recording an outcome"),
                        "the blocker string is unchanged by the kind-aware goal"
                    );
                }
                other => panic!("expected HardFail for the missed path, got {other:?}"),
            },
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }
}
