//! The run listing (M24 phase 10): one [`RunSummary`] per turn log in
//! `var/log/runs/`, rendered as the table `daemoneye run list` and `/runs`
//! print. This is the first reader of that directory — every earlier phase
//! wrote a run's `<id>.jsonl`, and a caller still had to find the id by hand.
//!
//! Pure split, mirroring `resume`'s: [`summarize`] and [`render_runs_table`]
//! touch no filesystem, [`list_runs`] is the only reader (through the
//! phase-02a log reader), and a name that does not parse as a [`RunId`] is
//! skipped rather than reported — the directory exists on a user's disk and
//! may hold anything.

use std::path::PathBuf;
use std::time::SystemTime;

use super::turn_log::{TurnLogEvent, TurnLogRecord};
use super::{RunId, RunKind, RunState};

/// One row of `daemoneye run list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    /// The run the log belongs to, parsed from the filename stem.
    pub id: RunId,
    /// The kind the log's `RunStarted` recorded.
    pub kind: RunKind,
    /// The state the run's last `RunFinished` recorded, or `Running` when it
    /// has none — a run whose log has no finish record is still in flight.
    pub state: RunState,
    /// The highest `TurnStarted` turn the log carries; 0 when none.
    pub turns: u32,
}

/// What the listing says when there is nothing to list.
pub const NO_RUNS_MESSAGE: &str = "no runs recorded yet";

