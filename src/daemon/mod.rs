use crate::config::default_socket_path;
use crate::ipc::{Request, Response};
pub use crate::tmux::cache::SessionCache;
pub use crate::util::UnpoisonExt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Timestamp of when `run_daemon` started. Initialised once at daemon startup.
static DAEMON_START: OnceLock<Instant> = OnceLock::new();

/// Returns the number of seconds since the daemon started, or 0 before init.
pub fn daemon_uptime_secs() -> u64 {
    DAEMON_START
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0)
}

pub mod auto_name;
pub mod briefing;
pub mod context;
pub mod digest;
pub mod executor;
pub mod ghost;
pub mod instance;
pub mod memory_prompt;
pub mod policy;
pub mod prompt;
pub mod ready;
pub mod run;
pub mod scheduled;
pub mod server;
pub mod session;
pub mod situational;
pub mod spend;
mod startup;
pub mod stats;
pub mod stream;
pub mod utils;
pub use startup::run_daemon;

/// Shared prefix for all daemon-managed tmux windows.  Used by the CLI to
/// filter windows from `tmux list-windows` output.
pub const DAEMON_WINDOW_PREFIX: &str = "de-";
/// Window-name prefix for background execution windows.
/// Format: `de-bg-<pane_num>-<unix_ts>-<cmd_slug>`, e.g. `de-bg-42-1712937600-cargo-build`.
pub const BG_WINDOW_PREFIX: &str = "de-bg-";
/// Window-name prefix for regular scheduled-job windows.
/// Format: `de-sj-<pane_num>-<unix_ts>-<cmd_slug>`, e.g. `de-sj-43-1712937600-backup.sh`.
pub const SCHED_WINDOW_PREFIX: &str = "de-sj-";
/// Window-name prefix for ghost-shell background execution windows.
/// Format: `de-gs-bg-<pane_num>-<unix_ts>-<cmd_slug>`.
/// Used when a ghost is triggered by a webhook or interactive `spawn_ghost_shell`.
pub const GS_BG_WINDOW_PREFIX: &str = "de-gs-bg-";
/// Window-name prefix for ghost-shell scheduled-job windows.
/// Format: `de-gs-sj-<pane_num>-<unix_ts>-<cmd_slug>`.
/// Used when a ghost is triggered by a scheduled job (`ActionOn::Ghost`).
pub const GS_SCHED_WINDOW_PREFIX: &str = "de-gs-sj-";
/// Window-name prefix for ghost-shell incident-response (main session) windows (`de-gs-ir-<ts>-<id>`).
pub const INCIDENT_WINDOW_PREFIX: &str = "de-gs-ir-";

/// True when `window_name` is a window this daemon created and manages.
///
/// The single source of truth for that question (M12 D6). Note it deliberately
/// does **not** use `DAEMON_WINDOW_PREFIX` (`"de-"`): that would also match a
/// user's own window called `de-icing`.
pub fn is_daemon_window(window_name: &str) -> bool {
    window_name.starts_with(BG_WINDOW_PREFIX)
        || window_name.starts_with(SCHED_WINDOW_PREFIX)
        || window_name.starts_with(GS_BG_WINDOW_PREFIX)
        || window_name.starts_with(GS_SCHED_WINDOW_PREFIX)
        || window_name.starts_with(INCIDENT_WINDOW_PREFIX) // all five daemon prefixes
}

/// True when `window_name` belongs to a Ghost Shell specifically — a strict
/// subset of [`is_daemon_window`]. `de-bg-` and `de-sj-` are daemon windows but
/// not ghost windows.
pub fn is_ghost_window(window_name: &str) -> bool {
    window_name.starts_with(GS_BG_WINDOW_PREFIX)
        || window_name.starts_with(GS_SCHED_WINDOW_PREFIX)
        || window_name.starts_with(INCIDENT_WINDOW_PREFIX)
}

