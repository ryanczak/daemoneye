//! Instance lock — exclusive `flock` on a PID file to enforce single-instance.
//!
//! The lock is acquired before any startup side effect in `run_daemon`. The
//! kernel releases it on process death, so there is no stale-lock recovery path.
//! The PID written into the file is diagnostic payload only — never branch on it.

use std::fs::OpenOptions;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

/// Exclusive instance lock backed by `flock`.
///
/// Owns the `Flock<File>` for its entire lifetime — the lock releases when
/// the `Flock` drops. Does not remove the PID file on drop to avoid a race
/// with a successor that may already have created and locked its own.
#[derive(Debug)]
pub struct InstanceLock {
    _guard: Flock<std::fs::File>,
    pid_path: PathBuf,
}

/// Error when the instance lock cannot be acquired.
#[derive(Debug)]
pub enum AcquireError {
    /// Another daemon holds the lock. `pid` is its PID if the file was readable
    /// and parsable; `None` when the payload was absent or malformed.
    Held { pid: Option<u32> },
    /// The lock file could not be opened or written.
    Io(std::io::Error),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcquireError::Held { pid: Some(p) } => {
                write!(
                    f,
                    "another daemon is already running (PID {p}) — stop it with: daemoneye stop"
                )
            }
            AcquireError::Held { pid: None } => {
                write!(
                    f,
                    "another daemon is already running — stop it with: daemoneye stop"
                )
            }
            AcquireError::Io(e) => write!(f, "could not acquire the instance lock: {e}"),
        }
    }
}

impl std::error::Error for AcquireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AcquireError::Held { .. } => None,
            AcquireError::Io(e) => Some(e),
        }
    }
}

impl InstanceLock {
    /// Acquire an exclusive lock on `path`, writing this process's PID into it.
    pub fn acquire(path: &Path) -> Result<Self, AcquireError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(AcquireError::Io)?;

        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(guard) => {
                let mut locked = guard;
                use std::io::{Seek, Write};
                locked.set_len(0).map_err(AcquireError::Io)?;
                locked.rewind().map_err(AcquireError::Io)?;
                writeln!(&mut locked, "{}", std::process::id()).map_err(AcquireError::Io)?;
                locked.flush().map_err(AcquireError::Io)?;

                Ok(InstanceLock {
                    _guard: locked,
                    pid_path: path.to_path_buf(),
                })
            }
            Err((back, Errno::EWOULDBLOCK)) => {
                let pid = read_pid_from_file(&back);
                Err(AcquireError::Held { pid })
            }
            Err((_, errno)) => Err(AcquireError::Io(std::io::Error::from(errno))),
        }
    }

    /// Path to the PID / lock file.
    pub fn pid_path(&self) -> &Path {
        &self.pid_path
    }
}

/// Reads the PID payload without taking the lock. Returns `None` if the file is
/// absent, unreadable, empty, or not a bare integer.
pub fn read_pid(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    read_pid_from_file(&file)
}

/// Read PID from an already-open file. Used internally after a contended lock
/// where we still have the file descriptor.
fn read_pid_from_file(file: &std::fs::File) -> Option<u32> {
    let mut bufreader = std::io::BufReader::new(file);
    let mut line = String::new();
    bufreader.read_line(&mut line).ok()?;
    line.trim().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_writes_own_pid() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.pid");
        let lock = InstanceLock::acquire(&path).unwrap();
        assert_eq!(read_pid(&path), Some(std::process::id()));
        assert_eq!(lock.pid_path(), path);
        drop(lock);
    }

    #[test]
    fn second_acquire_is_held_with_pid() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.pid");
        let first = InstanceLock::acquire(&path).unwrap();
        let err = InstanceLock::acquire(&path).unwrap_err();
        match err {
            AcquireError::Held { pid } => {
                assert_eq!(pid, Some(std::process::id()));
            }
            other => panic!("expected Held, got {other:?}"),
        }
        drop(first);
    }

    #[test]
    fn acquire_succeeds_after_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.pid");
        let first = InstanceLock::acquire(&path).unwrap();
        drop(first);
        let second = InstanceLock::acquire(&path).unwrap();
        assert_eq!(read_pid(&path), Some(std::process::id()));
        drop(second);
    }

    #[test]
    fn held_error_reports_none_for_unparsable_payload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.pid");
        // Pre-write garbage.
        std::fs::write(&path, "garbage").unwrap();
        // Take a lock by hand so the file is locked.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut guard = Flock::lock(file, FlockArg::LockExclusive).unwrap();
        // Write garbage into the locked file.
        use std::io::{Seek, Write};
        guard.set_len(0).unwrap();
        guard.rewind().unwrap();
        guard.write_all(b"garbage").unwrap();
        guard.flush().unwrap();
        // Now try to acquire — should get Held with pid: None.
        let err = InstanceLock::acquire(&path).unwrap_err();
        match err {
            AcquireError::Held { pid } => {
                assert!(
                    pid.is_none(),
                    "expected None for unparsable payload, got {pid:?}"
                );
            }
            other => panic!("expected Held, got {other:?}"),
        }
        drop(guard);
    }

    #[test]
    fn failed_acquire_preserves_incumbent_payload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.pid");
        let first = InstanceLock::acquire(&path).unwrap();
        let first_pid = std::process::id();
        // Second acquire fails but must not truncate the first's PID.
        let _err = InstanceLock::acquire(&path).unwrap_err();
        assert_eq!(
            read_pid(&path),
            Some(first_pid),
            "incumbent PID must survive a failed acquisition"
        );
        drop(first);
    }

    #[test]
    fn read_pid_returns_none_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.pid");
        assert!(read_pid(&path).is_none());
    }
}
