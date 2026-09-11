use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Status of a delegated agent job in the mailbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MailboxStatus {
    Pending,
    Complete,
    Failed,
}

/// Result written by a child ghost shell to its agent's mailbox so the
/// parent coordinator can retrieve it via `await_agent_result`.
///
/// Path: `~/.daemoneye/agents/<agent_name>/mailbox/<job_id>.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxResult {
    pub job_id: String,
    pub agent: String,
    pub task: String,
    pub status: MailboxStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub completed_at: Option<u64>,
    /// How the delegate's run ended. `None` for entries written before the
    /// run model reached ghost shells, and for any writer that has no run.
    // `default` only: keeping `skip_serializing_if` here would also drop the
    // field when it is legitimately present-and-`None` after a read of an old
    // file — serde round-trips must stay faithful, and the tests assert the
    // read-back value, not just parseability.
    #[serde(default)]
    pub run: Option<crate::daemon::run::RunResult>,
}

/// `~/.daemoneye/agents/<name>/mailbox/`
pub fn mailbox_dir(agent_name: &str) -> PathBuf {
    crate::agents::agent_dir(agent_name).join("mailbox")
}

/// Path to a specific mailbox file.
pub fn mailbox_path(agent_name: &str, job_id: &str) -> PathBuf {
    mailbox_dir(agent_name).join(format!("{}.json", job_id))
}

