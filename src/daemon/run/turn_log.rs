//! The per-run turn log (M24 phase 02a): an append-only JSONL file per run
//! at `config::runs_dir()/<run-id>.jsonl`. Each line is a
//! [`TurnLogRecord`] — a timestamp, the run id, and an `event_type`-tagged
//! payload (design § 2.3). Domain types are serialised verbatim; every field
//! added to a record after this phase carries `#[serde(default)]` so an old
//! log still deserialises.
//!
//! The writer flushes per record and warns rather than panicking on failure;
//! the reader skips a malformed line (a truncated final record from a crash
//! mid-write) rather than failing the whole read.
//!
//! Nothing writes a turn log yet — wiring is phases 05–08.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{RunId, RunKind, RunState};

/// The JSONL text of one logged turn event, appended as a single line.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TurnLogRecord {
    /// UTC wall-clock time the event happened, RFC 3339. Supplied by the
    /// caller: this type stays free of a clock and tests stay deterministic.
    pub timestamp: String,
    /// The run the event belongs to.
    pub run_id: RunId,
    /// The event itself, discriminated on `event_type`.
    #[serde(flatten)]
    pub event: TurnLogEvent,
}

/// One event in a run's turn log. Serde-rename each variant explicitly so a
/// wire name cannot drift with a Rust rename.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum TurnLogEvent {
    #[serde(rename = "run_started")]
    RunStarted { kind: RunKind },
    #[serde(rename = "turn_started")]
    TurnStarted { turn: u32 },
    #[serde(rename = "note")]
    Note { text: String },
    #[serde(rename = "run_finished")]
    RunFinished { state: RunState },
}

/// Path to a run's JSONL turn log.
pub fn turn_log_file(id: &RunId) -> PathBuf {
    crate::config::runs_dir().join(format!("{}.jsonl", id))
}

/// Append one record to the run's turn log, flushing per record. The turn
/// log is diagnostic — a failure to write is logged at WARN, never an error
/// returned to the caller and never a panic.
pub fn append_turn_event(record: &TurnLogRecord) {
    let path = turn_log_file(&record.run_id);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        log::warn!(
            "Failed to create turn-log directory {}: {}",
            parent.display(),
            e
        );
        return;
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Ok(line) = serde_json::to_string(record)
                && let Err(e) = writeln!(f, "{}", line)
            {
                log::warn!("Failed to append to turn log {}: {}", path.display(), e);
            }
            if let Err(e) = f.flush() {
                log::warn!("Failed to flush turn log {}: {}", path.display(), e);
            }
        }
        Err(e) => {
            log::warn!(
                "Failed to open turn log {} for append: {}",
                path.display(),
                e
            );
        }
    }
}

