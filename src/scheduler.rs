use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::RwLock;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// When a job should run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ScheduleKind {
    /// Run once at the given UTC datetime.
    Once { at: DateTime<Utc> },
    /// Run repeatedly every `interval_secs` seconds; `next_run` is updated after each execution.
    Every { interval_secs: u64, next_run: DateTime<Utc> },
}

impl ScheduleKind {
    /// Return the next scheduled run time, or `None` if this is a `Once` job that has already run.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        match self {
            ScheduleKind::Once { at } => Some(*at),
            ScheduleKind::Every { next_run, .. } => Some(*next_run),
        }
    }

    /// Advance `next_run` by `interval_secs` for `Every` jobs.
    pub fn advance(&mut self) {
        if let ScheduleKind::Every { interval_secs, next_run } = self {
            *next_run = *next_run + chrono::Duration::seconds(*interval_secs as i64);
        }
    }

    /// Human-readable description.
    pub fn describe(&self) -> String {
        match self {
            ScheduleKind::Once { at } => format!("once at {}", at.format("%Y-%m-%d %H:%M UTC")),
            ScheduleKind::Every { interval_secs, .. } => {
                format!("every {}", describe_secs(*interval_secs))
            }
        }
    }
}

/// What the scheduled job should do when it fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum ActionOn {
    /// Just emit a `SystemMsg` alert — no command execution.
    Alert,
    /// Execute a raw shell command string.
    Command(String),
    /// Execute a named script from `~/.daemoneye/scripts/`.
    Script(String),
}

impl ActionOn {
    pub fn describe(&self) -> String {
        match self {
            ActionOn::Alert => "alert".to_string(),
            ActionOn::Command(c) => format!("cmd: {}", c),
            ActionOn::Script(s) => format!("script: {}", s),
        }
    }
}

/// Lifecycle state of a scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    /// Waiting to fire.
    Pending,
    /// Currently executing.
    Running,
    /// Completed successfully (for `Every` jobs this transitions back to `Pending`).
    Succeeded,
    /// Failed; the tmux window `de-<id>` is left open for inspection.
    Failed(String),
    /// Cancelled by the user.
    Cancelled,
}

impl JobStatus {
    pub fn describe(&self) -> String {
        match self {
            JobStatus::Pending => "pending".to_string(),
            JobStatus::Running => "running".to_string(),
            JobStatus::Succeeded => "succeeded".to_string(),
            JobStatus::Failed(msg) => format!("failed: {}", msg),
            JobStatus::Cancelled => "cancelled".to_string(),
        }
    }
}

/// A single persisted scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// UUID v4 identifier.
    pub id: String,
    /// Human-readable name chosen by the AI or user.
    pub name: String,
    /// When and how often to fire.
    pub kind: ScheduleKind,
    /// What to do when the job fires.
    pub action: ActionOn,
    /// Optional runbook name for watchdog AI analysis.
    pub runbook: Option<String>,
    /// Current lifecycle state.
    pub status: JobStatus,
    /// Wall-clock time the job was created.
    pub created_at: DateTime<Utc>,
    /// Wall-clock time of the most recent execution attempt.
    pub last_run: Option<DateTime<Utc>>,
}