/// Summarise one run's records into a listing row. `None` when the log
/// carries no `RunStarted` — without it there is no kind, and the file is
/// not a run log we can describe. Pure: takes the records, does no I/O, so
/// the caller chooses where they came from.
pub fn summarize(id: &RunId, records: &[TurnLogRecord]) -> Option<RunSummary> {
    let kind = records.iter().find_map(|r| match &r.event {
        TurnLogEvent::RunStarted { kind } => Some(kind.clone()),
        _ => None,
    })?;
    let state = records
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            TurnLogEvent::RunFinished { state } => Some(state.clone()),
            _ => None,
        })
        .unwrap_or(RunState::Running);
    let turns = records
        .iter()
        .filter_map(|r| match &r.event {
            TurnLogEvent::TurnStarted { turn } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    Some(RunSummary {
        id: id.clone(),
        kind,
        state,
        turns,
    })
}

/// Every run log under `var/log/runs/`, newest first by file modification
/// time. An entry whose name is not `<run-id>.jsonl`, an id that does not
/// parse, and a file whose records carry no `RunStarted` are skipped, not
/// reported. A missing directory is an empty listing, not an error, and an
/// entry whose modification time cannot be read sorts last rather than
/// failing the listing.
pub fn list_runs() -> Vec<RunSummary> {
    let Ok(entries) = std::fs::read_dir(crate::config::runs_dir()) else {
        return Vec::new();
    };
    let mut found: Vec<(Option<SystemTime>, RunSummary)> = Vec::new();
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(id) = RunId::parse(stem) else {
            continue;
        };
        let records = super::turn_log::read_turn_log(&id);
        if let Some(summary) = summarize(&id, &records) {
            let modified = entry.metadata().and_then(|m| m.modified()).ok();
            found.push((modified, summary));
        }
    }
    // Newest first; an entry whose mtime is unreadable sorts last rather
    // than failing the listing.
    found.sort_by(|(a_time, a), (b_time, b)| match (a_time, b_time) {
        (Some(a), Some(b)) => b.cmp(a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.as_str().cmp(b.id.as_str()),
    });
    found.into_iter().map(|(_, summary)| summary).collect()
}

/// The table `daemoneye run list` and `/runs` both print. Pure: takes the
/// slice, returns the string, touches no filesystem. An empty listing renders
/// its literal message rather than nothing, because "prints nothing" and
/// "prints an empty table" are both worse for a user who typed the command.
#[must_use]
pub fn render_runs_table(runs: &[RunSummary]) -> String {
    if runs.is_empty() {
        return format!("{NO_RUNS_MESSAGE}\n");
    }
    let mut out = String::from("ID  KIND  STATE  TURNS\n");
    for run in runs {
        out.push_str(&format!(
            "{}  {}  {}  {}\n",
            run.id.as_str(),
            run.kind.label(),
            state_word(&run.state),
            run.turns
        ));
    }
    out
}

/// The short word a row renders a [`RunState`] with.
fn state_word(state: &RunState) -> &'static str {
    match state {
        RunState::Running => "running",
        RunState::Complete => "complete",
        RunState::HardFail(_) => "hard-fail",
        RunState::BudgetExceeded(_) => "budget-exceeded",
        RunState::Cancelled { .. } => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
    use crate::daemon::run::turn_log::append_turn_event;
    use crate::daemon::run::{Briefing, BudgetDimension};

    fn record(id: &RunId, event: TurnLogEvent) -> TurnLogRecord {
        TurnLogRecord {
            timestamp: "2026-09-12T17:00:00Z".to_string(),
            run_id: id.clone(),
            event,
        }
    }

    fn write_log(id: &RunId, events: Vec<TurnLogEvent>) {
        for event in events {
            append_turn_event(&record(id, event));
        }
    }

    /// Wrap one event in a record with a run id, for the pure `summarize`
    /// tests that pass a slice directly.
    fn rec(id: &RunId, event: TurnLogEvent) -> TurnLogRecord {
        record(id, event)
    }

    fn started(kind: RunKind) -> TurnLogEvent {
        TurnLogEvent::RunStarted { kind }
    }

    fn finished(state: RunState) -> TurnLogEvent {
        TurnLogEvent::RunFinished { state }
    }

    fn turn_started(n: u32) -> TurnLogEvent {
        TurnLogEvent::TurnStarted { turn: n }
    }

    fn chat_kind() -> RunKind {
        RunKind::Chat {
            session: "s1".into(),
        }
    }

    fn ghost_kind() -> RunKind {
        RunKind::Ghost {
            runbook: "investigate-disk".into(),
            trigger: "cron".into(),
        }
    }

    fn scheduled_kind() -> RunKind {
        RunKind::Scheduled {
            job: "nightly".into(),
        }
    }

    fn delegated_kind() -> RunKind {
        RunKind::Delegated {
            agent: "triage".into(),
            parent: fresh_run_id(),
        }
    }

    fn sample_briefing() -> Briefing {
        Briefing::new(
            "investigate the disk-full alert",
            Some("df reports under 90% used".to_string()),
            vec!["var/log/alerts".to_string()],
            vec!["checked dmesg".to_string()],
            Some("logrotate cannot write its state file".to_string()),
            Some(BudgetDimension::Turns),
        )
    }

    // ── summarize (pure) ─────────────────────────────────────────────────────

    #[test]
    fn summarize_reads_the_kind_and_the_terminal_state() {
        let id = fresh_run_id();
        let summary = summarize(
            &id,
            &[
                rec(&id, started(ghost_kind())),
                rec(&id, turn_started(1)),
                rec(&id, finished(RunState::HardFail(sample_briefing()))),
            ],
        );
        assert_eq!(
            summary,
            Some(RunSummary {
                id: id.clone(),
                kind: ghost_kind(),
                state: RunState::HardFail(sample_briefing()),
                turns: 1,
            }),
            "the summary must pair the log's kind with its terminal state"
        );
    }

    #[test]
    fn summarize_without_a_run_started_is_none() {
        let id = fresh_run_id();
        assert_eq!(
            summarize(&id, &[rec(&id, turn_started(1)), rec(&id, turn_started(2))]),
            None,
            "a log without its opening record cannot name a kind"
        );
        assert_eq!(summarize(&id, &[]), None, "nor can an empty log");
    }

    #[test]
    fn summarize_without_a_run_finished_reports_running() {
        let id = fresh_run_id();
        let summary = summarize(
            &id,
            &[rec(&id, started(chat_kind())), rec(&id, turn_started(1))],
        );
        assert_eq!(
            summary.map(|s| s.state),
            Some(RunState::Running),
            "a run whose log has no finish record is still in flight"
        );
    }

    #[test]
    fn summarize_takes_the_highest_turn_not_the_count() {
        let id = fresh_run_id();
        let summary = summarize(
            &id,
            &[
                rec(&id, started(scheduled_kind())),
                rec(&id, turn_started(1)),
                rec(&id, turn_started(2)),
                rec(&id, turn_started(5)),
            ],
        );
        assert_eq!(
            summary.map(|s| s.turns),
            Some(5),
            "a log appended to twice must still report the furthest turn reached, not \
             the number of turn_started records (3)"
        );
    }

    // ── render_runs_table (pure) ─────────────────────────────────────────────

    #[test]
    fn render_runs_table_empty_says_no_runs() {
        assert_eq!(
            render_runs_table(&[]),
            format!("{NO_RUNS_MESSAGE}\n"),
            "an empty listing must say so in words, not print nothing"
        );
        assert!(
            render_runs_table(&[]).contains("no runs recorded yet"),
            "the literal message is the phase's contract words"
        );
    }

    #[test]
    fn render_runs_table_lists_every_kind() {
        let summaries: Vec<RunSummary> = [
            RunKind::Chat {
                session: "s1".into(),
            },
            ghost_kind_none(),
            scheduled_kind(),
            delegated_kind(),
        ]
        .into_iter()
        .map(|kind| RunSummary {
            id: fresh_run_id(),
            kind,
            state: RunState::Running,
            turns: 1,
        })
        .collect();
        let rendered = render_runs_table(&summaries);
        assert!(
            rendered.starts_with("ID  KIND  STATE  TURNS\n"),
            "header first: {rendered}"
        );
        for word in ["chat", "ghost", "scheduled", "delegated"] {
            assert!(
                rendered.contains(word),
                "the milestone criterion is that the listing shows all four kinds; \
                 '{word}' is missing:\n{rendered}"
            );
        }
    }

    fn ghost_kind_none() -> RunKind {
        ghost_kind()
    }

    #[test]
    fn render_runs_table_names_each_terminal_state() {
        let briefing = sample_briefing();
        let states: Vec<RunState> = vec![
            RunState::Running,
            RunState::Complete,
            RunState::HardFail(briefing.clone()),
            RunState::BudgetExceeded(briefing),
            RunState::Cancelled {
                stage: "model_call".into(),
                turns: 3,
            },
        ];
        let summaries: Vec<RunSummary> = states
            .into_iter()
            .map(|state| RunSummary {
                id: fresh_run_id(),
                kind: chat_kind(),
                state,
                turns: 2,
            })
            .collect();
        let rendered = render_runs_table(&summaries);
        for word in [
            "running",
            "complete",
            "hard-fail",
            "budget-exceeded",
            "cancelled",
        ] {
            assert!(
                rendered.contains(word),
                "every terminal state must have a human word in the table; \
                 '{word}' is missing:\n{rendered}"
            );
        }
    }

    // ── list_runs (HOME) ─────────────────────────────────────────────────────

    #[test]
    fn list_runs_on_a_missing_directory_is_empty() {
        let _guard = HomeGuard::new();
        assert!(
            !crate::config::runs_dir().exists(),
            "setup sanity: this guarded temp HOME has no var/log/runs yet"
        );
        assert!(
            list_runs().is_empty(),
            "no runs dir must read as an empty listing, not an error"
        );
    }

    #[test]
    fn list_runs_reads_a_written_log() {
        let _guard = HomeGuard::new();
        let id = fresh_run_id();
        write_log(
            &id,
            vec![
                TurnLogEvent::RunStarted { kind: ghost_kind() },
                turn_started(1),
                turn_started(2),
                finished(RunState::Complete),
            ],
        );
        let runs = list_runs();
        let row = runs.iter().find(|s| s.id == id);
        assert_eq!(
            row,
            Some(&RunSummary {
                id: id.clone(),
                kind: ghost_kind(),
                state: RunState::Complete,
                turns: 2,
            }),
            "the summary must be read back from the real log on disk"
        );
    }

    #[test]
    fn list_runs_skips_a_name_that_is_not_a_run_id() {
        let _guard = HomeGuard::new();
        let id = fresh_run_id();
        write_log(
            &id,
            vec![
                TurnLogEvent::RunStarted { kind: chat_kind() },
                turn_started(1),
            ],
        );
        let dir = crate::config::runs_dir();
        std::fs::create_dir_all(&dir).expect("the log write already created the dir");
        std::fs::write(dir.join("notes.txt"), "not a run log\n")
            .expect("a stray file beside the real log");
        std::fs::write(dir.join("not-a-run-id.jsonl"), "{}\n")
            .expect("an unparseable .jsonl beside the real log");
        std::fs::write(dir.join("not-a-run-id.jsonl.tmp"), "{}\n")
            .expect("a temp-suffixed file beside the real log");
        std::fs::create_dir(dir.join("not-a-run-id.dir")).expect("a directory beside the real log");

        let runs = list_runs();
        let ids: Vec<&str> = runs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![id.as_str()],
            "only entries whose stem parses as a run id may be listed: {ids:?}"
        );
    }

    #[test]
    fn list_runs_skips_a_log_with_no_run_started() {
        let _guard = HomeGuard::new();
        let id = fresh_run_id();
        write_log(&id, vec![turn_started(1), turn_started(2)]);
        assert!(
            crate::config::runs_dir()
                .join(format!("{}.jsonl", id))
                .exists(),
            "setup sanity: the log must be on disk under var/log/runs"
        );
        let runs = list_runs();
        assert!(
            !runs.iter().any(|s| s.id == id),
            "a .jsonl without RunStarted cannot name a kind and must be skipped, not \
             listed with a guessed kind:\n{runs:?}"
        );
    }
}
