pub use crate::util::UnpoisonExt;

mod event_log;
mod host;
mod log_rotation;
mod output;
mod response;
mod shell;
mod sudo;
mod warnings;

pub use event_log::*;
pub use host::*;
pub use log_rotation::*;
pub use output::*;
pub use response::*;
pub use shell::*;
pub use sudo::*;
pub use warnings::*;

/// Delete session archive files (`*.archive.jsonl`) whose mtime is older than
/// `retention_days` days. 0 = keep forever (no-op). Active sessions (present
/// in the in-memory store) are never swept.
pub fn sweep_session_archives(
    retention_days: u32,
    active_sessions: &std::collections::HashSet<String>,
) {
    if retention_days == 0 {
        return;
    }

    let sessions_dir = crate::config::sessions_dir();
    let Ok(entries) = std::fs::read_dir(&sessions_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".archive.jsonl") {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        // Extract session id from `id.archive.jsonl`
        let session_id = name.trim_end_matches(".archive.jsonl");
        if active_sessions.contains(session_id) {
            continue;
        }

        log::info!("sessions: deleting expired archive {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!(
                "sessions: failed to delete archive {}: {}",
                path.display(),
                e
            );
        }
    }
}

/// Delete pane log files (`*.log`) in `var/log/panes/` whose mtime is older
/// than `retention_days` days. 0 = keep forever (no-op).
pub fn sweep_pane_logs(retention_days: u32) {
    if retention_days == 0 {
        return;
    }

    let panes_dir = crate::config::pane_logs_dir();
    let Ok(entries) = std::fs::read_dir(&panes_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".log") {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        log::info!("panes: deleting expired log {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("panes: failed to delete {}: {}", path.display(), e);
        }
    }
}

