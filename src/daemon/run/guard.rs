//! The chat turn's run-log guard (M24 phase 06a): writes a run's
//! `RunStarted` on construction and its `RunFinished` on drop, so a chat turn
//! cannot silently leave its turn log open.
//!
//! `run_conversation_loop` is a long function with six exit paths. Rather
//! than trust six hand-placed writes, the guard records whatever outcome was
//! last set via [`RunLogGuard::finish`] and, if none was ever set, records a
//! `HardFail` naming the missed path — a forgotten exit becomes a visible
//! wrong answer in the log instead of a run that appears to never end.
//!
//! Since M30 phase 02 the guard also emits the run's one `event: "run"`
//! record (after the `RunFinished` append), built from the final state, the
//! tally the loop fed, the wall clock it measured at `start` and the tags it
//! carries. A serialization error is logged and skipped, never a panic.

use std::collections::BTreeMap;
use std::time::Instant;

use super::metrics::RunMetrics;
use super::{RunId, RunKind, RunRecord, RunState, TurnLogEvent, TurnLogRecord, append_turn_event};

/// The blocker string a run's log carries when the loop exited without ever
/// recording an outcome. Quoted verbatim in the tests.
const UNRECORDED_BLOCKER: &str = "the run loop exited without recording an outcome";

/// Writes a run's `RunStarted` on construction and its `RunFinished` on drop.
pub struct RunLogGuard {
    run_id: RunId,
    kind_label: &'static str,
    state: Option<RunState>,
    /// What the run's loop tallied so far — tool calls and outcomes, model
    /// calls, tokens, spend. Empty at `start`; fed by the loop's cost sites
    /// and tool dispatches.
    metrics: RunMetrics,
    started: Instant,
    tags: BTreeMap<String, String>,
}

impl RunLogGuard {
    /// Write `RunStarted { kind }` and take ownership of the run's id. The
    /// tags are seeded here: `kind` always, plus `runbook` for `Ghost`,
    /// `agent` for `Delegated` and `job` for `Scheduled`.
    pub fn start(run_id: RunId, kind: RunKind) -> Self {
        let kind_label = kind.label();
        let tags = seed_tags(&kind);
        append_turn_event(&TurnLogRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            run_id: run_id.clone(),
            event: TurnLogEvent::RunStarted { kind },
        });
        RunLogGuard {
            run_id,
            kind_label,
            state: None,
            metrics: RunMetrics::default(),
            started: Instant::now(),
            tags,
        }
    }

    /// Record the outcome this run ended with. Calling it again **replaces**
    /// the recorded outcome: the last call before drop is what is written, so
    /// an inner path may set `Complete` and an outer one override it.
    pub fn finish(&mut self, state: RunState) {
        self.state = Some(state);
    }

    /// Add one tag to the record. Re-calling with the same key overwrites.
    pub fn tag(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.tags.insert(key.into(), value.into());
    }

    /// The run id whose turn log this guard owns.
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// What the run has tallied so far.
    pub fn metrics(&self) -> &RunMetrics {
        &self.metrics
    }

    /// What the tally to feed — model-call sites record through it.
    pub fn metrics_mut(&mut self) -> &mut RunMetrics {
        &mut self.metrics
    }

    /// Seconds since the run started — the record's `wall_clock_s`.
    pub fn elapsed_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}
