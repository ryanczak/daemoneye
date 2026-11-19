//! Daemon startup: `run_daemon` and the tmux-session choice it makes.

#[path = "startup_tasks.rs"]
mod tasks;

use super::server::handle_client;
use super::session::SessionStore;
use super::utils::log_event;
use super::{DAEMON_START, DaemonState, detect_session, instance, ready};
use crate::config::{Config, default_socket_path};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::net::UnixListener;

use tasks::{
    spawn_cache_poller_task, spawn_program_notices_task, spawn_scheduler_task,
    spawn_session_cleanup_task, spawn_session_counters_task, spawn_webhook_task,
};

/// The startup host warnings (M37 phase-06): each `[hosts.<name>]` problem
/// the config validator finds, then each entry whose `ssh` names the daemon's
/// own machine — that entry's `host:` is refused by the host gate.
fn warn_host_problems(startup_config: &crate::config::Config) {
    for warn in startup_config.validate_hosts() {
        log::warn!("{warn}");
    }
    for warn in startup_config.self_host_problems(&crate::daemon::utils::daemon_hostname()) {
        log::warn!("{warn}");
    }
}

/// Reconcile the schedule store right after it loads: jobs a daemon restart
/// left `Running` must not stay `Running` forever.
fn recover_schedules(store: &ScheduleStore) {
    let recovered = store.recover_interrupted();
    if !recovered.is_empty() {
        log::info!(
            "Scheduler startup recovery: {} job(s) reconciled",
            recovered.len()
        );
    }
}

