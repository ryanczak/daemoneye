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
        } else {
            if let Err(e) = crate::memory::index::remove_session_turns(session_id) {
                log::warn!(
                    "sessions: failed to remove index rows for {}: {}",
                    session_id,
                    e
                );
            }
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

    #[test]
    fn sweeping_an_archive_removes_its_turns_rows() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Index a session's turns
        let session_id = "sweep-me";
        let msg = crate::memory::index::make_test_message_for_index(
            "user",
            "unique sweep target text",
            Some(1),
        );
        crate::daemon::session::append_archive_message(session_id, &msg);

        // Verify the turns are indexed
        let conn = crate::memory::index::open_index().unwrap();
        let turns_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'unique sweep target text'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(turns_count, 1, "turns should be indexed before sweep");

        let map_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns_map WHERE session_id = ?1",
                (session_id,),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(map_count, 1, "turns_map should have a row before sweep");

        // Make the archive old enough to expire
        let archive_path = crate::daemon::session::archive_file(session_id);
        set_file_mtime(&archive_path, 30);

        // Sweep with 14-day retention, no active sessions
        let active: HashSet<String> = HashSet::new();
        sweep_session_archives(14, &active);

        // Archive file should be gone
        assert!(!archive_path.exists(), "expired archive should be deleted");

        // Index rows should be gone too
        let conn = crate::memory::index::open_index().unwrap();
        let turns_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'unique sweep target text'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            turns_count, 0,
            "turns FTS rows should be removed after sweep"
        );

        let map_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns_map WHERE session_id = ?1",
                (session_id,),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(map_count, 0, "turns_map rows should be removed after sweep");

        // Restore HOME
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweeping_an_archive_leaves_other_sessions_indexed() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Index two sessions
        let msg1 = crate::memory::index::make_test_message_for_index(
            "user",
            "alpha session unique text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("alpha", &msg1);

        let msg2 = crate::memory::index::make_test_message_for_index(
            "user",
            "beta session unique text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("beta", &msg2);

        // Make alpha old, keep beta recent
        set_file_mtime(&crate::daemon::session::archive_file("alpha"), 30);

        let active: HashSet<String> = HashSet::new();
        sweep_session_archives(14, &active);

        // Alpha should be gone
        assert!(
            !crate::daemon::session::archive_file("alpha").exists(),
            "alpha archive should be deleted"
        );
        let conn = crate::memory::index::open_index().unwrap();
        let alpha_turns: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'alpha session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alpha_turns, 0, "alpha turns should be removed");

        // Beta should survive
        assert!(
            crate::daemon::session::archive_file("beta").exists(),
            "beta archive should survive"
        );
        let beta_turns: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'beta session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(beta_turns, 1, "beta turns should survive");

        let beta_map: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns_map WHERE session_id = 'beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(beta_map, 1, "beta map rows should survive");

        // Restore HOME
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn active_session_archive_keeps_its_rows() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let msg = crate::memory::index::make_test_message_for_index(
            "user",
            "active session preserved text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("active-keep", &msg);

        set_file_mtime(&crate::daemon::session::archive_file("active-keep"), 30);

        let mut active: HashSet<String> = HashSet::new();
        active.insert("active-keep".to_string());
        sweep_session_archives(14, &active);

        // File should survive
        assert!(
            crate::daemon::session::archive_file("active-keep").exists(),
            "active archive should survive"
        );

        // Index rows should survive
        let conn = crate::memory::index::open_index().unwrap();
        let turns_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'active session preserved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(turns_count, 1, "active session turns should survive");

        let map_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns_map WHERE session_id = 'active-keep'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(map_count, 1, "active session map rows should survive");

        // Restore HOME
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn zero_retention_removes_no_rows() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let msg = crate::memory::index::make_test_message_for_index(
            "user",
            "zero retention safe text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("zero-ret", &msg);

        set_file_mtime(&crate::daemon::session::archive_file("zero-ret"), 30);

        let active: HashSet<String> = HashSet::new();
        sweep_session_archives(0, &active);

        // File should survive
        assert!(
            crate::daemon::session::archive_file("zero-ret").exists(),
            "file should survive with retention_days=0"
        );

        // Index rows should survive
        let conn = crate::memory::index::open_index().unwrap();
        let turns_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE turns MATCH 'zero retention safe'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(turns_count, 1, "turns should survive with retention_days=0");

        // Restore HOME
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_survives_unwritable_index() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Index a session
        let msg = crate::memory::index::make_test_message_for_index(
            "user",
            "unwritable index sweep text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("unwritable", &msg);

        set_file_mtime(&crate::daemon::session::archive_file("unwritable"), 30);

        // Make the index directory unwritable
        let index_path = crate::config::memory_index_path();
        let index_dir = index_path.parent().unwrap();
        let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Sweep should still succeed (unlink file, fail to remove index rows)
        let active: HashSet<String> = HashSet::new();
        sweep_session_archives(14, &active);

        // File should be gone
        assert!(
            !crate::daemon::session::archive_file("unwritable").exists(),
            "file should be deleted even when index is unwritable"
        );

        // Restore permissions
        std::fs::set_permissions(index_dir, original_perms).unwrap();

        // Restore HOME
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_then_reconcile_agree() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Index two sessions
        let msg1 = crate::memory::index::make_test_message_for_index(
            "user",
            "reconcile alpha text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("recon-alpha", &msg1);

        let msg2 = crate::memory::index::make_test_message_for_index(
            "user",
            "reconcile beta text",
            Some(1),
        );
        crate::daemon::session::append_archive_message("recon-beta", &msg2);

        // Make alpha old
        set_file_mtime(&crate::daemon::session::archive_file("recon-alpha"), 30);

        let active: HashSet<String> = HashSet::new();
        sweep_session_archives(14, &active);

        // Get per_corpus counts after sweep
        let conn = crate::memory::index::open_index().unwrap();
        let turns_after_sweep: i64 = conn
            .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
            .unwrap();

        // Run reconcile
        let report = crate::memory::index::reconcile_index().unwrap();

        // Find turns count in reconcile report
        let turns_reconciled = report
            .per_corpus
            .iter()
            .find(|(name, _)| name == "turns")
            .map(|(_, c)| *c)
            .unwrap_or(0);

        assert_eq!(
            turns_after_sweep as usize, turns_reconciled,
            "reconcile turns count should match post-sweep count"
        );

        // Restore HOME
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