/// True when `session_id` names a Ghost Shell session. Ghost session ids are
/// built as `ghost-<alert>-<uuid>` (`daemon/ghost.rs`). This is the *session*
/// namespace, unrelated to the `de-gs-*` window prefixes [`is_ghost_window`]
/// matches.
pub fn is_ghost_session_id(session_id: &str) -> bool {
    session_id.starts_with("ghost-")
}

/// Resolve whether a background job belongs to a ghost session.
/// `entry_is_ghost` is the authoritative `SessionEntry.is_ghost` when the
/// session has an entry; when it does not, fall back to the id prefix rather
/// than silently answering `false` and dropping the ghost label.
pub fn resolve_is_ghost(session_id: Option<&str>, entry_is_ghost: Option<bool>) -> bool {
    match entry_is_ghost {
        Some(known) => known,
        None => session_id.is_some_and(is_ghost_session_id),
    }
}

/// True when a pane may be offered to the user or the agent as a target: not
/// daemon-managed, and not the chat pane itself.
pub fn is_targetable_pane(window_name: &str, pane_id: &str, chat_pane: Option<&str>) -> bool {
    !is_daemon_window(window_name) && chat_pane != Some(pane_id) // never target the chat pane
}

pub use scheduled::run_scheduled_job;
pub use server::*;

pub use session::*;

pub use utils::*;

/// Supervise a long-lived daemon task, restarting it with exponential backoff
/// on panic or unexpected exit (A1).
///
/// `factory` is called each time the task is (re-)started; it must produce a
/// fresh `Future<Output = ()>`.  The supervisor exits cleanly when `shutdown`
/// is `true` — either set before the first call or by the factory itself.
///
/// Backoff schedule: 1 s → 2 s → 4 s → 8 s → 16 s → 30 s (cap).
/// The failure counter resets when a task runs stably for ≥ 60 s.
pub async fn supervise<F, Fut>(name: &'static str, shutdown: Arc<AtomicBool>, factory: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    const STABLE_THRESHOLD: Duration = Duration::from_secs(60);
    let mut attempt: u32 = 0;

    loop {
        let start = Instant::now();
        let handle = tokio::spawn(factory());

        match handle.await {
            Ok(()) => {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                log::warn!(
                    "Supervised task '{}' exited unexpectedly — restarting.",
                    name
                );
            }
            Err(e) if e.is_panic() => {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                let payload = e.into_panic();
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "(non-string panic payload)".to_string()
                };
                log::error!("Supervised task '{}' panicked: {} — restarting.", name, msg);
                log_event(
                    "task_panic",
                    serde_json::json!({ "task": name, "msg": msg }),
                );
            }
            Err(_) => {
                // Task was cancelled — expected during shutdown.
                return;
            }
        }

        // Reset the backoff if the task ran stably long enough before failing.
        if start.elapsed() >= STABLE_THRESHOLD {
            attempt = 0;
        }

        let delay = MAX_BACKOFF.min(Duration::from_secs(1u64 << attempt.min(4)));
        log::info!(
            "Restarting task '{}' in {:?} (attempt {}).",
            name,
            delay,
            attempt + 1
        );
        tokio::time::sleep(delay).await;
        attempt = attempt.saturating_add(1);

        if shutdown.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Detect the tmux session the daemon is running in, without creating one.
///
/// Returns the session name when the process is already inside an active tmux
/// session (e.g. the daemon was started manually from within tmux).  Returns
/// `None` when launched from outside tmux — the normal case for a systemd
/// user service that starts before the user logs in.
pub fn detect_session() -> Option<String> {
    if std::env::var("TMUX").is_err() {
        return None;
    }
    let out = match std::process::Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::error!("detect_session: $TMUX is set but `tmux display-message` failed: {e}");
            return None;
        }
    };
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// What a liveness probe against the daemon socket found.
///
/// This is a *report*, never an authorization. Nothing may unlink a socket,
/// remove a file, or otherwise act destructively on the strength of a variant
/// here — instance ownership is decided solely by the `InstanceLock`
/// (`docs/design/daemon-instance.md` § 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonLiveness {
    /// No socket file, or nothing listening on it.
    NotRunning,
    /// Connected, but the daemon did not answer `Ping` within the timeout.
    /// A live process that is wedged looks like this.
    Unresponsive,
    /// Connected and answered `Ping` with something other than `Response::Ok`.
    Confused,
    /// Connected and answered `Ping` with `Response::Ok`.
    Running,
}