/// Remove pipe logs and stop sentinels left behind by a previous daemon run.
fn sweep_stale_var_run_artifacts() {
    if let Ok(entries) = std::fs::read_dir(crate::config::pipe_log_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("de-pipe-") && name_str.ends_with(".log") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(crate::config::var_run_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if crate::daemon::run::is_stop_sentinel_name(&name.to_string_lossy()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Load the daemon's config at startup. A file that fails to parse runs on
/// the defaults (and the ghost foreground gate fails closed), except a file
/// that still sets a key removed in 2.10.0.
fn load_startup_config() -> Result<Config> {
    match Config::load() {
        Ok(cfg) => {
            crate::config::set_startup_error(Ok(()));
            Ok(cfg)
        }
        Err(e)
            if e.downcast_ref::<crate::config::RemovedKeysError>()
                .is_some() =>
        {
            log::error!("{e:#}");
            Err(e)
        }
        Err(e) => {
            log::error!("Failed to load config, using defaults: {e:#}");
            crate::config::set_startup_error(Err(anyhow::anyhow!("{e:#}")));
            Ok(Config::default())
        }
    }
}

fn init_logging_and_stdio(log_file: &Option<PathBuf>) -> Result<()> {
    if let Err(e) =
        env_logger::Builder::from_env(env_logger::Env::new().filter_or("DAEMONEYE_LOG", "info"))
            .write_style(env_logger::WriteStyle::Never)
            .format(|buf, record| {
                use std::io::Write;
                let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
                writeln!(buf, "{} {:5} {}", ts, record.level(), record.args())
            })
            .try_init()
    {
        eprintln!(
            "daemoneye: logger already initialised: {e} — continuing with the existing logger"
        );
    }

    if let Some(path) = log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("Failed to open log file {}", path.display()))?;
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        unsafe {
            if libc::dup2(fd, 1) < 0 {
                return Err(std::io::Error::last_os_error()).context(format!(
                    "dup2(log_fd → stdout) failed for {}",
                    path.display()
                ));
            }
            if libc::dup2(fd, 2) < 0 {
                return Err(std::io::Error::last_os_error()).context(format!(
                    "dup2(log_fd → stderr) failed for {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_and_init_config(startup_config: &Config) -> Result<()> {
    startup_config.limits.validate();
    startup_config.sandbox.validate();
    startup_config.execution.validate();
    startup_config.chat.validate();
    warn_host_problems(startup_config);

    for warn in crate::daemon::utils::retention_warnings(startup_config) {
        log::warn!(
            "retention off for {}: {} — {}",
            warn.artifact_class,
            warn.config_key,
            warn.suggestion,
        );
    }

    crate::ai::filter::init_masking(&startup_config.masking.extra_patterns);
    crate::ai::init_stream_timeouts(&startup_config.ai);

    if let Err(e) = crate::memory::migrate_namespace() {
        log::warn!("Memory namespace migration failed: {e}");
    }

    sweep_stale_var_run_artifacts();

    let default_model = startup_config.resolve_model(None);
    let api_key = default_model.resolve_api_key();
    if api_key.is_empty() {
        let env_var = default_model.api_key_env_var();
        anyhow::bail!(
            "No API key found for provider '{provider}'.\n\
             Set 'api_key' in [models.default] in ~/.daemoneye/etc/config.toml  or  export {env_var}=<your-key>",
            provider = default_model.provider,
            env_var = env_var,
        );
    }
    match default_model.provider.as_str() {
        "anthropic" if !api_key.starts_with("sk-ant-") => {
            log::warn!(
                "API key for provider 'anthropic' should start with 'sk-ant-'. \
                 The configured key may be invalid — check your config."
            );
        }
        "openai" if !api_key.starts_with("sk-") => {
            log::warn!(
                "API key for provider 'openai' should start with 'sk-'. \
                 The configured key may be invalid — check your config."
            );
        }
        _ => {}
    }
    log::info!(
        "Provider: {} / {}",
        default_model.provider,
        default_model.model
    );
    Ok(())
}

async fn resolve_initial_tmux_session(
    startup_config: &Config,
    session_override: Option<&str>,
) -> Option<String> {
    let inside_session = crate::process::off_runtime("detect-session", detect_session)
        .await
        .flatten();
    let configured: String = session_override
        .map(str::to_string)
        .unwrap_or_else(|| startup_config.daemon.tmux_session.clone());
    let exists = if inside_session.is_none() && tmux_wanted(session_override, &configured) {
        let n = configured.clone();
        crate::process::off_runtime("session-exists", move || crate::tmux::session_exists(&n))
            .await
            .unwrap_or(false)
    } else {
        false
    };
    let initial_session = startup_session(inside_session, &configured, exists);
    if initial_session.is_none() {
        log::info!("No tmux session to adopt; the daemon starts tmux-free.");
    }
    initial_session
}

fn adopt_shells_and_sandboxes(sandbox_enabled: bool) -> crate::shell::ShellRegistry {
    let swept = crate::shell::sweep_stale(&crate::config::shells_dir());
    if !swept.is_empty() {
        let ids: Vec<&str> = swept.iter().map(|id| id.as_str()).collect();
        log::info!(
            "shell registry: swept {} dead host(s): {ids:?}",
            swept.len()
        );
    }
    let reg = crate::shell::registry().clone();
    let adopted = crate::shell::adopt_at_startup(
        &reg,
        &crate::config::shells_dir(),
        &crate::config::shell_logs_dir(),
    );
    if !adopted.is_empty() {
        let ids: Vec<&str> = adopted.iter().map(|id| id.as_str()).collect();
        log::info!(
            "shell registry: adopted {} surviving shell(s): {ids:?}",
            adopted.len()
        );
    }
    if sandbox_enabled {
        let live: Vec<String> = crate::shell::with_shells(&reg, |store| {
            store.values().filter_map(|r| r.sandbox.clone()).collect()
        });
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        tokio::spawn(async move {
            let report = crate::daemon::executor::sbx_lifecycle::sweep_daemon_sandboxes(
                &crate::shell::real_runner(program),
                &live,
            )
            .await;
            crate::daemon::utils::log_event(
                "sbx_startup_sweep",
                serde_json::json!({
                    "removed": report.removed,
                    "stopped": report.stopped,
                    "failed": report.failed,
                    "list_error": report.list_error,
                }),
            );
        });
    }
    reg
}

async fn seed_initial_viewport(cache: &SessionCache, initial_session: &Option<String>) {
    if let Some(sn) = initial_session {
        let s = sn.to_string();
        let (w, h) = crate::process::off_runtime("client-dimensions", move || {
            crate::tmux::client_dimensions(&s)
        })
        .await
        .unwrap_or((0, 0));
        if w > 0 && h > 0 {
            cache.set_client_size(w, h);
            log::info!(
                "N7: initial client viewport {}x{} for session '{}'",
                w,
                h,
                sn
            );
        }
    }
}

async fn bind_and_serve_ipc(state: Arc<DaemonState>, shutdown: Arc<AtomicBool>) -> Result<()> {
    let socket_path: PathBuf = default_socket_path();

    match socket_path.symlink_metadata() {
        Ok(_) => {
            std::fs::remove_file(&socket_path).context("Failed to remove stale socket file")?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Failed to stat socket path"),
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind to socket at {}", socket_path.display()))?;

    let socket_id = socket_path.symlink_metadata().ok().map(|m| {
        (
            std::os::unix::fs::MetadataExt::dev(&m),
            std::os::unix::fs::MetadataExt::ino(&m),
        )
    });

    log::info!("Daemon listening on {}", socket_path.display());
    ready::report_ready();

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Failed to install SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Failed to install SIGINT handler")?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let state_conn = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, state_conn).await {
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
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
            _ = sigint.recv() => {
                log::info!("Received SIGINT, shutting down.");
                log_event("daemon_stop", serde_json::json!({ "reason": "SIGINT" }));
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
        }
    }

    let current_id = socket_path.symlink_metadata().ok().map(|m| {
        (
            std::os::unix::fs::MetadataExt::dev(&m),
            std::os::unix::fs::MetadataExt::ino(&m),
        )
    });
    if socket_id.is_some() && current_id == socket_id {
        let _ = std::fs::remove_file(&socket_path);
    } else {
        log::warn!(
            "socket at {} is not the one this daemon bound — leaving it in place",
            socket_path.display()
        );
    }

    log::info!("Daemon stopped cleanly.");
    Ok(())
}

/// Start the daemon process.
///
/// Lifecycle:
/// 1. Redirect stdout/stderr to `log_file` (if provided).
/// 2. Validate the configured AI API key; bail immediately if absent.
/// 3. Detect or adopt an existing tmux session to monitor.
/// 4. Spawn background supervisor tasks (cache, scheduler, webhook, counters, cleanup).
/// 5. Bind the Unix domain socket and enter the accept loop until SIGTERM/SIGINT.
pub async fn run_daemon(log_file: Option<PathBuf>, session_override: Option<String>) -> Result<()> {
    let started_at = *DAEMON_START.get_or_init(Instant::now);
    init_logging_and_stdio(&log_file)?;

    let _instance = match instance::InstanceLock::acquire(&crate::config::default_pid_path()) {
        Ok(lock) => lock,
        Err(e) => {
            log::error!("{e}");
            anyhow::bail!("{e}");
        }
    };

    log::info!(
        "daemoneye {} starting — PID {}, exe {}, log {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string()),
        log_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<stdout>".to_string()),
    );

    let startup_config = load_startup_config()?;
    let config_handle = Arc::new(crate::config::ConfigHandle::new(startup_config.clone()));
    crate::config::install(Arc::clone(&config_handle));
    validate_and_init_config(&startup_config)?;

    let initial_session =
        resolve_initial_tmux_session(&startup_config, session_override.as_deref()).await;
    log_event(
        "daemon_start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "session": initial_session.as_deref().unwrap_or(""),
            "socket":  default_socket_path().display().to_string(),
        }),
    );

    let cache = Arc::new(SessionCache::new(initial_session.as_deref().unwrap_or("")));
    let shells = adopt_shells_and_sandboxes(startup_config.sandbox.enabled);
    seed_initial_viewport(&cache, &initial_session).await;

    let shutdown: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    spawn_cache_poller_task(Arc::clone(&shutdown), Arc::clone(&cache));

    let sessions: SessionStore = SessionStore::new();
    crate::daemon::notice_sinks::replay_alert_registry(crate::webhook::alert_registry());
    crate::daemon::notice_sinks::register_default_sinks(
        crate::bus::global(),
        sessions.clone(),
        crate::daemon::notice_sinks::default_routes(crate::bus::Severity::Warning),
    );

    let schedule_store = Arc::new(
        ScheduleStore::load_or_create(Config::schedules_path()).unwrap_or_else(|e| {
            log::error!("Could not load schedules from primary path: {e} — schedules will not persist this session");
            ScheduleStore::new_empty()
        }),
    );
    recover_schedules(&schedule_store);

    let state = DaemonState::new(
        config_handle,
        sessions,
        crate::daemon::session::global_presence().clone(),
        shells,
        schedule_store,
        cache,
        crate::bus::global().clone(),
        crate::webhook::alert_registry().clone(),
        crate::daemon::stats::global().clone(),
        started_at,
    );

    spawn_scheduler_task(
        Arc::clone(&shutdown),
        Arc::clone(&state),
        initial_session.unwrap_or_default(),
    );
    spawn_webhook_task(Arc::clone(&shutdown), Arc::clone(&state), &startup_config).await?;
    spawn_session_counters_task(Arc::clone(&shutdown), Arc::clone(&state));
    spawn_program_notices_task(Arc::clone(&shutdown));
    spawn_session_cleanup_task(Arc::clone(&shutdown), Arc::clone(&state), log_file);

    bind_and_serve_ipc(state, shutdown).await
}

/// Whether the daemon should use tmux when launched outside it: an empty
/// (or all-whitespace) session name, from `--session` or `[daemon]
/// tmux_session`, means no tmux at all.
pub(super) fn tmux_wanted(session_override: Option<&str>, configured: &str) -> bool {
    !session_override.unwrap_or(configured).trim().is_empty()
}

/// Resolve the session the daemon adopts at startup — never creates one.
fn startup_session(inside: Option<String>, configured: &str, exists: bool) -> Option<String> {
    if let Some(name) = inside {
        return Some(name);
    }
    if !configured.trim().is_empty() && exists {
        return Some(configured.trim().to_string());
    }
    None
}

#[cfg(test)]
mod startup_session_tests {
    use super::startup_session;

    #[test]
    fn startup_session_never_creates() {
        assert_eq!(
            startup_session(Some("dev".to_string()), "other", false),
            Some("dev".to_string()),
            "inside tmux must adopt that session"
        );
        assert_eq!(
            startup_session(None, "work", true),
            Some("work".to_string()),
            "a configured name that exists must be adopted"
        );
        assert_eq!(
            startup_session(None, "work", false),
            None,
            "a configured name that does not exist must not be created"
        );
        assert_eq!(
            startup_session(None, "", true),
            None,
            "an empty configured name means no tmux"
        );
        assert_eq!(
            startup_session(None, "   ", true),
            None,
            "a whitespace-only configured name means no tmux"
        );
    }
}
