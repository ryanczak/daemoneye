pub use crate::util::UnpoisonExt;

mod event_log;
mod host;
mod keepalive;
mod log_rotation;
mod output;
mod response;
mod shell;
mod sudo;
mod warnings;

pub use event_log::*;
pub use host::*;
pub use keepalive::*;
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

/// Delete an inactive session's chat log (`<id>.jsonl`) and its resume state
/// (`<id>.meta.json`) when the log's mtime is older than `retention_days`
/// days. 0 = keep forever (no-op). Archives (`<id>.archive.jsonl`) and epoch
/// files (`<id>.epochs.jsonl`) are never touched: they also end in `.jsonl`,
/// and they feed the memory index.
pub fn sweep_session_logs(
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
        if !name.ends_with(".jsonl")
            || name.ends_with(".archive.jsonl")
            || name.ends_with(".epochs.jsonl")
        {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        // Extract session id from `<id>.jsonl`
        let session_id = name.trim_end_matches(".jsonl");
        if active_sessions.contains(session_id) {
            continue;
        }

        log::info!("sessions: deleting expired chat log {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("sessions: failed to delete log {}: {}", path.display(), e);
            continue;
        }
        let meta_path = sessions_dir.join(format!("{}.meta.json", session_id));
        if meta_path.exists()
            && let Err(e) = std::fs::remove_file(&meta_path)
        {
            log::warn!(
                "sessions: failed to delete resume state {}: {}",
                meta_path.display(),
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

/// Delete asciicast logs (`*.cast`) and their `.meta.json` sidecars in
/// `var/log/shells/` whose mtime is older than `retention_days` days.
/// 0 = keep forever (no-op). A log whose shell is still registered is never
/// deleted, however old it is — a long-running shell that has been quiet past
/// the retention still owns its recording. A file whose stem does not start
/// with a valid shell id is kept too: if ownership cannot be read off the
/// name, the file cannot be proven not to belong to a live shell.
pub fn sweep_shell_logs(retention_days: u32, live_ids: &std::collections::BTreeSet<String>) {
    if retention_days == 0 {
        return;
    }

    let shells_dir = crate::config::shell_logs_dir();
    let Ok(entries) = std::fs::read_dir(&shells_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `meta_path_for` writes `<stem>.meta.json` (the producer's spelling);
        // a `.cast.meta.json` name is the legacy spelling the sweep once
        // recognised. Note `.cast` is checked after the two `.meta.json`
        // forms, so the arm order matches suffix specificity.
        let (extension, stem) = if let Some(stem) = name.strip_suffix(".cast.meta.json") {
            (".meta.json", stem)
        } else if let Some(stem) = name.strip_suffix(".meta.json") {
            (".meta.json", stem)
        } else if let Some(stem) = name.strip_suffix(".cast") {
            (".cast", stem)
        } else {
            continue;
        };

        // The id is the part before the first hyphen, mirroring the cast-name
        // grammar (`<id>-<started_unix>[.label].cast`).
        let Some((id_part, _)) = stem.split_once('-') else {
            continue;
        };
        if crate::shell::ShellId::parse(id_part).is_err() {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        if live_ids.contains(id_part) {
            continue;
        }

        log::info!(
            "shells: deleting expired {extension} log {}",
            path.display()
        );
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("shells: failed to delete {}: {}", path.display(), e);
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
    use std::collections::{BTreeSet, HashSet};

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

    /// Backdate a file with std only. `filetime` is a dependency of this
    /// crate, but `FileTimes::set_modified` requires a writable handle —
    /// missing it fails on a read-only `.cast`, so go through an explicit
    /// `OpenOptions::write(true)` handle instead.
    fn backdate(path: &std::path::Path, days_ago: u64) {
        let mtime =
            std::time::SystemTime::now() - std::time::Duration::from_secs(days_ago * 86_400);
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(mtime))
            .unwrap();
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
    fn sweep_logs_deletes_expired_inactive_logs_and_their_meta() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let log = sessions_dir.join("old.jsonl");
        let meta = sessions_dir.join("old.meta.json");
        std::fs::write(&log, "{\"turn\":1}\n").unwrap();
        std::fs::write(&meta, "{\"started_at\":0}\n").unwrap();
        set_file_mtime(&log, 30);
        set_file_mtime(&meta, 30);

        let empty: HashSet<String> = HashSet::new();
        sweep_session_logs(14, &empty);

        assert!(!log.exists(), "expired inactive chat log should be deleted");
        assert!(
            !meta.exists(),
            "expired session's resume state should go with the log"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_logs_never_touches_archives_or_epochs() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let archive = sessions_dir.join("old.archive.jsonl");
        let epochs = sessions_dir.join("old.epochs.jsonl");
        std::fs::write(&archive, "{\"turn\":1}\n").unwrap();
        std::fs::write(&epochs, "{\"epoch\":1}\n").unwrap();
        set_file_mtime(&archive, 30);
        set_file_mtime(&epochs, 30);

        let empty: HashSet<String> = HashSet::new();
        sweep_session_logs(14, &empty);

        assert!(
            archive.exists(),
            "indexed archive file must never be swept by the log sweep"
        );
        assert!(
            epochs.exists(),
            "indexed epochs file must never be swept by the log sweep"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_logs_respects_active_recent_and_zero() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let active = sessions_dir.join("active.jsonl");
        let recent = sessions_dir.join("recent.jsonl");
        std::fs::write(&active, "{\"turn\":1}\n").unwrap();
        std::fs::write(&recent, "{\"turn\":1}\n").unwrap();
        set_file_mtime(&active, 30);

        let mut active_ids: HashSet<String> = HashSet::new();
        active_ids.insert("active".to_string());

        sweep_session_logs(14, &active_ids);
        assert!(
            active.exists(),
            "an active session's log survives even when expired"
        );
        assert!(recent.exists(), "a recent log survives");

        let stale = sessions_dir.join("stale.jsonl");
        std::fs::write(&stale, "{\"turn\":1}\n").unwrap();
        set_file_mtime(&stale, 30);
        sweep_session_logs(0, &active_ids);
        assert!(stale.exists(), "retention_days=0 should be a no-op");

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
    fn utils_sweep_shell_logs_deletes_an_old_cast_and_its_sidecar() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let logs_dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&logs_dir).unwrap();

        let cast = logs_dir.join("s7-1788699323-shell.cast");
        let sidecar = crate::shell::meta_path_for(&cast);
        let sidecar = logs_dir.join(sidecar.file_name().unwrap());
        std::fs::write(&cast, b"{\"version\": 2}").unwrap();
        std::fs::write(&sidecar, b"{\"shell_id\": \"s7\"}").unwrap();
        backdate(&cast, 30);
        backdate(&sidecar, 30);

        sweep_shell_logs(7, &BTreeSet::new());

        assert!(
            !cast.exists(),
            "an expired .cast past retention must be swept"
        );
        assert!(
            !sidecar.exists(),
            "an expired .meta.json sidecar past retention must be swept"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn utils_sweep_shell_logs_deletes_a_fresh_spelled_sidecar_but_keeps_a_young_one() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let logs_dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&logs_dir).unwrap();

        let cast = logs_dir.join("s4-1788699323.cast");
        let sidecar = crate::shell::meta_path_for(&cast);
        let sidecar = logs_dir.join(sidecar.file_name().unwrap());
        std::fs::write(&sidecar, b"{\"shell_id\": \"s4\"}").unwrap();

        // The expired sidecar is swept under the producer's spelling.
        backdate(&sidecar, 30);
        sweep_shell_logs(7, &BTreeSet::new());
        assert!(
            !sidecar.exists(),
            "an expired <stem>.meta.json sidecar past retention must be swept"
        );

        // A young sidecar at the producer's spelling survives retention.
        std::fs::write(&sidecar, b"{\"shell_id\": \"s4\"}").unwrap();
        sweep_shell_logs(7, &BTreeSet::new());
        assert!(
            sidecar.exists(),
            "a young <stem>.meta.json sidecar within retention must never be swept"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn utils_sweep_shell_logs_keeps_a_log_whose_shell_is_still_registered() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let logs_dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&logs_dir).unwrap();

        let cast = logs_dir.join("s3-1788699323.quiet.cast");
        std::fs::write(&cast, b"{\"version\": 2}").unwrap();
        backdate(&cast, 30);

        let mut live = BTreeSet::new();
        live.insert("s3".to_string());
        sweep_shell_logs(7, &live);

        assert!(
            cast.exists(),
            "a registered shell's recording must never be swept, however old"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn utils_sweep_shell_logs_zero_retention_is_a_no_op() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let logs_dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&logs_dir).unwrap();

        let cast = logs_dir.join("s9-1788699323.build.cast");
        std::fs::write(&cast, b"{\"version\": 2}").unwrap();
        backdate(&cast, 30);

        sweep_shell_logs(0, &BTreeSet::new());

        assert!(cast.exists(), "retention_days=0 must keep the log forever");

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
