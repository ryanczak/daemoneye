//! `top`'s loaders and change fingerprint: the Runs, Budget, Shells and header reads, `take_snapshot`, `fingerprint`.

use super::*;

// ── Loaders ──────────────────────────────────────────────────────────────────

/// Build the Runs panel from [`list_runs`]: up to 20 rows, running first,
/// then the rest newest first. The start instant and the log's mtime come
/// from the run log itself, so the row's `age_s` and `liveness` are read
/// where `list_runs` sorted from.
fn load_runs(now: DateTime<Utc>) -> Vec<TopRunRow> {
    let summaries = list_runs();
    let mut rows: Vec<(TopRunRow, DateTime<Utc>, bool)> = Vec::new();
    for summary in &summaries {
        let Some(started) = run_started_at(&summary.id) else {
            continue;
        };
        let path = crate::daemon::run::turn_log_file(&summary.id);
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        let running = !summary.state.is_terminal();
        let row = TopRunRow {
            id: summary.id.as_str().to_string(),
            kind: summary.kind.label().to_string(),
            subject: run_subject(&summary.kind),
            state: state_word(&summary.state),
            turns: summary.turns,
            age_s: (now - started).num_seconds(),
            liveness: liveness(running, mtime, now),
        };
        rows.push((row, started, running));
    }
    sort_rows(rows).into_iter().take(MAX_RUN_ROWS).collect()
}

/// What the run is, from the `RunStarted` kind: `chat:<8>`,
/// `<runbook> ← <trigger>`, `job:<job>` or `<agent> ← <8>`; a `40`-character
/// subject is cut to `32` with `…`; `-` is never a kind, so this is only
/// reached with a real one.
pub(super) fn run_subject(kind: &crate::daemon::run::RunKind) -> String {
    use crate::daemon::run::RunKind;
    const MAX_SUBJECT: usize = 32;
    let raw = match kind {
        RunKind::Chat { session } => format!("chat:{}", short_session(session)),
        RunKind::Ghost { runbook, trigger } => format!("{runbook} ← {trigger}"),
        RunKind::Scheduled { job } => format!("job:{job}"),
        RunKind::Delegated { agent, parent } => {
            format!("{agent} ← {}", short_session(parent.as_str()))
        }
    };
    if raw.chars().count() > MAX_SUBJECT {
        format!("{}…", raw.chars().take(MAX_SUBJECT).collect::<String>())
    } else {
        raw
    }
}

/// The state word the Runs panel prints for a summary, matching `run list`'s
/// table (`src/daemon/run/list.rs`).
pub(super) fn state_word(state: &crate::daemon::run::RunState) -> String {
    use crate::daemon::run::RunState;
    match state {
        RunState::Running => "running".to_string(),
        RunState::Complete => "complete".to_string(),
        RunState::HardFail(_) => "hard-fail".to_string(),
        RunState::BudgetExceeded(_) => "budget-exceeded".to_string(),
        RunState::Cancelled { .. } => "cancelled".to_string(),
    }
}

/// The instant the run log's `RunStarted` carries, or `None` when it cannot
/// be read or parsed.
fn run_started_at(id: &crate::daemon::run::RunId) -> Option<DateTime<Utc>> {
    let records = read_turn_log(id);
    let ts = records.iter().find_map(|r| match &r.event {
        crate::daemon::run::TurnLogEvent::RunStarted { .. } => Some(r.timestamp.clone()),
        _ => None,
    })?;
    DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Running runs first, then the rest newest first. Pure: takes the rows with
/// their start instants and running flags, returns the order.
pub(super) fn sort_rows(mut rows: Vec<(TopRunRow, DateTime<Utc>, bool)>) -> Vec<TopRunRow> {
    rows.sort_by(|(_a, a_started, a_running), (_b, b_started, b_running)| {
        match (a_running, b_running) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => b_started.cmp(a_started),
            (false, false) => b_started.cmp(a_started),
        }
    });
    rows.into_iter().map(|(row, _, _)| row).collect()
}

