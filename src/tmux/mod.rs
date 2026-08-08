mod ansi;
pub mod cache;
pub mod pane;
pub mod session;
pub mod status;
pub mod window;

pub use pane::*;
pub use session::*;
pub use window::*;

/// Ceiling for a single tmux subprocess call made from async code.
///
/// tmux normally answers in milliseconds; five seconds means the server is
/// wedged, and waiting longer cannot help the caller.
pub const TMUX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run a blocking `tmux` helper off the async runtime, bounded by [`TMUX_TIMEOUT`].
///
/// The `src/tmux/` helpers are synchronous `std::process::Command` calls: invoked
/// directly from an `async fn` they block a tokio worker until tmux answers, and
/// a wedged tmux server therefore stalls the whole daemon. This moves the call to
/// the blocking pool and gives up on it after the timeout, so a wedge degrades
/// one operation instead of the reactor. See `docs/design/daemon-stalls.md`
/// § 1 mechanism B.
///
/// Returns `None` if the call timed out or the blocking task panicked — both are
/// logged. `Some(v)` carries whatever the helper returned, including its own
/// `Err`.
pub async fn off_runtime<T, F>(what: &'static str, f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(TMUX_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            log::error!("tmux {what}: blocking task panicked: {e}");
            None
        }
        Err(_) => {
            log::error!(
                "tmux {what}: timed out after {TMUX_TIMEOUT:?} — tmux server may be wedged"
            );
            None
        }
    }
}

/// How often [`bounded_output_with`] checks whether the child has exited.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Run a command to completion, killing it if it outlives `timeout`.
///
/// A drop-in replacement for [`std::process::Command::output`] for the
/// synchronous `src/tmux/` helpers: the callers that cannot be wrapped in
/// [`off_runtime`] — `Drop` impls, which cannot be `async`, and the CLI, which
/// has no runtime to protect — still get a bound, with no call-site churn.
///
/// stdout and stderr are drained on **their own threads**. Polling `try_wait`
/// while the pipes go unread would deadlock on any command whose output exceeds
/// the OS pipe buffer (~64 KiB) — `tmux capture-pane -S -` dumps the entire
/// scrollback and routinely does.
///
/// On timeout the child is killed and reaped, and the error is
/// [`std::io::ErrorKind::TimedOut`].
pub fn bounded_output_with(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let status = child.wait()?;
                    let _ = out_reader.join();
                    let _ = err_reader.join();
                    let _ = status;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "tmux command timed out",
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    };

    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// [`bounded_output_with`] at the standard [`TMUX_TIMEOUT`].
pub fn bounded_output(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    bounded_output_with(cmd, TMUX_TIMEOUT)
}

#[cfg(test)]
mod bounded_output_tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn bounded_output_returns_stdout_and_success() {
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "printf hello"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
    }

    #[test]
    fn bounded_output_preserves_failure_status() {
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "exit 3"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!out.status.success());
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn bounded_output_captures_stderr() {
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "printf oops 1>&2"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "oops");
    }

    /// THE regression test: output far larger than the OS pipe buffer must NOT
    /// time out. A try_wait loop that does not drain the pipes deadlocks here.
    #[test]
    fn bounded_output_handles_output_larger_than_pipe_buffer() {
        // ~1 MiB, well past the ~64 KiB pipe buffer.
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "yes abcdefghijklmnopqrstuvwxyz | head -c 1048576"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 1_048_576);
    }

    #[test]
    fn bounded_output_times_out_and_kills_the_child() {
        let start = Instant::now();
        let err = bounded_output_with(
            Command::new("sh").args(["-c", "sleep 30"]),
            Duration::from_millis(200),
        )
        .unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < Duration::from_secs(5),
            "returned in {elapsed:?}, should be ~200ms"
        );
    }
}
