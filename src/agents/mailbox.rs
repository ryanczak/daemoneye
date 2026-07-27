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
/// Applies the sensitive-data masking filter on `result` before the write.
pub fn write_mailbox(agent_name: &str, entry: &MailboxResult) -> Result<()> {
    ensure_mailbox_dir(agent_name)?;
    let path = mailbox_path(agent_name, &entry.job_id);
    let mut entry = entry.clone();
    if let Some(ref result) = entry.result {
        entry.result = Some(crate::ai::filter::mask_sensitive(result));
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
}
