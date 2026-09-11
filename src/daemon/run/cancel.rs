//! Run-keyed cooperative cancellation (M24 phase 04): a `CancelReason`-
//! carrying signal, a run registry, the `var/run/stop-<run>` filesystem
//! sentinel, and the watcher that polls it.
//!
//! `daemoneye stop <run>` works from any terminal, including when the daemon
//! is wedged, because it writes a sentinel file and exits — it never opens
//! the IPC socket. The daemon-side watcher polls the sentinel at
//! [`STOP_POLL_INTERVAL`] and flips the run's signal.
//!
//! The signal carries `Option<CancelReason>`, not a bool: a cancellation must
//! report *which* reason fired (design § 2.3, `Cancelled{reason, stage,
//! turns_done}`).
//!
//! This module deliberately duplicates `crate::daemon::cancel` (the
//! session-keyed chat-turn registry) rather than widening it: that channel
//! carries a `bool`, and changing it would touch the live chat-turn path for
//! no benefit here. When phase 06 makes a chat turn a `Run`, the two
//! collapse.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;

use crate::util::UnpoisonExt;

use super::{CancelReason, RunId};

static REGISTRY: OnceLock<Mutex<HashMap<String, RunCancelHandle>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, RunCancelHandle>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a run's cancel handle. Returns a guard that deregisters on drop,
/// so every exit path of the run cleans up.
pub fn register_run(id: &RunId) -> (RunCancelGuard, RunCancelSignal) {
    let (handle, signal) = RunCancelSignal::new();
    registry()
        .lock()
        .unwrap_or_log()
        .insert(id.as_str().to_string(), handle);
    (
        RunCancelGuard {
            run_id: id.as_str().to_string(),
        },
        signal,
    )
}

/// Flip the cancel signal for a registered run, naming the reason.
/// Returns whether such a run was found.
pub fn cancel_run(id: &RunId, reason: CancelReason) -> bool {
    match registry().lock().unwrap_or_log().get(id.as_str()) {
        Some(handle) => {
            handle.cancel(reason);
            true
        }
        None => false,
    }
}

/// Guard deregistering a run on drop.
pub struct RunCancelGuard {
    run_id: String,
}

impl Drop for RunCancelGuard {
    fn drop(&mut self) {
        registry().lock().unwrap_or_log().remove(&self.run_id);
    }
}

/// Handle that can flip a run's cancellation signal with a reason.
pub struct RunCancelHandle {
    tx: watch::Sender<Option<CancelReason>>,
}

impl RunCancelHandle {
    /// Flip the signal, carrying the reason. Ignores a send error from
    /// all-receivers-dropped.
    pub fn cancel(&self, reason: CancelReason) {
        let _ = self.tx.send(Some(reason));
    }
}

/// Observable side of a run's cancellation signal.
#[derive(Clone)]
pub struct RunCancelSignal {
    rx: watch::Receiver<Option<CancelReason>>,
}

impl RunCancelSignal {
    /// Create a fresh pair; the signal starts unset.
    pub fn new() -> (RunCancelHandle, RunCancelSignal) {
        let (tx, rx) = watch::channel::<Option<CancelReason>>(None);
        (RunCancelHandle { tx }, RunCancelSignal { rx })
    }

    /// The reason the run was cancelled with, or `None` while it still runs.
    pub fn reason(&self) -> Option<CancelReason> {
        self.rx.borrow().clone()
    }

    /// Whether the signal has been flipped at all.
    pub fn is_cancelled(&self) -> bool {
        self.reason().is_some()
    }

    /// Resolve to the reason that fired. If the handle is dropped before the
    /// flip, park forever — a dropped handle must not read as a cancellation.
    pub async fn cancelled(&mut self) -> CancelReason {
        loop {
            if let Some(reason) = self.reason() {
                return reason;
            }
            match self.rx.changed().await {
                Ok(_) => {}
                Err(_) => std::future::pending::<()>().await,
            }
        }
    }
}

/// The filename prefix of a run's stop sentinel: `var/run/stop-<run-id>`.
pub const STOP_SENTINEL_PREFIX: &str = "stop-";