/// Probe the daemon socket and report what was found.
/// Uses a 2-second timeout so a hung process doesn't block startup.
pub async fn daemon_liveness() -> DaemonLiveness {
    let Ok(stream) = tokio::net::UnixStream::connect(default_socket_path()).await else {
        return DaemonLiveness::NotRunning;
    };
    let (rx_half, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx_half);

    let Ok(mut data) = serde_json::to_vec(&Request::Ping) else {
        return DaemonLiveness::Confused;
    };
    data.push(b'\n');
    if tx.write_all(&data).await.is_err() {
        return DaemonLiveness::NotRunning;
    }

    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(2), rx.read_line(&mut line)).await {
        Ok(Ok(0)) => DaemonLiveness::NotRunning,
        Ok(Ok(_)) => match serde_json::from_str::<Response>(line.trim()) {
            Ok(Response::Ok) => DaemonLiveness::Running,
            _ => DaemonLiveness::Confused,
        },
        _ => DaemonLiveness::Unresponsive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::startup::tmux_wanted;
    use std::sync::atomic::AtomicU32;

    // ---------------------------------------------------------------------------
    // D6 predicate tests — pure, no tmux or cache
    // ---------------------------------------------------------------------------

    #[test]
    fn an_empty_session_name_means_no_tmux() {
        assert!(
            !tmux_wanted(None, ""),
            "an empty configured session name means no tmux"
        );
        assert!(
            !tmux_wanted(None, "   "),
            "an all-whitespace configured session name means no tmux"
        );
        assert!(
            !tmux_wanted(Some(""), "daemoneye"),
            "an empty --session override wins over a non-empty configured name"
        );
        assert!(
            tmux_wanted(None, "daemoneye"),
            "the configured default session name still wants tmux"
        );
        assert!(
            tmux_wanted(Some("work"), ""),
            "a non-empty --session override wants tmux even with an empty config"
        );
    }

    #[test]
    fn is_daemon_window_matches_all_five_prefixes() {
        assert!(is_daemon_window("de-bg-1-abc"), "de-bg- prefix");
        assert!(is_daemon_window("de-sj-1-abc"), "de-sj- prefix");
        assert!(is_daemon_window("de-gs-bg-1-abc"), "de-gs-bg- prefix");
        assert!(is_daemon_window("de-gs-sj-1-abc"), "de-gs-sj- prefix");
        assert!(is_daemon_window("de-gs-ir-1-abc"), "de-gs-ir- prefix");
    }

    #[test]
    fn is_daemon_window_rejects_user_windows() {
        assert!(!is_daemon_window("main"), "user window 'main'");
        assert!(!is_daemon_window("editor"), "user window 'editor'");
        assert!(
            !is_daemon_window("de-icing"),
            "user window 'de-icing' (starts with DAEMON_WINDOW_PREFIX)"
        );
    }

    #[test]
    fn is_ghost_window_matches_only_ghost_prefixes() {
        assert!(is_ghost_window("de-gs-bg-1-abc"), "de-gs-bg- is ghost");
        assert!(is_ghost_window("de-gs-sj-1-abc"), "de-gs-sj- is ghost");
        assert!(is_ghost_window("de-gs-ir-1-abc"), "de-gs-ir- is ghost");
        assert!(
            !is_ghost_window("de-bg-1-abc"),
            "de-bg- is daemon but not ghost"
        );
        assert!(
            !is_ghost_window("de-sj-1-abc"),
            "de-sj- is daemon but not ghost"
        );
    }

    #[test]
    fn is_ghost_session_id_matches_ghost_session_ids() {
        assert!(
            is_ghost_session_id("ghost-disk-full-ab12"),
            "ghost-<alert>-<uuid>"
        );
        assert!(is_ghost_session_id("ghost-x-0"), "minimal ghost id");
    }

    #[test]
    fn is_ghost_session_id_rejects_non_ghost_ids() {
        assert!(
            !is_ghost_session_id("sess-7f3a"),
            "normal session id is not ghost"
        );
        assert!(!is_ghost_session_id(""), "empty string is not ghost");
        assert!(
            !is_ghost_session_id("de-gs-bg-1-abc"),
            "ghost window name is not a ghost session id"
        );
    }

    #[test]
    fn resolve_is_ghost_prefers_the_session_entry() {
        assert!(
            !resolve_is_ghost(Some("ghost-disk-full-ab12"), Some(false)),
            "stored Some(false) beats the ghost- prefix"
        );
        assert!(
            resolve_is_ghost(Some("sess-7f3a"), Some(true)),
            "stored Some(true) beats a non-ghost id"
        );
    }

    #[test]
    fn resolve_is_ghost_falls_back_to_the_prefix_when_unknown() {
        assert!(
            resolve_is_ghost(Some("ghost-disk-full-ab12"), None),
            "unknown entry falls back to the ghost- prefix"
        );
        assert!(
            !resolve_is_ghost(Some("sess-7f3a"), None),
            "unknown entry with a normal id is not ghost"
        );
        assert!(
            !resolve_is_ghost(None, None),
            "no session id and no entry is not ghost"
        );
    }

    #[test]
    fn is_targetable_pane_excludes_daemon_and_chat() {
        // User window, non-chat pane → targetable
        assert!(
            is_targetable_pane("editor", "pane-1", Some("pane-2")),
            "user window non-chat pane is targetable"
        );
        // User window, chat pane → not targetable
        assert!(
            !is_targetable_pane("editor", "pane-1", Some("pane-1")),
            "chat pane is never targetable"
        );
        // Daemon window, not chat → not targetable
        assert!(
            !is_targetable_pane("de-bg-1-abc", "pane-3", Some("pane-1")),
            "daemon window is never targetable"
        );
    }

    #[test]
    fn is_targetable_pane_with_no_chat_pane() {
        assert!(
            is_targetable_pane("editor", "pane-1", None),
            "user window with no chat pane is targetable"
        );
    }

    // ---------------------------------------------------------------------------
    // Existing supervisor / liveness tests
    // ---------------------------------------------------------------------------

    /// Supervisor restarts the factory after a panic and exits cleanly once
    /// the factory signals shutdown.
    #[tokio::test(start_paused = true)]
    async fn supervise_restarts_on_panic() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let call_count = Arc::new(AtomicU32::new(0));

        let sd = Arc::clone(&shutdown);
        let cc = Arc::clone(&call_count);

        let handle = tokio::spawn(supervise(
            "test-restart",
            Arc::clone(&shutdown),
            move || {
                let count = Arc::clone(&cc);
                let sd2 = Arc::clone(&sd);
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        panic!("deliberate test panic");
                    }
                    // Second call: signal shutdown so the supervisor exits after us.
                    sd2.store(true, Ordering::Relaxed);
                }
            },
        ));

        // Advance time past the 1 s backoff so the supervisor restarts.
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;

        handle.await.expect("supervisor should complete cleanly");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "factory should be called exactly twice"
        );
    }

    /// Supervisor does not restart when the shutdown flag is already set
    /// at the time the managed task panics.
    #[tokio::test(start_paused = true)]
    async fn supervise_no_restart_when_shutdown() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let call_count = Arc::new(AtomicU32::new(0));

        let sd = Arc::clone(&shutdown);
        let cc = Arc::clone(&call_count);

        let handle = tokio::spawn(supervise(
            "test-shutdown",
            Arc::clone(&shutdown),
            move || {
                let count = Arc::clone(&cc);
                let sd2 = Arc::clone(&sd);
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    // Signal shutdown before panicking — supervisor must not restart.
                    sd2.store(true, Ordering::Relaxed);
                    panic!("deliberate test panic");
                }
            },
        ));

        handle.await.expect("supervisor should complete cleanly");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "factory should be called exactly once when shutdown is set"
        );
    }

    // ---------------------------------------------------------------------------
    // Liveness probe tests
    // ---------------------------------------------------------------------------

    struct TestHome {
        _tmp: tempfile::TempDir,
        _lock: crate::TestHomeGuard,
        saved: Option<String>,
    }

    impl TestHome {
        fn new() -> Self {
            let lock = crate::test_home_guard();
            let saved = std::env::var("HOME").ok();
            let tmp = tempfile::tempdir().unwrap();
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                _lock: lock,
                saved,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match &self.saved {
                Some(v) => unsafe {
                    std::env::set_var("HOME", v);
                },
                None => unsafe {
                    std::env::remove_var("HOME");
                },
            }
        }
    }

    #[tokio::test]
    async fn liveness_is_not_running_when_socket_absent() {
        let _home = TestHome::new();
        let liveness = daemon_liveness().await;
        assert_eq!(liveness, DaemonLiveness::NotRunning);
    }

    #[tokio::test(start_paused = true)]
    async fn liveness_is_unresponsive_when_peer_never_replies() {
        let _home = TestHome::new();
        let socket_path = crate::config::default_socket_path();
        std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

        let probe = tokio::spawn(async { daemon_liveness().await });

        // Accept the connection and keep the stream open without writing
        // anything — the probe times out waiting for a response.
        let (stream, _) = listener.accept().await.unwrap();
        // Keep the stream alive for the full duration of the probe's 2s timeout.
        tokio::time::sleep(Duration::from_secs(3)).await;
        drop(stream);

        let liveness = probe.await.unwrap();
        assert_eq!(liveness, DaemonLiveness::Unresponsive);
    }

    #[tokio::test]
    async fn liveness_is_not_running_when_peer_closes_immediately() {
        let _home = TestHome::new();
        let socket_path = crate::config::default_socket_path();
        std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

        let probe = tokio::spawn(async { daemon_liveness().await });

        // Read the probe's Ping first so its write_all succeeds, THEN close —
        // otherwise the close races the write and the probe fails on the write
        // instead of reading EOF, leaving the Ok(Ok(0)) arm uncovered.
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 256];
        let _ = stream.read(&mut buf).await;
        drop(stream);

        let liveness = probe.await.unwrap();
        assert_eq!(liveness, DaemonLiveness::NotRunning);
    }

    #[tokio::test]
    async fn liveness_is_running_when_peer_answers_ok() {
        let _home = TestHome::new();
        let socket_path = crate::config::default_socket_path();
        std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

        let probe = tokio::spawn(async { daemon_liveness().await });

        let (mut stream, _) = listener.accept().await.unwrap();
        let reply = format!("{}\n", serde_json::to_string(&Response::Ok).unwrap());
        stream.writable().await.unwrap();
        use tokio::io::AsyncWriteExt;
        stream.write_all(reply.as_bytes()).await.unwrap();

        let liveness = probe.await.unwrap();
        assert_eq!(liveness, DaemonLiveness::Running);
    }

    #[tokio::test]
    async fn liveness_is_confused_on_unexpected_reply() {
        let _home = TestHome::new();
        let socket_path = crate::config::default_socket_path();
        std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

        let probe = tokio::spawn(async { daemon_liveness().await });

        let (mut stream, _) = listener.accept().await.unwrap();
        let reply = format!(
            "{}\n",
            serde_json::to_string(&Response::Error("test".into())).unwrap()
        );
        stream.writable().await.unwrap();
        use tokio::io::AsyncWriteExt;
        stream.write_all(reply.as_bytes()).await.unwrap();

        let liveness = probe.await.unwrap();
        assert_eq!(liveness, DaemonLiveness::Confused);
    }
}
