pub use crate::util::UnpoisonExt;

mod event_log;
mod host;
mod log_rotation;
mod output;
mod response;
mod shell;
mod sudo;

pub use event_log::*;
pub use host::*;
pub use log_rotation::*;
pub use output::*;
pub use response::*;
pub use shell::*;
pub use sudo::*;

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
    }
}