/// Path of a run's stop sentinel under `config::var_run_dir()`.
///
/// Takes a [`RunId`], never a `&str`: the id becomes a path segment and
/// `RunId::parse` is the validation.
pub fn stop_sentinel_path(id: &RunId) -> PathBuf {
    crate::config::var_run_dir().join(format!("{STOP_SENTINEL_PREFIX}{id}"))
}

/// Whether the run's stop sentinel file exists right now.
pub fn sentinel_present(id: &RunId) -> bool {
    stop_sentinel_path(id).exists()
}

/// Remove the run's stop sentinel, best-effort. A failure to clean up is
/// warned about, not fatal — the next sweep clears the leftover.
pub fn clear_sentinel(id: &RunId) {
    let path = stop_sentinel_path(id);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Failed to remove stop sentinel {}: {}", path.display(), e);
    }
}

/// Whether a `var/run` filename is a run stop sentinel: only entries starting
/// with `stop-`. Everything else in that directory — `daemoneye.pid`,
/// `daemoneye.sock`, `schedules.json`, `pane_prefs.json`, the `shells/`
/// subtree — must survive the startup sweep, so the predicate is exact.
pub fn is_stop_sentinel_name(name: &str) -> bool {
    name.starts_with(STOP_SENTINEL_PREFIX)
}

/// Mark a run as cancelled by writing its stop sentinel. The one
/// string-accepting entry point: the raw id is parsed first, so a
/// caller-supplied traversal can never reach the filesystem.
pub fn request_stop(raw_id: &str) -> anyhow::Result<PathBuf> {
    let id = RunId::parse(raw_id)?;
    let dir = crate::config::var_run_dir();
    std::fs::create_dir_all(&dir)?;
    let path = stop_sentinel_path(&id);
    std::fs::File::create(&path)?;
    Ok(path)
}

