mod ansi;
pub mod cache;
pub mod pane;
pub mod session;
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