/// The tags a run's kind seeds: `kind` always, plus the kind's own identity
/// field — `runbook` for a ghost, `agent` for a delegated run, `job` for a
/// scheduled one. A chat run carries no other seed.
fn seed_tags(kind: &RunKind) -> BTreeMap<String, String> {
    let mut tags = BTreeMap::new();
    tags.insert("kind".to_string(), kind.label().to_string());
    match kind {
        RunKind::Ghost { runbook, .. } => {
            tags.insert("runbook".to_string(), runbook.clone());
        }
        RunKind::Delegated { agent, .. } => {
            tags.insert("agent".to_string(), agent.clone());
        }
        RunKind::Scheduled { job } => {
            tags.insert("job".to_string(), job.clone());
        }
        RunKind::Chat { .. } => {}
    }
    tags
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
            event: TurnLogEvent::RunFinished {
                state: state.clone(),
            },
        });
        let wall_clock_s = self.started.elapsed().as_secs_f64();
        let record = RunRecord::new(
            self.run_id.to_string(),
            self.kind_label.to_string(),
            &state,
            &self.metrics,
            wall_clock_s,
            self.tags.clone(),
        );
        match serde_json::to_value(&record) {
            Ok(value) => crate::daemon::utils::log_event("run", value),
            Err(e) => {
                log::warn!(
                    "run {}: failed to serialize the run record, skipping it: {}",
                    self.run_id,
                    e
                );
            }
        }
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

    /// The `event == "run"` lines in today's events segment.
    fn run_event_lines() -> Vec<serde_json::Value> {
        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).unwrap_or_default();
        content
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|v| v.get("event").and_then(|e| e.as_str()) == Some("run"))
            .collect()
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
                other => panic!("expected RunFinished, got {other:?}"),
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

    #[test]
    fn guard_holds_the_metrics_recorded_through_it() {
        let _home = HomeGuard::new();
        let id = run_id();
        let mut guard = RunLogGuard::start(id.clone(), chat_kind());
        assert_eq!(
            guard.metrics().tool_calls,
            0,
            "a fresh guard's tally is empty"
        );
        assert_eq!(guard.metrics().model_calls, 0, "no model calls yet");
        assert_eq!(guard.metrics().tool_success_rate(), None, "no rates yet");
        guard.metrics_mut().tool_calls = 3;
        guard.metrics_mut().tool_successes = 2;
        guard.metrics_mut().parse_failures = 1;
        guard.metrics_mut().record_model_call(
            "anthropic",
            "claude-sonnet-4-6",
            &crate::ai::TokenBreakdown {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_write_tokens: 5,
            },
            0.01,
        );
        assert_eq!(
            guard.metrics().tool_calls,
            3,
            "the tally lands on the guard"
        );
        assert_eq!(
            guard.metrics().tool_success_rate(),
            Some(2.0 / 3.0),
            "the rates read back off the guard"
        );
        assert_eq!(guard.metrics().parse_failure_rate(), Some(1.0 / 3.0));
        assert_eq!(guard.metrics().model_calls, 1, "the model call is recorded");
        assert_eq!(
            guard.metrics().tokens.input_tokens,
            100,
            "the token buckets sum through the guard"
        );
        assert_eq!(guard.metrics().provider.as_deref(), Some("anthropic"));
        assert_eq!(guard.metrics().model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn guard_drop_emits_one_run_event() {
        let _home = HomeGuard::new();
        let id = RunId::parse("6ac1a6b2e5f60704").unwrap();
        {
            let mut guard = RunLogGuard::start(
                id.clone(),
                RunKind::Ghost {
                    runbook: "investigate-disk".into(),
                    trigger: "webhook".into(),
                },
            );
            guard.tag("agent", "analyst");
            guard.tag("host", "db01");
            guard
                .metrics_mut()
                .set_model("anthropic", "claude-sonnet-4-6");
            guard.metrics_mut().tool_calls = 2;
            guard.metrics_mut().tool_successes = 1;
            guard.finish(RunState::HardFail(crate::daemon::run::Briefing::new(
                "one ghost run",
                None,
                Vec::new(),
                Vec::new(),
                Some("a tool failed".to_string()),
                None,
            )));
        }
        let lines = run_event_lines();
        assert_eq!(
            lines.len(),
            1,
            "exactly one run event must be emitted, got: {lines:?}"
        );
        let record = &lines[0];
        assert_eq!(
            record.get("run_id").and_then(|r| r.as_str()),
            Some(id.as_str()),
            "the run event names its run id"
        );
        assert_eq!(
            record.get("kind").and_then(|k| k.as_str()),
            Some("ghost"),
            "the kind is the run kind's label"
        );
        assert_eq!(
            record.get("status").and_then(|s| s.as_str()),
            Some("hard_fail"),
            "the status is the terminal state's wire name"
        );
        assert_eq!(
            record.get("failure_class").and_then(|f| f.as_str()),
            Some("tool_errors"),
            "failed tools behind a hard fail classify as tool_errors"
        );
        assert_eq!(
            record.get("agent").and_then(|a| a.as_str()),
            Some("analyst"),
            "the agent tag is the record's agent"
        );
        assert_eq!(
            record.get("model").and_then(|m| m.as_str()),
            Some("claude-sonnet-4-6"),
            "the tally's model is the record's model"
        );
        assert_eq!(
            record.get("provider").and_then(|m| m.as_str()),
            Some("anthropic"),
            "the tally's provider is the record's provider"
        );
        let wall = record
            .get("wall_clock_s")
            .and_then(|w| w.as_f64())
            .expect("the wall clock is a number");
        assert!(wall >= 0.0, "the wall clock must read as elapsed, {wall}");
        let tags = record
            .get("tags")
            .and_then(|t| t.as_object())
            .expect("tags");
        assert_eq!(
            tags.get("kind").and_then(|k| k.as_str()),
            Some("ghost"),
            "kind is seeded at start"
        );
        assert_eq!(
            tags.get("runbook").and_then(|r| r.as_str()),
            Some("investigate-disk"),
            "runbook is seeded from the Ghost kind"
        );
        assert_eq!(tags.get("agent").and_then(|a| a.as_str()), Some("analyst"));
        assert_eq!(tags.get("host").and_then(|h| h.as_str()), Some("db01"));
    }

    #[test]
    fn guard_without_finish_records_class_other() {
        let _home = HomeGuard::new();
        let id = RunId::parse("6ac1a6b2e5f60705").unwrap();
        {
            let _guard = RunLogGuard::start(id.clone(), chat_kind());
        }
        let lines = run_event_lines();
        assert_eq!(lines.len(), 1, "the missed-outcome guard still emits one");
        let record = &lines[0];
        assert_eq!(
            record.get("run_id").and_then(|r| r.as_str()),
            Some(id.as_str())
        );
        assert_eq!(
            record.get("status").and_then(|s| s.as_str()),
            Some("hard_fail"),
            "a missed outcome is a hard fail"
        );
        assert_eq!(
            record.get("failure_class").and_then(|f| f.as_str()),
            Some("other"),
            "a clean tally behind a hard fail is other"
        );
        assert_eq!(
            record
                .get("is_model_attributable")
                .and_then(|a| a.as_bool()),
            Some(false),
            "other is not model-attributable"
        );
        assert_eq!(
            record.get("kind").and_then(|k| k.as_str()),
            Some("chat"),
            "the kind label is still carried"
        );
    }

    #[test]
    fn guard_tags_seed_from_the_kind() {
        let _home = HomeGuard::new();
        for (kind, key, value) in [
            (
                RunKind::Ghost {
                    runbook: "nightly".into(),
                    trigger: "scheduled".into(),
                },
                "runbook",
                "nightly",
            ),
            (
                RunKind::Delegated {
                    agent: "analyst".into(),
                    parent: run_id(),
                },
                "agent",
                "analyst",
            ),
            (
                RunKind::Scheduled {
                    job: "sweep".into(),
                },
                "job",
                "sweep",
            ),
        ] {
            let guard = RunLogGuard::start(RunId::parse("6ac1a6b2e5f60706").unwrap(), kind.clone());
            assert_eq!(
                guard.tags.get("kind").map(String::as_str),
                Some(kind.label()),
                "kind is always seeded"
            );
            assert_eq!(
                guard.tags.get(key).map(String::as_str),
                Some(value),
                "{key} is seeded from {kind:?}"
            );
        }
        let chat = RunLogGuard::start(RunId::parse("6ac1a6b2e5f60706").unwrap(), chat_kind());
        assert_eq!(
            chat.tags.get("kind").map(String::as_str),
            Some("chat"),
            "kind is seeded for a chat run"
        );
        assert_eq!(
            chat.tags.len(),
            1,
            "a chat run carries no other seed tag: {:?}",
            chat.tags
        );
    }
}