/// Ensure the mailbox directory exists for an agent.
pub fn ensure_mailbox_dir(agent_name: &str) -> Result<()> {
    let dir = mailbox_dir(agent_name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating mailbox dir {}", dir.display()))
}

/// Write a `MailboxResult` to the agent's mailbox.
///
/// Applies the sensitive-data masking filter on `result` before the write,
/// and on the model-authored text inside `run` (its summary and briefing
/// blocker) — the same output by another route.
pub fn write_mailbox(agent_name: &str, entry: &MailboxResult) -> Result<()> {
    ensure_mailbox_dir(agent_name)?;
    let path = mailbox_path(agent_name, &entry.job_id);
    let mut entry = entry.clone();
    if let Some(ref result) = entry.result {
        entry.result = Some(crate::ai::filter::mask_sensitive(result));
    }
    if let Some(ref mut run) = entry.run {
        run.summary = crate::ai::filter::mask_sensitive(&run.summary);
        if let Some(ref mut briefing) = run.briefing
            && let Some(ref blocker) = briefing.blocker
        {
            briefing.blocker = Some(crate::ai::filter::mask_sensitive(blocker));
        }
    }
    let text = serde_json::to_string_pretty(&entry).context("serializing mailbox entry")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Read a `MailboxResult` from the agent's mailbox.
///
/// Returns `Ok(None)` if the file does not exist.
pub fn read_mailbox(agent_name: &str, job_id: &str) -> Result<Option<MailboxResult>> {
    let path = mailbox_path(agent_name, job_id);
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let entry: MailboxResult =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("de_mailbox_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::test_home_guard();
        let old = env::var("HOME").ok();
        unsafe {
            env::set_var("HOME", tmp.path());
        }
        f();
        match old {
            Some(v) => unsafe {
                env::set_var("HOME", v);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
    }

    fn sample_entry(job_id: &str, agent: &str) -> MailboxResult {
        MailboxResult {
            job_id: job_id.to_string(),
            agent: agent.to_string(),
            task: "investigate disk usage".to_string(),
            status: MailboxStatus::Complete,
            result: Some("Disk is 90% full on /dev/sda1".to_string()),
            error: None,
            completed_at: Some(1712937600),
            run: None,
        }
    }

    #[test]
    fn write_and_read_result() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let entry = sample_entry("job-abc-123", "analyst");
            write_mailbox("analyst", &entry).unwrap();
            let read = read_mailbox("analyst", "job-abc-123").unwrap().unwrap();
            assert_eq!(read.job_id, "job-abc-123");
            assert_eq!(read.agent, "analyst");
            assert_eq!(read.status, MailboxStatus::Complete);
            assert_eq!(
                read.result,
                Some("Disk is 90% full on /dev/sda1".to_string())
            );
            assert_eq!(read.completed_at, Some(1712937600));
        });
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let result = read_mailbox("analyst", "no-such-job").unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn status_transitions() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let mut entry = MailboxResult {
                job_id: "job-x".to_string(),
                agent: "analyst".to_string(),
                task: "test".to_string(),
                status: MailboxStatus::Pending,
                result: None,
                error: None,
                completed_at: None,
                run: None,
            };
            write_mailbox("analyst", &entry).unwrap();

            entry.status = MailboxStatus::Complete;
            entry.result = Some("done".to_string());
            entry.completed_at = Some(1000);
            write_mailbox("analyst", &entry).unwrap();

            let read = read_mailbox("analyst", "job-x").unwrap().unwrap();
            assert_eq!(read.status, MailboxStatus::Complete);
            assert_eq!(read.result, Some("done".to_string()));
        });
    }
    #[test]
    fn masking_applied() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let entry = MailboxResult {
                job_id: "job-mask".to_string(),
                agent: "analyst".to_string(),
                task: "test".to_string(),
                status: MailboxStatus::Complete,
                result: Some("Found key: AKIAIOSFODNN7EXAMPLE in logs".to_string()),
                error: None,
                completed_at: Some(1000),
                run: None,
            };
            write_mailbox("analyst", &entry).unwrap();
            let path = mailbox_path("analyst", "job-mask");
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                !content.contains("AKIAIOSFODNN7EXAMPLE"),
                "sensitive data should be masked"
            );
        });
    }

    #[test]
    fn mailbox_round_trips_a_run_result() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let mut entry = sample_entry("job-run", "parser");
            entry.run = Some(crate::daemon::run::RunResult::complete(
                crate::daemon::run::RunId::parse("0123456789abcdef").unwrap(),
                crate::daemon::run::RunKind::Ghost {
                    runbook: "triage".to_string(),
                    trigger: "manual".to_string(),
                },
                "all done".to_string(),
                4,
                crate::daemon::run::Budget::unlimited(),
            ));
            write_mailbox("parser", &entry).unwrap();
            let read = read_mailbox("parser", "job-run").unwrap().unwrap();
            let run = read.run.expect("the run must survive the round trip");
            assert_eq!(run.status, crate::daemon::run::RunStatus::Complete);
            assert_eq!(run.turns, 4);
            assert_eq!(run.summary, "all done");
        });
    }

    #[test]
    fn mailbox_reads_an_entry_written_before_the_run_field_existed() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            ensure_mailbox_dir("legacy").unwrap();
            let path = mailbox_path("legacy", "job-old");
            let pre_run_json = r#"{
  "job_id": "job-old",
  "agent": "legacy",
  "task": "old task",
  "status": "Complete",
  "result": "earlier answer",
  "error": null,
  "completed_at": 1712937600
}"#;
            std::fs::write(&path, pre_run_json).unwrap();
            let read = read_mailbox("legacy", "job-old")
                .expect("an entry written before the run field must still parse")
                .expect("the entry must exist");
            assert!(
                read.run.is_none(),
                "a pre-run entry must read back with run: None, got {:?}",
                read.run
            );
        });
    }

    #[test]
    fn mailbox_masks_the_run_summary() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let mut entry = sample_entry("job-runmask", "raw");
            entry.run = Some(crate::daemon::run::RunResult::complete(
                crate::daemon::run::RunId::parse("0123456789abcdef").unwrap(),
                crate::daemon::run::RunKind::Ghost {
                    runbook: "scan".to_string(),
                    trigger: "manual".to_string(),
                },
                "Found key: AKIAIOSFODNN7EXAMPLE in logs".to_string(),
                1,
                crate::daemon::run::Budget::unlimited(),
            ));
            write_mailbox("raw", &entry).unwrap();
            let path = mailbox_path("raw", "job-runmask");
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                !content.contains("AKIAIOSFODNN7EXAMPLE"),
                "sensitive data should be masked"
            );
        });
    }
}
