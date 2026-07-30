use crate::util::UnpoisonExt;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Mutex;

/// What the parent learned from the forked child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildReport {
    /// Child bound its socket and is serving.
    Ready,
    /// Child reported a startup failure.
    Failed(String),
    /// Child exited without reporting. Its own log is the only record.
    Died,
}

static REPORTER: Mutex<Option<OwnedFd>> = Mutex::new(None);

/// Create the readiness pipe. Returns `(read_end, write_end)`.
pub fn create_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as std::os::fd::RawFd; 2];
    // SAFETY: `fds` is a two-element array of RawFd, exactly what pipe(2)
    // requires. On success the kernel has written two fresh descriptors we take
    // sole ownership of via OwnedFd.
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created by pipe(2) and are not owned
    // elsewhere.
    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

/// Install the write end as this process's reporter. Called in the child.
pub fn set_reporter(fd: OwnedFd) {
    let mut guard = REPORTER.lock().unwrap_or_log();
    *guard = Some(fd);
}

/// Report success, then release the reporter. No-op when none is installed
/// (`--console`, or any non-forking entry point).
pub fn report_ready() {
    let mut guard = REPORTER.lock().unwrap_or_log();
    let fd = match guard.take() {
        Some(fd) => fd,
        None => return,
    };
    drop(guard);
    let mut file = std::fs::File::from(fd);
    let _ = file.write_all(b"READY\n");
}

/// Report a startup failure, then release the reporter. No-op when none is
/// installed.
pub fn report_failure(msg: &str) {
    let mut guard = REPORTER.lock().unwrap_or_log();
    let fd = match guard.take() {
        Some(fd) => fd,
        None => return,
    };
    drop(guard);
    let sanitized = msg.replace(['\n', '\r'], " ");
    let payload = format!("ERR {}\n", sanitized);
    let mut file = std::fs::File::from(fd);
    let _ = file.write_all(payload.as_bytes());
}

/// Block until the child reports or the pipe reaches EOF. Called in the parent.
pub fn await_child_report(read_end: OwnedFd) -> ChildReport {
    let file = std::fs::File::from(read_end);
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => ChildReport::Died,
        Ok(_) => parse_report_line(&line),
        Err(_) => ChildReport::Died,
    }
}

/// Parse one protocol line. Pure — this is what the unit tests target.
pub fn parse_report_line(line: &str) -> ChildReport {
    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
    if trimmed == "READY" {
        ChildReport::Ready
    } else if let Some(rest) = trimmed.strip_prefix("ERR ") {
        ChildReport::Failed(rest.to_string())
    } else {
        ChildReport::Died
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static REPORTER_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn parses_ready() {
        assert_eq!(parse_report_line("READY\n"), ChildReport::Ready);
    }

    #[test]
    fn parses_failure_with_message() {
        assert_eq!(
            parse_report_line("ERR another daemon is already running (PID 42)\n"),
            ChildReport::Failed("another daemon is already running (PID 42)".to_string())
        );
    }

    #[test]
    fn parses_unknown_line_as_died() {
        assert_eq!(parse_report_line(""), ChildReport::Died);
        assert_eq!(parse_report_line("\n"), ChildReport::Died);
        assert_eq!(parse_report_line("READYISH\n"), ChildReport::Died);
        assert_eq!(parse_report_line("ERR\n"), ChildReport::Died);
        assert_eq!(parse_report_line("ready\n"), ChildReport::Died);
    }

    #[test]
    fn await_report_reads_ready_then_returns() {
        // Keeps the write end alive after writing — read_line must return
        // without blocking. If this were read_to_string instead, it would hang
        // because the child never closes its write end during normal operation.
        let _lock = REPORTER_TEST_LOCK.lock().unwrap();
        let (read_end, write_end) = create_pipe().unwrap();
        let mut file = std::fs::File::from(write_end.try_clone().unwrap());
        file.write_all(b"READY\n").unwrap();
        // Keep write_end alive so read_to_string would block
        drop(file);
        let report = await_child_report(read_end);
        assert_eq!(report, ChildReport::Ready);
        drop(write_end);
    }

    #[test]
    fn await_report_returns_died_on_eof() {
        let _lock = REPORTER_TEST_LOCK.lock().unwrap();
        let (read_end, write_end) = create_pipe().unwrap();
        drop(write_end);
        let report = await_child_report(read_end);
        assert_eq!(report, ChildReport::Died);
    }

    #[test]
    fn await_report_returns_failure() {
        let _lock = REPORTER_TEST_LOCK.lock().unwrap();
        let (read_end, write_end) = create_pipe().unwrap();
        let mut file = std::fs::File::from(write_end);
        file.write_all(b"ERR boom\n").unwrap();
        drop(file);
        let report = await_child_report(read_end);
        assert_eq!(report, ChildReport::Failed("boom".to_string()));
    }

    #[test]
    fn report_ready_without_reporter_is_a_noop() {
        let _lock = REPORTER_TEST_LOCK.lock().unwrap();
        // Ensure no reporter is installed
        {
            let mut guard = REPORTER.lock().unwrap_or_log();
            *guard = None;
        }
        // Must not panic
        report_ready();
    }
}