/// Delete mailbox files (`*.json`) in every agent's `agents/<name>/mailbox/`
/// directory whose mtime is older than `retention_days` days. 0 = keep forever
/// (no-op). An agent with no mailbox directory is silently skipped.
pub fn sweep_agent_mailboxes(retention_days: u32) {
    if retention_days == 0 {
        return;
    }

    let agents_dir = crate::agents::agents_dir();
    let Ok(agent_entries) = std::fs::read_dir(&agents_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for agent_entry in agent_entries.filter_map(|e| e.ok()) {
        let mailbox = agent_entry.path().join("mailbox");
        let Ok(mailbox_entries) = std::fs::read_dir(&mailbox) else {
            continue;
        };

        for entry in mailbox_entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".json") {
                continue;
            }

            let Ok(meta) = path.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if modified >= cutoff {
                continue;
            }

            log::info!("mailboxes: deleting expired entry {}", path.display());
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("mailboxes: failed to delete {}: {}", path.display(), e);
            }
        }
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use std::collections::HashSet;

    fn create_archive_file(
        sessions_dir: &std::path::Path,
        id: &str,
        content: &str,
    ) -> std::path::PathBuf {
        let path = sessions_dir.join(format!("{}.archive.jsonl", id));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn set_file_mtime(path: &std::path::Path, days_ago: u64) {
        let mtime =
            std::time::SystemTime::now() - std::time::Duration::from_secs(days_ago * 86_400);
        let ft = filetime::FileTime::from_system_time(mtime);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    #[test]
    fn sweep_archives_respects_active_and_zero() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Create 3 archives: active, inactive-expired, inactive-recent
        create_archive_file(&sessions_dir, "active-sess", "data");
        create_archive_file(&sessions_dir, "expired-sess", "data");
        create_archive_file(&sessions_dir, "recent-sess", "data");

        // Make expired-sess old (30 days)
        set_file_mtime(&sessions_dir.join("expired-sess.archive.jsonl"), 30);
        // Make active-sess old too (30 days) — should survive because active
        set_file_mtime(&sessions_dir.join("active-sess.archive.jsonl"), 30);
        // recent-sess stays new (0 days)

        // Active set contains "active-sess"
        let mut active = HashSet::new();
        active.insert("active-sess".to_string());

        // retention_days = 14: expired-sess should be deleted, active-sess survives
        sweep_session_archives(14, &active);
        assert!(
            sessions_dir.join("active-sess.archive.jsonl").exists(),
            "active session archive should survive even when expired"
        );
        assert!(
            !sessions_dir.join("expired-sess.archive.jsonl").exists(),
            "expired inactive archive should be deleted"
        );
        assert!(
            sessions_dir.join("recent-sess.archive.jsonl").exists(),
            "recent archive should survive"
        );

        // retention_days = 0: no-op (recreate the expired one first)
        create_archive_file(&sessions_dir, "expired-sess", "data");
        set_file_mtime(&sessions_dir.join("expired-sess.archive.jsonl"), 30);
        sweep_session_archives(0, &active);
        assert!(
            sessions_dir.join("expired-sess.archive.jsonl").exists(),
            "retention_days=0 should be a no-op"
        );

        // Restore HOME so ambient readers in other tests are not poisoned.
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_pane_logs_deletes_expired_keeps_recent() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let panes_dir = crate::config::pane_logs_dir();
        std::fs::create_dir_all(&panes_dir).unwrap();

        let old_log = panes_dir.join("old-pane.log");
        let recent_log = panes_dir.join("recent-pane.log");
        let not_log = panes_dir.join("something.txt");
        std::fs::write(&old_log, "old data").unwrap();
        std::fs::write(&recent_log, "recent data").unwrap();
        std::fs::write(&not_log, "not a log").unwrap();

        set_file_mtime(&old_log, 30);

        sweep_pane_logs(14);

        assert!(!old_log.exists(), "expired pane log should be deleted");
        assert!(recent_log.exists(), "recent pane log should survive");
        assert!(not_log.exists(), "non-.log files should be untouched");

        // Restore HOME so ambient readers in other tests are not poisoned.
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_pane_logs_zero_is_noop() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let panes_dir = crate::config::pane_logs_dir();
        std::fs::create_dir_all(&panes_dir).unwrap();

        let old_log = panes_dir.join("old-pane.log");
        std::fs::write(&old_log, "old data").unwrap();
        set_file_mtime(&old_log, 30);

        sweep_pane_logs(0);

        assert!(old_log.exists(), "retention_days=0 should be a no-op");

        // Restore HOME so ambient readers in other tests are not poisoned.
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_agent_mailboxes_deletes_expired_keeps_recent() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let agents_dir = crate::agents::agents_dir();
        std::fs::create_dir_all(&agents_dir).unwrap();

        let agent1_mailbox = agents_dir.join("agent-1/mailbox");
        std::fs::create_dir_all(agents_dir.join("agent-2")).unwrap();
        std::fs::create_dir_all(&agent1_mailbox).unwrap();

        let old_entry = agent1_mailbox.join("job-1.json");
        let recent_entry = agent1_mailbox.join("job-2.json");
        let not_json = agent1_mailbox.join("notes.txt");
        std::fs::write(&old_entry, r#"{"job_id":"job-1"}"#).unwrap();
        std::fs::write(&recent_entry, r#"{"job_id":"job-2"}"#).unwrap();
        std::fs::write(&not_json, "not json").unwrap();

        set_file_mtime(&old_entry, 30);

        sweep_agent_mailboxes(14);

        assert!(
            !old_entry.exists(),
            "expired mailbox entry should be deleted"
        );
        assert!(recent_entry.exists(), "recent mailbox entry should survive");
        assert!(not_json.exists(), "non-.json files should be untouched");

        // Restore HOME so ambient readers in other tests are not poisoned.
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_agent_mailboxes_zero_is_noop() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let agents_dir = crate::agents::agents_dir();
        let agent_mailbox = agents_dir.join("agent-x/mailbox");
        std::fs::create_dir_all(&agent_mailbox).unwrap();

        let old_entry = agent_mailbox.join("job-1.json");
        std::fs::write(&old_entry, r#"{"job_id":"job-1"}"#).unwrap();
        set_file_mtime(&old_entry, 30);

        sweep_agent_mailboxes(0);

        assert!(old_entry.exists(), "retention_days=0 should be a no-op");

        // Restore HOME so ambient readers in other tests are not poisoned.
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