impl ScheduledJob {
    /// Create a new job with a fresh UUID.
    pub fn new(
        name: String,
        kind: ScheduleKind,
        action: ActionOn,
        runbook: Option<String>,
    ) -> Self {
        ScheduledJob {
            id: Uuid::new_v4().to_string(),
            name,
            kind,
            action,
            runbook,
            status: JobStatus::Pending,
            created_at: Utc::now(),
            last_run: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Thread-safe, file-backed store for scheduled jobs.
///
/// Persistence is atomic: writes go to a `.tmp` file then rename over the target.
pub struct ScheduleStore {
    path: PathBuf,
    pub jobs: RwLock<Vec<ScheduledJob>>,
}

impl ScheduleStore {
    /// Load from `path` or create an empty store if the file does not exist.
    pub fn load_or_create(path: PathBuf) -> Result<Self> {
        let jobs = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str::<Vec<ScheduledJob>>(&text)
                .with_context(|| format!("parsing {}", path.display()))?
        } else {
            Vec::new()
        };
        Ok(ScheduleStore { path, jobs: RwLock::new(jobs) })
    }

    /// Persist the current job list atomically.
    fn save(&self) -> Result<()> {
        let jobs = self.jobs.read().unwrap_or_else(|e| e.into_inner());
        let json = serde_json::to_string_pretty(&*jobs)?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    /// Add a new job and persist.
    pub fn add(&self, job: ScheduledJob) -> Result<String> {
        let id = job.id.clone();
        {
            let mut jobs = self.jobs.write().unwrap_or_else(|e| e.into_inner());
            jobs.push(job);
        }
        self.save()?;
        Ok(id)
    }

    /// Cancel a job by ID. Returns `true` if the job was found and cancelled.
    pub fn cancel(&self, id: &str) -> Result<bool> {
        let found;
        {
            let mut jobs = self.jobs.write().unwrap_or_else(|e| e.into_inner());
            if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
                j.status = JobStatus::Cancelled;
                found = true;
            } else {
                found = false;
            }
        }
        if found { self.save()?; }
        Ok(found)
    }

    /// Return a snapshot of all jobs for listing.
    pub fn list(&self) -> Vec<ScheduledJob> {
        self.jobs.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Return all jobs that are due to run now and set their status to `Running`.
    /// Removes `Once` jobs from the queue when they are taken.
    pub fn take_due(&self) -> Vec<ScheduledJob> {
        let now = Utc::now();
        let mut due = Vec::new();
        {
            let mut jobs = self.jobs.write().unwrap_or_else(|e| e.into_inner());
            for job in jobs.iter_mut() {
                if job.status != JobStatus::Pending {
                    continue;
                }
                let fire = match job.kind.next_run() {
                    Some(t) => t <= now,
                    None => false,
                };
                if fire {
                    job.status = JobStatus::Running;
                    job.last_run = Some(now);
                    due.push(job.clone());
                }
            }
        }
        due
    }

    /// Mark a running job as succeeded or failed, advancing `Every` jobs back to `Pending`.
    pub fn mark_done(&self, id: &str, success: bool, error_msg: Option<String>) {
        {
            let mut jobs = self.jobs.write().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                if success {
                    match &mut job.kind {
                        ScheduleKind::Every { .. } => {
                            job.kind.advance();
                            job.status = JobStatus::Pending;
                        }
                        ScheduleKind::Once { .. } => {
                            job.status = JobStatus::Succeeded;
                        }
                    }
                } else {
                    job.status = JobStatus::Failed(error_msg.unwrap_or_else(|| "unknown error".to_string()));
                }
            }
        }
        let _ = self.save();
    }
}

// ---------------------------------------------------------------------------
// ISO 8601 duration parser
// ---------------------------------------------------------------------------

/// Parse a minimal ISO 8601 duration string into seconds.
///
/// Supports: `P[nD]T[nH][nM][nS]` — e.g. PT5M, PT1H30M, P1D, PT30S.
/// Returns `None` if the string cannot be parsed.
pub fn parse_iso_duration(s: &str) -> Option<u64> {
    if !s.starts_with('P') {
        return None;
    }
    let s = &s[1..]; // strip 'P'
    let (date_part, time_part) = if let Some(t_pos) = s.find('T') {
        (&s[..t_pos], &s[t_pos + 1..])
    } else {
        (s, "")
    };

    let mut secs = 0u64;

    // Parse date part: nD
    for (suffix, multiplier) in [('D', 86400u64)] {
        if let Some(val) = extract_component(date_part, suffix) {
            secs += val * multiplier;
        }
    }

    // Parse time part: nH, nM, nS
    for (suffix, multiplier) in [('H', 3600u64), ('M', 60), ('S', 1)] {
        if let Some(val) = extract_component(time_part, suffix) {
            secs += val * multiplier;
        }
    }

    if secs == 0 { None } else { Some(secs) }
}

fn extract_component(s: &str, unit: char) -> Option<u64> {
    let pos = s.find(unit)?;
    // find the start of the number before `pos`
    let before = &s[..pos];
    let start = before.rfind(|c: char| !c.is_ascii_digit()).map(|p| p + 1).unwrap_or(0);
    before[start..].parse::<u64>().ok()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn describe_secs(secs: u64) -> String {
    if secs % 86400 == 0 { return format!("{}d", secs / 86400); }
    if secs % 3600 == 0  { return format!("{}h", secs / 3600); }
    if secs % 60 == 0    { return format!("{}m", secs / 60); }
    format!("{}s", secs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_pt5m() {
        assert_eq!(parse_iso_duration("PT5M"), Some(300));
    }

    #[test]
    fn parse_iso_pt1h() {
        assert_eq!(parse_iso_duration("PT1H"), Some(3600));
    }

    #[test]
    fn parse_iso_pt1h30m() {
        assert_eq!(parse_iso_duration("PT1H30M"), Some(5400));
    }

    #[test]
    fn parse_iso_p1d() {
        assert_eq!(parse_iso_duration("P1D"), Some(86400));
    }

    #[test]
    fn parse_iso_pt30s() {
        assert_eq!(parse_iso_duration("PT30S"), Some(30));
    }

    #[test]
    fn parse_iso_invalid() {
        assert_eq!(parse_iso_duration("not-a-duration"), None);
    }

    #[test]
    fn schedule_kind_describe_once() {
        let t = DateTime::parse_from_rfc3339("2026-03-01T12:00:00Z").unwrap().with_timezone(&Utc);
        let k = ScheduleKind::Once { at: t };
        assert!(k.describe().contains("once"));
    }

    #[test]
    fn schedule_kind_describe_every() {
        let t = Utc::now();
        let k = ScheduleKind::Every { interval_secs: 300, next_run: t };
        assert!(k.describe().contains("every"));
        assert!(k.describe().contains("5m"));
    }

    #[test]
    fn scheduled_job_new_has_unique_id() {
        let j1 = ScheduledJob::new(
            "test".to_string(),
            ScheduleKind::Every { interval_secs: 60, next_run: Utc::now() },
            ActionOn::Alert,
            None,
        );
        let j2 = ScheduledJob::new(
            "test2".to_string(),
            ScheduleKind::Every { interval_secs: 60, next_run: Utc::now() },
            ActionOn::Alert,
            None,
        );
        assert_ne!(j1.id, j2.id);
    }

    fn tmp_store() -> (ScheduleStore, PathBuf) {
        let path = std::env::temp_dir().join(format!("de_test_{}.json", Uuid::new_v4()));
        let store = ScheduleStore::load_or_create(path.clone()).unwrap();
        (store, path)
    }

    #[test]
    fn store_add_list_cancel() {
        let (store, path) = tmp_store();

        let job = ScheduledJob::new(
            "disk-check".to_string(),
            ScheduleKind::Every { interval_secs: 300, next_run: Utc::now() },
            ActionOn::Command("df -h".to_string()),
            None,
        );
        let id = store.add(job).unwrap();
        assert_eq!(store.list().len(), 1);

        let found = store.cancel(&id).unwrap();
        assert!(found);
        assert_eq!(store.list()[0].status, JobStatus::Cancelled);

        // File should be persisted
        let store2 = ScheduleStore::load_or_create(path).unwrap();
        assert_eq!(store2.list()[0].status, JobStatus::Cancelled);
    }

    #[test]
    fn store_take_due_marks_running() {
        let (store, _path) = tmp_store();

        // Job with next_run in the past
        let past = Utc::now() - chrono::Duration::seconds(10);
        let job = ScheduledJob::new(
            "past-job".to_string(),
            ScheduleKind::Every { interval_secs: 60, next_run: past },
            ActionOn::Alert,
            None,
        );
        store.add(job).unwrap();

        let due = store.take_due();
        assert_eq!(due.len(), 1);
        assert_eq!(store.list()[0].status, JobStatus::Running);
    }

    #[test]
    fn store_mark_done_success_reschedules_every() {
        let (store, _path) = tmp_store();

        let past = Utc::now() - chrono::Duration::seconds(10);
        let job = ScheduledJob::new(
            "repeat".to_string(),
            ScheduleKind::Every { interval_secs: 300, next_run: past },
            ActionOn::Alert,
            None,
        );
        let id = store.add(job).unwrap();
        store.take_due(); // marks running

        store.mark_done(&id, true, None);
        assert_eq!(store.list()[0].status, JobStatus::Pending);
    }
}