/// Read back the turn-log records for a run, in order. A malformed line
/// inside the file is skipped rather than fatal (the same rule
/// `shell::log::read_command_output` applies to casts): a truncated final
/// record from a crash mid-write must not make the whole log unreadable.
/// A run with no log yields an empty `Vec`, not an error.
pub fn read_turn_log(id: &RunId) -> Vec<TurnLogRecord> {
    let path = turn_log_file(id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Append one `TurnStarted { turn }` record to the run's turn log. The turn
/// log is diagnostic: a write problem is logged inside `append_turn_event`,
/// never a failure of the turn itself. The one writer, shared by the ghost
/// and chat loops so their logs cannot drift apart.
pub fn emit_turn_started(run_id: &RunId, turn: usize) {
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id: run_id.clone(),
        event: TurnLogEvent::TurnStarted { turn: turn as u32 },
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::{Briefing, BudgetDimension};

    /// RAII-guarded HOME pointing at a fresh tempdir, so a test that writes a
    /// turn log never touches the developer's real `~/.daemoneye`.
    struct HomeGuard {
        _lock: crate::TestHomeGuard,
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

    fn record(run_id: &RunId, event: TurnLogEvent) -> TurnLogRecord {
        TurnLogRecord {
            timestamp: "2026-09-09T20:01:00Z".to_string(),
            run_id: run_id.clone(),
            event,
        }
    }

    fn append_all(run_id: &RunId, events: Vec<TurnLogEvent>) {
        for event in events {
            append_turn_event(&record(run_id, event));
        }
    }

    #[test]
    fn emit_turn_started_appends_the_turn() {
        let _guard = HomeGuard::new();
        let id = RunId::parse("a1b2c3d4e5f6071f").unwrap();
        emit_turn_started(&id, 1);
        emit_turn_started(&id, 2);
        let records = read_turn_log(&id);
        let turns: Vec<u32> = records
            .iter()
            .filter_map(|r| match &r.event {
                TurnLogEvent::TurnStarted { turn } => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(
            turns,
            vec![1, 2],
            "two emits must land two TurnStarted records, in order"
        );
        assert!(records.iter().all(|r| r.run_id == id));
    }

    #[test]
    fn turn_log_writes_one_json_line_per_record() {
        let guard = HomeGuard::new();
        let id = RunId::parse("a1b2c3d4e5f60718").unwrap();
        append_all(
            &id,
            vec![
                TurnLogEvent::Note { text: "one".into() },
                TurnLogEvent::Note { text: "two".into() },
                TurnLogEvent::Note {
                    text: "three".into(),
                },
            ],
        );
        let records = read_turn_log(&id);
        assert_eq!(records.len(), 3, "three records must be three records");
        let raw = std::fs::read_to_string(turn_log_file(&id)).unwrap();
        assert_eq!(raw.lines().count(), 3, "three records must be three lines");
        assert!(
            guard.tmp.path().join(".daemoneye/var/log/runs").exists(),
            "the log must live under the guarded temp HOME, not the real one"
        );
    }

    #[test]
    fn turn_log_round_trips_all_four_event_variants() {
        let _guard = HomeGuard::new();
        let id = RunId::parse("a1b2c3d4e5f60719").unwrap();
        let started = TurnLogEvent::RunStarted {
            kind: RunKind::Ghost {
                runbook: "investigate-disk".into(),
                trigger: "cron".into(),
            },
        };
        let turn = TurnLogEvent::TurnStarted { turn: 3 };
        let note = TurnLogEvent::Note {
            text: "hello".into(),
        };
        let finished = TurnLogEvent::RunFinished {
            state: RunState::BudgetExceeded(Briefing::new(
                "goal",
                Some("criteria".into()),
                vec!["src/main.rs".into()],
                vec!["tried X".into()],
                Some("blocked".into()),
                Some(BudgetDimension::Turns),
            )),
        };
        append_all(
            &id,
            vec![
                started.clone(),
                turn.clone(),
                note.clone(),
                finished.clone(),
            ],
        );

        let records: Vec<TurnLogRecord> = read_turn_log(&id);
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].event, started);
        assert_eq!(records[1].event, turn);
        assert_eq!(records[2].event, note);
        assert_eq!(records[3].event, finished);
        assert_eq!(records[0].run_id, id);
        assert_eq!(records[0].timestamp, "2026-09-09T20:01:00Z");
    }

    #[test]
    fn turn_log_reader_skips_a_malformed_line() {
        let _guard = HomeGuard::new();
        let id = RunId::parse("a1b2c3d4e5f6071a").unwrap();
        append_all(
            &id,
            vec![TurnLogEvent::Note {
                text: "first".into(),
            }],
        );
        let path = turn_log_file(&id);
        let existing = std::fs::read_to_string(&path).unwrap();
        let mut rewrite = existing.clone();
        rewrite.push_str("not json at all\n");
        let mut second = record(
            &id,
            TurnLogEvent::Note {
                text: "second".into(),
            },
        );
        second.timestamp = "2026-09-09T20:01:01Z".into();
        rewrite.push_str(&serde_json::to_string(&second).unwrap());
        rewrite.push('\n');
        std::fs::write(&path, rewrite).unwrap();
        drop(existing);

        let records = read_turn_log(&id);
        assert_eq!(
            records.len(),
            2,
            "the garbage line must be skipped, not fatal"
        );
        assert_eq!(records[0].timestamp, "2026-09-09T20:01:00Z");
        assert_eq!(records[1].timestamp, "2026-09-09T20:01:01Z");
    }

    #[test]
    fn turn_log_reader_returns_empty_for_an_unknown_run() {
        let _guard = HomeGuard::new();
        let id = RunId::parse("a1b2c3d4e5f6071b").unwrap();
        let records = read_turn_log(&id);
        assert!(records.is_empty(), "no file means no records, not an error");
        assert!(!turn_log_file(&id).exists());
    }

    #[test]
    fn turn_log_appends_rather_than_truncating() {
        let _guard = HomeGuard::new();
        let id = RunId::parse("a1b2c3d4e5f6071c").unwrap();
        append_turn_event(&record(&id, TurnLogEvent::Note { text: "one".into() }));
        let first = std::fs::read_to_string(turn_log_file(&id)).unwrap();
        assert_eq!(first.lines().count(), 1, "setup: exactly one line");
        append_all(&id, vec![TurnLogEvent::Note { text: "two".into() }]);
        let records = read_turn_log(&id);
        assert_eq!(records.len(), 2, "the second writer must not truncate");
        assert_eq!(records[0].timestamp, "2026-09-09T20:01:00Z");
        assert_eq!(records[1].timestamp, "2026-09-09T20:01:00Z");
    }

    #[test]
    fn runs_dir_is_under_var_log() {
        let _guard = HomeGuard::new();
        let dir = crate::config::runs_dir();
        assert!(
            dir.ends_with("var/log/runs"),
            "runs_dir must be var/log/runs, got {dir:?}"
        );
        let var_log = crate::config::var_log_dir();
        assert!(
            dir.starts_with(&var_log),
            "runs_dir must sit beneath var_log_dir: {dir:?} vs {var_log:?}"
        );
    }

    #[test]
    fn ensure_dirs_creates_the_runs_dir() {
        let _guard = HomeGuard::new();
        crate::config::Config::ensure_dirs().unwrap();
        let dir = crate::config::runs_dir();
        assert!(
            dir.is_dir(),
            "ensure_dirs() must create {} eagerly — F1's lifecycle gate",
            dir.display()
        );
    }
}
