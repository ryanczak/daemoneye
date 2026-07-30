//! Daemon-log rotation: pure file-shifting logic separated from
//! descriptor re-attachment so it can be tested without a running daemon.

use std::fs;
use std::path::Path;

/// Rotate a log file when it exceeds `max_size_bytes`.
///
/// Uses a rename-chain strategy:
///   `daemon.log` → `daemon.log.1` → `daemon.log.2` → … → `daemon.log.{max_keep}`
/// Files beyond `max_keep` are deleted. After the chain shifts, a fresh empty
/// `daemon.log` is created so the daemon can re-open it and `dup2` the new fd.
///
/// Returns `true` if a rotation was performed, `false` if the file was under
/// the bound or did not exist.
///
/// This function is intentionally pure — it touches only the files named by
/// `path` and its numbered successors. It does not call `dup2`, touch globals,
/// or depend on any daemon state.
pub fn rotate_log_file(path: &Path, max_size_bytes: u64, max_keep: u32) -> bool {
    // Check current size.
    let metadata = fs::metadata(path).ok();
    let current_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
    if current_size <= max_size_bytes {
        return false;
    }

    // Drop the oldest rotated file if it would exceed max_keep.
    let oldest = format!("{}.{}", path.display(), max_keep);
    let _ = fs::remove_file(&oldest);

    // Shift existing rotated files: .N → .N+1
    for n in (1..max_keep).rev() {
        let src = format!("{}.{}", path.display(), n);
        let dst = format!("{}.{}", path.display(), n + 1);
        let _ = fs::rename(&src, &dst);
    }

    // Move the current log to .1.
    let rotated = format!("{}.{}", path.display(), 1);
    if fs::rename(path, &rotated).is_err() {
        return false;
    }

    // Create a fresh empty log file.
    if fs::write(path, "").is_err() {
        return false;
    }

    true
}

/// Re-point stdout/stderr at `path` after a rotation.
///
/// The daemon logs through fds 1 and 2, which were `dup2`'d from the log file at
/// startup (`daemon/mod.rs`). After `rotate_log_file` renames the old inode away,
/// those fds still refer to it — so without this the daemon would keep writing
/// into the rotated file and the live log would stay empty.
///
/// Daemon-only: it mutates process-global descriptors and so is deliberately kept
/// out of `rotate_log_file`, which stays pure and testable.
pub fn reattach_log_fds(path: &Path) {
    let file = match fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            log::error!("log rotation: reopen {} failed: {e}", path.display());
            return;
        }
    };
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: dup2 onto the process's own stdout/stderr with a descriptor we own.
    unsafe {
        if libc::dup2(fd, 1) < 0 {
            log::error!("log rotation: dup2 -> stdout failed");
        }
        if libc::dup2(fd, 2) < 0 {
            log::error!("log rotation: dup2 -> stderr failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rotates_file_over_bound() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("daemon.log");

        // Write content over the 100-byte bound.
        let content = "x".repeat(200);
        fs::write(&log_path, &content).unwrap();

        let rotated = rotate_log_file(&log_path, 100, 3);
        assert!(rotated, "should have rotated");

        // .1 contains the old content.
        let rotated_path = dir.path().join("daemon.log.1");
        assert_eq!(fs::read_to_string(&rotated_path).unwrap(), content);

        // Live path is a fresh empty file.
        assert_eq!(fs::read_to_string(&log_path).unwrap(), "");

        // No .2 or .3 yet.
        assert!(!dir.path().join("daemon.log.2").exists());
    }

    #[test]
    fn leaves_file_under_bound_alone() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("daemon.log");

        // Write content under the 100-byte bound.
        fs::write(&log_path, "small").unwrap();

        let rotated = rotate_log_file(&log_path, 100, 3);
        assert!(!rotated, "should not have rotated");

        // Original content untouched.
        assert_eq!(fs::read_to_string(&log_path).unwrap(), "small");
        assert!(!dir.path().join("daemon.log.1").exists());
    }

    #[test]
    fn drops_files_beyond_keep_count() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("daemon.log");

        // Pre-populate .1, .2, .3 with known content (max_keep = 3).
        fs::write(dir.path().join("daemon.log.1"), "old1").unwrap();
        fs::write(dir.path().join("daemon.log.2"), "old2").unwrap();
        fs::write(dir.path().join("daemon.log.3"), "old3").unwrap();
        fs::write(&log_path, "x".repeat(500)).unwrap();

        rotate_log_file(&log_path, 100, 3);

        // old3 was dropped; .3 now holds what was in .2.
        // .3 now holds what was in .2.
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.3")).unwrap(),
            "old2"
        );

        // .2 now holds what was in .1.
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.2")).unwrap(),
            "old1"
        );

        // .1 holds the content that was in the live log.
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.1")).unwrap(),
            "x".repeat(500)
        );

        // Live log is fresh.
        assert_eq!(fs::read_to_string(&log_path).unwrap(), "");
    }

    #[test]
    fn no_op_when_file_does_not_exist() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("daemon.log");

        let rotated = rotate_log_file(&log_path, 100, 3);
        assert!(!rotated, "should not rotate missing file");
        assert!(!log_path.exists());
    }

    #[test]
    fn multiple_rotations_chain_correctly() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("daemon.log");

        // First rotation.
        fs::write(&log_path, "first".repeat(50)).unwrap();
        rotate_log_file(&log_path, 100, 3);
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.1")).unwrap(),
            "first".repeat(50)
        );

        // Second rotation.
        fs::write(&log_path, "second".repeat(50)).unwrap();
        rotate_log_file(&log_path, 100, 3);
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.2")).unwrap(),
            "first".repeat(50)
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.1")).unwrap(),
            "second".repeat(50)
        );

        // Third rotation.
        fs::write(&log_path, "third".repeat(50)).unwrap();
        rotate_log_file(&log_path, 100, 3);
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.3")).unwrap(),
            "first".repeat(50)
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.2")).unwrap(),
            "second".repeat(50)
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("daemon.log.1")).unwrap(),
            "third".repeat(50)
        );
    }
}