/// How often the watcher checks for the sentinel. Stop latency is bounded by
/// this (a human waits at most this long after `daemoneye stop <run>`).
pub const STOP_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Watch one run's stop sentinel and cancel the registered run when it
/// appears.
///
/// `poll` is injectable for tests. The loop returns — without firing — as
/// soon as the run is no longer registered, so the task never outlives its
/// run: the run is gone, there is nobody left to hand the reason to.
pub async fn watch_stop_sentinel(id: RunId, poll: Duration) {
    loop {
        tokio::time::sleep(poll).await;
        if !registry().lock().unwrap_or_log().contains_key(id.as_str()) {
            return;
        }
        if sentinel_present(&id) {
            cancel_run(&id, CancelReason::UserStop);
            clear_sentinel(&id);
            return;
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Fixtures shared by `daemon::run`'s test modules (`cancel` and
    //! `active`).
    //!
    //! The run-id registry is process-global and the sentinel directory is
    //! `$HOME/.daemoneye/var/run`: with libtest running tests in parallel,
    //! those only stay consistent if every test gets a unique id and a
    //! throwaway HOME — and both must come from one shared definition or a
    //! copied one rots out from under the other user.

    use super::RunId;

    /// A run id no other test in this binary will ever register. The
    /// registries are process-globals and libtest runs these tests in
    /// parallel — every test that inserts into one must use its own id, or
    /// one test's registration is another test's flake.
    pub fn fresh_run_id() -> RunId {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        RunId::parse(&format!("deadc0de0000{n:04x}")).expect("fixture id is valid hex")
    }

    /// RAII-guarded HOME pointing at a fresh tempdir, so a test that writes
    /// a sentinel or a run log never touches the developer's real
    /// `~/.daemoneye`.
    pub(crate) struct HomeGuard {
        _lock: crate::TestHomeGuard,
        pub(crate) tmp: tempfile::TempDir,
    }

    impl HomeGuard {
        pub(crate) fn new() -> Self {
            let _lock = crate::test_home_guard();
            let tmp = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", tmp.path()) };
            HomeGuard { _lock, tmp }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{HomeGuard, fresh_run_id};
    use super::*;
    use std::path::MAIN_SEPARATOR as SEP;

    const RUN_ID_STR: &str = "0123456789abcdef";

    fn run_id() -> RunId {
        RunId::parse(RUN_ID_STR).expect("fixture id is valid hex")
    }

    #[test]
    fn stop_sentinel_path_is_var_run_stop_plus_the_id() {
        let _guard = HomeGuard::new();
        let path = stop_sentinel_path(&run_id());
        assert!(
            path.ends_with(format!("var{SEP}run{SEP}stop-{RUN_ID_STR}")),
            "the path must end in var/run/stop-<id>, got {path:?}"
        );
        assert!(
            path.starts_with(crate::config::var_run_dir()),
            "the sentinel must live under var_run_dir, got {path:?}"
        );
    }

    #[test]
    fn request_stop_rejects_an_id_that_is_not_a_run_id() {
        let guard = HomeGuard::new();
        let run_dir = crate::config::var_run_dir();
        std::fs::create_dir_all(&run_dir).expect("a fresh tempdir var/run");
        let before = list_var_run();
        for bad in [
            "../../etc/passwd",
            "a/b",
            "a\\b",
            "0123456789ABCDEF",
            "",
            "0123456789abcdeg",
            "0123456789abcde",
        ] {
            let err = request_stop(bad)
                .map(|_| ())
                .expect_err("a non-RunId must be rejected");
            assert!(
                !err.to_string().is_empty(),
                "the error must describe the rejection for {bad:?}"
            );
        }
        assert_eq!(
            list_var_run(),
            before,
            "a rejected id must not write anything under var/run"
        );
        assert!(guard.tmp.path().join(".daemoneye/var/run").exists());
    }

    fn list_var_run() -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(crate::config::var_run_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn request_stop_writes_a_sentinel_the_watcher_can_see() {
        let _guard = HomeGuard::new();
        let path = request_stop(RUN_ID_STR).expect("a valid id writes the sentinel");
        assert!(path.is_file(), "request_stop must leave a file at {path:?}");
        assert!(sentinel_present(&run_id()), "the watcher must then see it");
        clear_sentinel(&run_id());
        assert!(
            !sentinel_present(&run_id()),
            "clear_sentinel must remove what request_stop wrote"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn run_cancel_signal_reports_the_reason_that_fired() {
        let (handle, mut signal) = RunCancelSignal::new();
        assert!(signal.reason().is_none(), "a fresh signal is not cancelled");
        handle.cancel(CancelReason::UserStop);
        assert_eq!(
            signal.reason(),
            Some(CancelReason::UserStop),
            "reason() must carry the variant, not just presence"
        );
        let reason = signal.cancelled().await;
        assert_eq!(
            reason,
            CancelReason::UserStop,
            "cancelled() must resolve to the reason that fired"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn run_cancel_dropped_handle_does_not_cancel() {
        let (handle, signal) = RunCancelSignal::new();
        drop(handle);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        assert!(
            !signal.is_cancelled(),
            "a dropped handle must not read as a cancellation"
        );
        assert_eq!(
            signal.reason(),
            None,
            "dropping the sender must not inject a reason"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn register_run_then_cancel_run_flips_that_runs_signal() {
        let id = fresh_run_id();
        let (guard, mut signal) = register_run(&id);
        assert!(
            cancel_run(&id, CancelReason::UserStop),
            "a registered run must be cancellable"
        );
        let reason = signal.cancelled().await;
        assert_eq!(reason, CancelReason::UserStop);
        assert!(signal.is_cancelled());
        drop(guard);
    }

    #[test]
    fn cancel_run_on_an_unregistered_run_is_false() {
        let id = fresh_run_id();
        assert!(
            !cancel_run(&id, CancelReason::UserStop),
            "an unregistered run must report not-found, not panic"
        );
    }

    #[test]
    fn run_cancel_guard_drop_deregisters() {
        let id = fresh_run_id();
        let (guard, _signal) = register_run(&id);
        assert!(cancel_run(&id, CancelReason::ClaudeStop));
        drop(guard);
        assert!(
            !cancel_run(&id, CancelReason::ClaudeStop),
            "after the guard drops, the run must no longer be registered"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_fires_user_stop_and_clears_the_sentinel() {
        let _guard = HomeGuard::new();
        let id = run_id();
        let (run_guard, signal) = register_run(&id);
        let watcher = tokio::spawn(watch_stop_sentinel(id.clone(), Duration::from_millis(10)));
        request_stop(RUN_ID_STR).expect("sentinel written before the watcher runs");
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        drop(run_guard);
        assert!(
            watcher.is_finished(),
            "the watcher must return once the sentinel is present"
        );
        assert_eq!(
            signal.reason(),
            Some(CancelReason::UserStop),
            "the watcher must fire UserStop, not a bare flip"
        );
        assert!(
            !sentinel_present(&id),
            "the watcher must clear the sentinel it charged"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_exits_without_firing_when_the_run_is_gone() {
        let _guard = HomeGuard::new();
        let id = run_id();
        let (run_guard, mut signal) = register_run(&id);
        drop(run_guard);
        let watcher = tokio::spawn(watch_stop_sentinel(id, Duration::from_millis(10)));
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            watcher.is_finished(),
            "a watcher whose run is gone must stop polling and return"
        );
        assert!(
            !signal.is_cancelled(),
            "exiting without a sentinel must not fire the signal"
        );
        assert!(
            signal.reason().is_none(),
            "exit must not carry a reason, only a sentinel fire may"
        );
        tokio::time::timeout(Duration::ZERO, signal.cancelled())
            .await
            .expect_err(
                "the receiver must be paired with the handle the run registered: \
                 if this timeout stops resolving, the signal was orphaned and the \
                 assertions above are vacuous",
            );
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_cancel_reaches_the_registered_runs_signal() {
        let _guard = HomeGuard::new();
        let id = fresh_run_id();
        let (run_guard, signal) = register_run(&id);
        let watcher = tokio::spawn(watch_stop_sentinel(id.clone(), Duration::from_millis(10)));
        request_stop(id.as_str()).expect("sentinel written before the watcher runs");
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            watcher.is_finished(),
            "the watcher must return once the sentinel is present"
        );
        assert_eq!(
            signal.reason(),
            Some(CancelReason::UserStop),
            "the registered run's signal must report UserStop — the watcher and \
             the turn share one channel"
        );
        drop(run_guard);
        assert!(
            !sentinel_present(&id),
            "the watcher must still clear the sentinel it charged"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_on_an_unregistered_run_cancels_nothing() {
        let _guard = HomeGuard::new();
        let id = fresh_run_id();
        let other = fresh_run_id();
        let (_other_guard, other_signal) = register_run(&other);
        let _ = &other_signal;
        request_stop(id.as_str()).expect("sentinel written before the watcher runs");
        let watcher = tokio::spawn(watch_stop_sentinel(id.clone(), Duration::from_millis(10)));
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            watcher.is_finished(),
            "a watcher for a run that was never registered must exit at its first poll"
        );
        assert_eq!(
            other_signal.reason(),
            None,
            "a sentinel naming an unregistered run must not fire any registered run"
        );
        assert!(
            sentinel_present(&id),
            "the watcher exiting early must leave the unregistered run's sentinel \
             to the sweep"
        );
    }

    #[test]
    fn sweep_predicate_matches_only_stop_sentinels() {
        assert!(
            is_stop_sentinel_name(&format!("stop-{RUN_ID_STR}")),
            "a real sentinel name must be matched"
        );
        for neighbour in [
            "daemoneye.pid",
            "daemoneye.sock",
            "schedules.json",
            "pane_prefs.json",
            "shells",
            "proxy",
            "",
        ] {
            assert!(
                !is_stop_sentinel_name(neighbour),
                "{neighbour:?} must survive the sweep"
            );
        }
        assert!(
            !is_stop_sentinel_name("st0p-"),
            "a name not starting with the literal prefix must not match"
        );
    }
}
