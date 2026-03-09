
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use std::time::Duration;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};
use crate::tmux::cache::SessionCache;
use crate::config::Config;
use crate::scheduler::ScheduleStore;


pub mod session;
pub mod utils;
pub mod server;
pub mod executor;
pub mod background;

/// Shared prefix for all daemon-managed tmux windows.  Used by the CLI to
/// filter windows from `tmux list-windows` output.
pub const DAEMON_WINDOW_PREFIX: &str = "de-";
/// Window-name prefix for background execution windows (`de-bg-<session>-<ts>-<id>`).
pub const BG_WINDOW_PREFIX: &str = "de-bg-";
/// Window-name prefix for scheduled-job windows (`de-sched-<ts>-<id>`).
pub const SCHED_WINDOW_PREFIX: &str = "de-sched-";

pub use session::*;
pub use utils::*;
pub use server::*;

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
    let out = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Install the per-session tmux `alert-bell` hook so the daemon is notified
/// when background panes ring the bell.  The global `pane-died` hook must be
/// installed separately (see `run_daemon`).
pub fn install_session_hooks(session_name: &str, hook_exe: &str) {
    let notify_cmd = format!(
        "run-shell -b '{} notify activity #{{pane_id}} 0 \"{}\"'",
        hook_exe,
        crate::daemon::utils::shell_escape_arg(session_name),
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args(["set-hook", "-t", session_name, "alert-bell", &notify_cmd])
        .output()
    {
        log::warn!("Failed to register alert-bell hook for '{}': {}", session_name, e);
    } else {
        log::info!("Session hooks installed for: {}", session_name);
    }
}

/// Returns true if a daemon is already listening and responding on the socket.
/// Uses a 2-second timeout so a hung process doesn't block startup.
pub async fn daemon_is_running() -> bool {
    let Ok(stream) = tokio::net::UnixStream::connect(DEFAULT_SOCKET_PATH).await else {
        return false;
    };
    let (rx_half, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx_half);

    let Ok(mut data) = serde_json::to_vec(&Request::Ping) else {
        return false;
    };
    data.push(b'\n');
    if tx.write_all(&data).await.is_err() {
        return false;
    }

    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(2), rx.read_line(&mut line)).await {
        Ok(Ok(_)) => matches!(serde_json::from_str::<Response>(line.trim()), Ok(Response::Ok)),
        _ => false,
    }
}

