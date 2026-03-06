
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use std::time::Duration;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::config::Config;
use crate::scheduler::ScheduleStore;


pub mod session;
pub mod utils;
pub mod server;
pub mod executor;
pub mod background;

pub use session::*;
pub use utils::*;
pub use server::*;

/// Returns `(session_name, newly_created)`.
/// If the daemon was launched from inside an existing tmux session, that
/// session is used and `newly_created` is false.
/// Otherwise the fallback "daemoneye" session is used; `newly_created` is true
/// when this call actually created it (not when it already existed).
pub fn detect_or_create_session() -> Result<(String, bool)> {
    if std::env::var("TMUX").is_ok() {
        let out = std::process::Command::new("tmux")
            .args(["display-message", "-p", "#S"])
            .output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok((s, false));
            }
        }
    }
    let already_exists = tmux::has_session(FALLBACK_SESSION);
    if !already_exists {
        tmux::create_session(FALLBACK_SESSION)?;
    }
    Ok((FALLBACK_SESSION.to_string(), !already_exists))
}

/// After the daemon socket is bound, open the AI chat pane in the newly
/// created session so the user sees it immediately on `tmux attach`.
pub async fn open_chat_pane(session_name: String) {
    // Brief pause so the accept loop is running before the chat client connects.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let pane_target = format!("{}:0.0", session_name);

    // Resolve the global pane ID (e.g. %3) of the shell pane so we can pass
    // it as DAEMONEYE_SOURCE_PANE — the pane where commands should be injected.
    let shell_pane_id = match std::process::Command::new("tmux")
        .args(["display-message", "-t", &pane_target, "-p", "#{pane_id}"])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => {
            log::warn!("Could not read shell pane ID: {e}");
            return;
        }
    };

    if shell_pane_id.is_empty() {
        log::warn!("Empty shell pane ID, skipping chat pane setup");
        return;
    }

    // Use the exact binary that is currently running so the path is always
    // correct regardless of how the daemon was invoked.
    let daemon_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    let chat_cmd = format!("{} chat", daemon_bin);

    // R7: split-window is rejected when the window is zoomed — use new-window instead.
    let zoomed = std::process::Command::new("tmux")
        .args(["display-message", "-t", &pane_target, "-p", "#{window_zoomed_flag}"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false);

    let zoomed_target = format!("{}:", session_name);
    let result = if zoomed {
        std::process::Command::new("tmux")
            .args([
                "new-window",
                "-t", &zoomed_target,
                "-e", &format!("DAEMONEYE_SOURCE_PANE={}", shell_pane_id),
                &chat_cmd,
            ])
            .output()
    } else {
        std::process::Command::new("tmux")
            .args([
                "split-window", "-h",
                "-t", &pane_target,
                "-e", &format!("DAEMONEYE_SOURCE_PANE={}", shell_pane_id),
                &chat_cmd,
            ])
            .output()
    };

    match result {
        Ok(o) if o.status.success() => {
            println!("Chat pane ready. Attach with:  tmux attach -t {}", session_name);
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            log::warn!("Could not open chat pane: {}", err.trim());
            log::info!("Attach manually with:  tmux attach -t {}  then run `daemoneye chat`", session_name);
        }
        Err(e) => {
            log::warn!("Could not open chat pane: {e}");
        }
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
    let _ = env_logger::Builder::from_env(
        env_logger::Env::new().filter_or("DAEMONEYE_LOG", "info")
    ).try_init();

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
    let startup_config = Config::load().unwrap_or_default();

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

    let (session_name, session_was_created) = detect_or_create_session()?;
    log::info!("Monitoring tmux session: {}", session_name);
    log_event("daemon_start", serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "session": session_name,
        "pid":     std::process::id(),
        "socket":  DEFAULT_SOCKET_PATH,
    }));

    // Install session-wide pane-died and alert-bell hooks to catch background job activity natively
    let hook_exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());
    
    // We use the raw #{pane_id} template formatting string so tmux expands it automatically per-pane.
    let notify_cmd = format!("run-shell -b '{} notify activity #{{pane_id}} 0 \"{}\"'", hook_exe_path, session_name);
    
    // pane-died is a global hook, so we must set it globally (-g).
    // We overwrite to prevent duplicate run-shell commands proliferating if the daemon restarts.
    if let Err(e) = std::process::Command::new("tmux")
        .args(["set-hook", "-g", "pane-died", &notify_cmd])
        .output()
    {
        log::error!("Failed to register global tmux pane-died hook: {}", e);
    }
        
    // alert-bell can be set per session (-t)
    if let Err(e) = std::process::Command::new("tmux")
        .args(["set-hook", "-t", &session_name, "alert-bell", &notify_cmd])
        .output()
    {
        log::error!("Failed to register session tmux alert-bell hook: {}", e);
    }

    let cache = Arc::new(SessionCache::new(&session_name));

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
        let sn = session_name.clone();
        let cfg = startup_config.clone();
        let sessions_sched = Arc::clone(&sessions);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
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

    // Prune chat sessions idle for more than 30 minutes.
    let sessions_cleanup = Arc::clone(&sessions);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let now = Instant::now();
            sessions_cleanup
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, v| now.duration_since(v.last_accessed()) < Duration::from_secs(1800));
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

    // If the daemon just created the tmux session, open the chat pane inside
    // it now that the socket is ready. Users can then simply
    // `tmux attach -t daemoneye` and start chatting immediately.
    if session_was_created {
        let sn = session_name.clone();
        tokio::spawn(async move { open_chat_pane(sn).await });
    }

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
                        let sn = session_name.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, cache_conn, sessions_conn, sched_conn, sn).await {
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