/// Read today's event values for the Budget and Activity panels. Reads the
/// segments in the local-day window, like every other disk reader.
fn read_today_events() -> Vec<serde_json::Value> {
    let (from, to) = local_day_bounds(Local::now());
    let mut events = Vec::new();
    for_each_event_between(Some(from), Some(to), &mut |value| {
        events.push(value.clone());
    });
    events
}

/// Read the disk panels: the Runs panel, the Budget panel and the Activity
/// rows. No daemon involved.
pub fn disk_panels(config_cap: f64) -> DiskPanels {
    let now = Utc::now();
    let events = read_today_events();
    DiskPanels {
        runs: load_runs(now),
        budget: fold_today(&events, now, config_cap),
        activity: last_activity(&events),
    }
}

/// Ask the daemon for its live PTY shells. One read-only `Request::ShellList`,
/// nothing written. A daemon that answers anything other than a shell list is
/// as good as a daemon that is not running: the panel says so either way.
pub async fn shells_panel() -> ShellsPanel {
    match super::ipc_client::request_once(crate::ipc::Request::ShellList).await {
        Ok(crate::ipc::Response::ShellList { shells }) => ShellsPanel::Rows(shells),
        _ => ShellsPanel::DaemonDown,
    }
}

/// Ask the daemon for the header facts: pid, uptime, provider and model. One
/// read-only `Request::Status`, nothing written. `None` when no daemon answers.
pub async fn daemon_header() -> Option<DaemonHeader> {
    match super::ipc_client::request_once(crate::ipc::Request::Status).await {
        Ok(crate::ipc::Response::DaemonStatus {
            pid,
            uptime_secs,
            provider,
            model,
            ..
        }) => Some(DaemonHeader {
            pid,
            uptime_secs,
            provider,
            model,
        }),
        _ => None,
    }
}

/// Take one frame: the disk panels plus the Shells panel from the daemon.
pub async fn take_snapshot(config_cap: f64) -> TopSnapshot {
    let now = Utc::now();
    let (shells, daemon) = tokio::join!(shells_panel(), daemon_header());
    let DiskPanels {
        runs,
        budget,
        activity,
    } = disk_panels(config_cap);
    TopSnapshot {
        taken_at: now,
        runs,
        budget,
        activity,
        shells,
        daemon,
    }
}

// ── Fingerprint ──────────────────────────────────────────────────────────────

/// The stats-only fingerprint the live loop re-reads on: the runs directory's
/// entry count and newest mtime, today's segment's length and mtime. Reads
/// no file contents, so it costs nothing to poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TopFingerprint {
    /// Entries under `var/log/runs/`.
    pub runs_entries: u64,
    /// The newest entry's mtime, in seconds since the epoch; 0 when none.
    pub runs_newest_mtime_s: u64,
    /// Today's segment file's length in bytes; 0 when there is none.
    pub segment_len: u64,
    /// Today's segment file's mtime, in seconds since the epoch; 0 when none.
    pub segment_mtime_s: u64,
}

/// A segment file's length and mtime, or zeros when there is none.
fn file_stats(path: &std::path::Path) -> (u64, u64) {
    match std::fs::metadata(path) {
        Ok(meta) => (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

/// Read the runs directory's stats only: how many entries it holds and the
/// newest entry's mtime. Never opens a file.
fn runs_dir_stats() -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(crate::config::runs_dir()) else {
        return (0, 0);
    };
    let mut count: u64 = 0;
    let mut newest: u64 = 0;
    for entry in entries.flatten() {
        count += 1;
        if let Ok(meta) = entry.metadata()
            && let Ok(m) = meta.modified()
            && let Ok(d) = m.duration_since(std::time::UNIX_EPOCH)
        {
            newest = newest.max(d.as_secs());
        }
    }
    (count, newest)
}

/// Read the fingerprint from disk: stats only, never file contents.
pub fn fingerprint() -> TopFingerprint {
    let (runs_entries, runs_newest_mtime_s) = runs_dir_stats();
    let (segment_len, segment_mtime_s) = file_stats(&crate::config::current_event_segment_path());
    TopFingerprint {
        runs_entries,
        runs_newest_mtime_s,
        segment_len,
        segment_mtime_s,
    }
}