/// Start the daemon process.
///
/// Lifecycle:
/// 1. Redirect stdout/stderr to `log_file` (if provided).
/// 2. Validate the configured AI API key; bail immediately if absent.
/// 3. Detect or create a tmux session to monitor.
/// 4. Spawn the pane-cache refresh loop (every 2 s).
/// 5. Bind the Unix domain socket and enter the accept loop.
/// 6. Optionally open the chat pane if the daemon just created the tmux session.
/// 7. Shut down cleanly on SIGTERM or SIGINT.
pub async fn run_daemon(log_file: Option<PathBuf>) -> Result<()> {
    // Initialise env_logger once.  DAEMONEYE_LOG=debug|info|warn|error controls verbosity.
    // Default is `info` which shows lifecycle events, connections, and command execution.
    // Color is disabled and a human-readable UTC timestamp is prepended to every line.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::new().filter_or("DAEMONEYE_LOG", "info")
    )
    .write_style(env_logger::WriteStyle::Never)
    .format(|buf, record| {
        use std::io::Write;
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        writeln!(buf, "{} {:5} {}", ts, record.level(), record.args())
    })
    .try_init();

    if let Some(ref path) = log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("Failed to open log file {}", path.display()))?;
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // Redirect stdout (1) and stderr (2) to the log file.
        // dup2 creates independent FDs 1/2 pointing to the file; `file` can drop safely after.
        unsafe {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
        }
    }
    // Validate API key before binding the socket so the error is immediate
    // and obvious rather than surfacing as a cryptic 401 mid-conversation.
    let startup_config = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            log::error!("Failed to load config, using defaults: {e}");
            Config::default()
        }
    };

    // Initialise the masking filter with built-in patterns + any user-defined extras.
    crate::ai::filter::init_masking(&startup_config.masking.extra_patterns);

    if startup_config.ai.resolve_api_key().is_empty() {
        let env_var = startup_config.ai.api_key_env_var();
        anyhow::bail!(
            "No API key found for provider '{provider}'.\n\
             Set 'api_key' in ~/.daemoneye/config.toml  or  export {env_var}=<your-key>",
            provider = startup_config.ai.provider,
            env_var = env_var,
        );
    }
    log::info!("Provider: {} / {}", startup_config.ai.provider, startup_config.ai.model);

    let initial_session = detect_session();
    match &initial_session {
        Some(s) => log::info!("Attaching to existing tmux session: {}", s),
        None => log::warn!(
            "No tmux session detected at startup. \
             DaemonEye will begin monitoring once `daemoneye chat` is run."
        ),
    }

    log_event("daemon_start", serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "session": initial_session.as_deref().unwrap_or(""),
        "pid":     std::process::id(),
        "socket":  DEFAULT_SOCKET_PATH,
    }));

    let hook_exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    // pane-died is a global hook — install it regardless of whether a session
    // is known yet, so it fires as soon as the user's session appears.
    let global_notify_cmd = format!(
        "run-shell -b '{} notify activity #{{pane_id}} 0 #{{session_name}}'",
        hook_exe_path,
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args(["set-hook", "-g", "pane-died", &global_notify_cmd])
        .output()
    {
        log::error!("Failed to register global tmux pane-died hook: {}", e);
    }

    // Install per-session alert-bell hook if we already know the session.
    if let Some(ref sn) = initial_session {
        install_session_hooks(sn, &hook_exe_path);
    }

    // bg_session is the tmux session used for background/scheduled job windows.
    // Starts empty when started by systemd; adopted from the first connecting client.
    let bg_session: Arc<Mutex<String>> = Arc::new(Mutex::new(
        initial_session.clone().unwrap_or_default()
    ));

    let cache = Arc::new(SessionCache::new(
        initial_session.as_deref().unwrap_or("")
    ));

    log::info!("Cache poller started");
    let cache_monitor = Arc::clone(&cache);
    tokio::spawn(async move {
        loop {
            if let Err(e) = cache_monitor.refresh() {
                log::warn!("Failed to refresh tmux cache: {}", e);
                log_event("cache_refresh_error", serde_json::json!({ "error": e.to_string() }));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));

    // Load or create the schedule store.
    let schedules_path = Config::schedules_path();
    let schedule_store = Arc::new(
        ScheduleStore::load_or_create(schedules_path)
            .unwrap_or_else(|e| {
                log::warn!("Could not load schedules: {e}");
                ScheduleStore::load_or_create(
                    std::env::temp_dir().join("daemoneye_schedules.json")
                ).expect("fallback schedule store")
            })
    );

    // Scheduler task: poll every second for due jobs.
    {
        let store = Arc::clone(&schedule_store);
        let bg_sn = Arc::clone(&bg_session);
        let cfg = startup_config.clone();
        let sessions_sched = Arc::clone(&sessions);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let sn = bg_sn.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if sn.is_empty() {
                    continue; // No session adopted yet; skip until a client connects.
                }
                let due = store.take_due();
                for job in due {
                    let store2 = Arc::clone(&store);
                    let sn2 = sn.clone();
                    let cfg2 = cfg.clone();
                    let sessions2 = Arc::clone(&sessions_sched);
                    tokio::spawn(async move {
                        run_scheduled_job(job, store2, sn2, sessions2, cfg2, None).await;
                    });
                }
            }
        });
    }

    // Optional webhook ingestion endpoint.
    if startup_config.webhook.enabled {
        let wh_config = startup_config.clone();
        let wh_sessions = Arc::clone(&sessions);
        tokio::spawn(async move {
            if let Err(e) = crate::webhook::start(wh_config, wh_sessions).await {
                log::error!("Webhook server exited: {}", e);
            }
        });
        if startup_config.webhook.secret.is_empty() {
            log::warn!("Webhook listener enabled on port {} — no auth (set webhook.secret in config.toml to require a Bearer token)", startup_config.webhook.port);
        } else {
            log::info!("Webhook listener enabled on port {} — Bearer token auth required", startup_config.webhook.port);
        }
    }

    // Prune chat sessions idle for more than 30 minutes.
    let sessions_cleanup = Arc::clone(&sessions);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let now = Instant::now();
            sessions_cleanup
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, v| {
                    if now.duration_since(v.last_accessed()) >= Duration::from_secs(1800) {
                        v.cleanup_bg_windows();
                        false
                    } else {
                        true
                    }
                });
        }
    });

    if daemon_is_running().await {
        anyhow::bail!(
            "A daemon is already running on {}.\n\
             Stop it with:  daemoneye stop",
            DEFAULT_SOCKET_PATH,
        );
    }

    let socket_path = Path::new(DEFAULT_SOCKET_PATH);

    if socket_path.exists() {
        std::fs::remove_file(socket_path)
            .context("Failed to remove stale socket file")?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("Failed to bind to socket at {}", DEFAULT_SOCKET_PATH))?;

    log::info!("Daemon listening on {}", DEFAULT_SOCKET_PATH);

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Failed to install SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Failed to install SIGINT handler")?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let cache_conn = Arc::clone(&cache);
                        let sessions_conn = Arc::clone(&sessions);
                        let sched_conn = Arc::clone(&schedule_store);
                        let bg_conn = Arc::clone(&bg_session);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, cache_conn, sessions_conn, sched_conn, bg_conn).await {
                                log::error!("Error handling client: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("Failed to accept incoming connection: {}", e);
                    }
                }
            }
            _ = sigterm.recv() => {
                log::info!("Received SIGTERM, shutting down.");
                log_event("daemon_stop", serde_json::json!({ "reason": "SIGTERM" }));
                break;
            }
            _ = sigint.recv() => {
                log::info!("Received SIGINT, shutting down.");
                log_event("daemon_stop", serde_json::json!({ "reason": "SIGINT" }));
                break;
            }
        }
    }

    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

