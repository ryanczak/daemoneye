//! Daemon startup: `run_daemon` and the tmux-session choice it makes.

use super::scheduled::run_scheduled_job;
use super::server::handle_client;
use super::session::SessionStore;
use super::utils::log_event;
use super::{DAEMON_START, detect_session, instance, ready, scheduled, supervise};
use crate::config::{Config, default_socket_path};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;
use tokio::net::UnixListener;

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
pub async fn run_daemon(log_file: Option<PathBuf>, session_override: Option<String>) -> Result<()> {
    // Record daemon start time for uptime reporting (F1).
    DAEMON_START.get_or_init(Instant::now);

    // Initialise env_logger once.  DAEMONEYE_LOG=debug|info|warn|error controls verbosity.
    // Default is `info` which shows lifecycle events, connections, and command execution.
    // Color is disabled and a human-readable UTC timestamp is prepended to every line.
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
    // Acquire the instance lock before any startup side effect. The lock is
    // held for the lifetime of the daemon — the kernel releases it on death.
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

    // Validate API key before binding the socket so the error is immediate
    // and obvious rather than surfacing as a cryptic 401 mid-conversation.
    let startup_config = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            log::error!("Failed to load config, using defaults: {e}");
            Config::default()
        }
    };

    // Warn about any limit configuration that is likely unintentional.
    startup_config.limits.validate();

    // Warn about sandbox configuration that would silently defeat the sandbox.
    startup_config.sandbox.validate();

    startup_config.execution.validate();

    // Warn about host entries that will not resolve; a typo'd host must not
    // stop the daemon.
    for warn in startup_config.validate_hosts() {
        log::warn!("{warn}");
    }

    // Warn about retention settings that are off by default (keep forever).
    for warn in crate::daemon::utils::retention_warnings(&startup_config) {
        log::warn!(
            "retention off for {}: {} — {}",
            warn.artifact_class,
            warn.config_key,
            warn.suggestion,
        );
    }

    // Initialise the masking filter with built-in patterns + any user-defined extras.
    crate::ai::filter::init_masking(&startup_config.masking.extra_patterns);

    // Initialise the two-phase stream-timeout budgets from `[ai]` config.
    crate::ai::init_stream_timeouts(&startup_config.ai);

    // G2: migrate any legacy memory files to include namespace frontmatter.
    if let Err(e) = crate::memory::migrate_namespace() {
        log::warn!("Memory namespace migration failed: {e}");
    }

    // R1: clean up any pipe log files left behind by a previous daemon run so
    // stale content from a different session is never shown to the AI.
    if let Ok(entries) = std::fs::read_dir(crate::config::pipe_log_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("de-pipe-") && name_str.ends_with(".log") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    // Remove stop sentinels left behind by a previous daemon run: a
    // `daemoneye stop <run>` from a terminal whose daemon died would
    // otherwise still be honoured on restart. Only `stop-*` entries are
    // removed — everything else in var/run (pid, socket, schedules,
    // pane prefs, the shells subtree) must survive.
    if let Ok(entries) = std::fs::read_dir(crate::config::var_run_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if crate::daemon::run::is_stop_sentinel_name(&name.to_string_lossy()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

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
    // Warn early if the key format looks wrong for known providers, so the
    // user sees a clear message at startup rather than a cryptic 401 mid-chat.
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

    // Determine the initial tmux session, read-only.
    //
    // The daemon never creates a tmux session. It adopts one when it can:
    //   1. If launched inside tmux ($TMUX is set), adopt the current session.
    //   2. Otherwise, use the configured name (CLI override > config.daemon
    //      .tmux_session) when such a session already exists.
    // Anything else means no tmux at all: no session, no cache poll. Never fatal.
    let inside_session = crate::process::off_runtime("detect-session", detect_session)
        .await
        .flatten();
    let configured: String = session_override
        .clone()
        .unwrap_or_else(|| startup_config.daemon.tmux_session.clone());
    let exists =
        if inside_session.is_none() && tmux_wanted(session_override.as_deref(), &configured) {
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

    log_event(
        "daemon_start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "session": initial_session.as_deref().unwrap_or(""),
            "socket":  default_socket_path().display().to_string(),
        }),
    );

    let cache = Arc::new(SessionCache::new(initial_session.as_deref().unwrap_or("")));
    // The live shell registry: clear dead hosts' leftovers out of the shells
    // directory, then re-register the shells that survived a daemon restart.
    // The PTY substrate is the only substrate, so this always runs.
    let _shells = {
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
        // 06c: a daemon that died with `sbx` sandboxes live sweeps the ones
        // no surviving shell record owns, off the startup path.
        if startup_config.sandbox.enabled {
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
        Some(reg)
    };

    // N7: seed the initial client viewport dimensions now that the cache exists.
    if let Some(ref sn) = initial_session {
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

    // Shutdown flag shared with all supervisor tasks so they know not to restart
    // after the accept loop exits on SIGTERM/SIGINT (A1).
    let shutdown: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    log::info!("Cache poller started");
    let cache_sup = Arc::clone(&cache);
    tokio::spawn(supervise(
        "cache-poller",
        Arc::clone(&shutdown),
        move || {
            let c = Arc::clone(&cache_sup);
            async move {
                loop {
                    if let Err(e) = c.refresh() {
                        log::warn!("Failed to refresh tmux cache: {}", e);
                        log_event(
                            "cache_refresh_error",
                            serde_json::json!({ "error": e.to_string() }),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        },
    ));

    let sessions: SessionStore = SessionStore::new();

    // Load or create the schedule store.
    let schedules_path = Config::schedules_path();
    let schedule_store = Arc::new(
        ScheduleStore::load_or_create(schedules_path).unwrap_or_else(|e| {
            log::error!("Could not load schedules from primary path: {e} — schedules will not persist this session");
            ScheduleStore::new_empty()
        }),
    );

    // Scheduler task: poll every second for due jobs.
    {
        let store_sup = Arc::clone(&schedule_store);
        let cfg_sup = startup_config.clone();
        let sessions_sup = sessions.clone();
        let cache_sup = Arc::clone(&cache);
        let sn_sup = initial_session.clone().unwrap_or_default();
        tokio::spawn(supervise("scheduler", Arc::clone(&shutdown), move || {
            let store = Arc::clone(&store_sup);
            let cfg = cfg_sup.clone();
            let sessions_sched = sessions_sup.clone();
            let cache_sched = Arc::clone(&cache_sup);
            let sn = sn_sup.clone();
            async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tick.tick().await;
                    let due = scheduled::due_jobs_for_tick(&store, &sn);
                    for job in due {
                        let store2 = Arc::clone(&store);
                        let sn2 = sn.clone();
                        let cfg2 = cfg.clone();
                        let sessions2 = sessions_sched.clone();
                        let cache2 = Arc::clone(&cache_sched);
                        tokio::spawn(async move {
                            // The refusal is already recorded on the run's
                            // log; a tick-level caller has nothing to do
                            // with it.
                            let _ =
                                run_scheduled_job(job, store2, sn2, sessions2, cfg2, cache2, None)
                                    .await;
                        });
                    }
                }
            }
        }));
    }

    // Optional webhook ingestion endpoint.
    if startup_config.webhook.enabled {
        let listener = crate::webhook::bind(&startup_config).await?;
        if startup_config.webhook.secret.is_empty() {
            log::warn!(
                "Webhook listener enabled on port {} — no auth (set webhook.secret in config.toml to require a Bearer token)",
                startup_config.webhook.port
            );
        } else {
            log::info!(
                "Webhook listener enabled on port {} — Bearer token auth required",
                startup_config.webhook.port
            );
        }
        let wh_config_sup = startup_config.clone();
        let wh_sessions_sup = sessions.clone();
        let wh_cache_sup = Arc::clone(&cache);
        let wh_schedule_store_sup = Arc::clone(&schedule_store);
        let listener = Arc::new(tokio::sync::Mutex::new(Some(listener)));
        tokio::spawn(supervise("webhook", Arc::clone(&shutdown), move || {
            let cfg = wh_config_sup.clone();
            let sessions = wh_sessions_sup.clone();
            let cache = Arc::clone(&wh_cache_sup);
            let schedule_store = Arc::clone(&wh_schedule_store_sup);
            let listener = Arc::clone(&listener);
            async move {
                let listener = {
                    let mut guard = listener.lock().await;
                    guard.take()
                };
                match listener {
                    Some(l) => {
                        if let Err(e) =
                            crate::webhook::serve(l, cfg, sessions, cache, schedule_store).await
                        {
                            log::error!("Webhook server exited: {}", e);
                        }
                    }
                    None => {
                        log::error!("webhook listener was consumed; not restarting");
                    }
                }
            }
        }));
    }

    // Prune chat sessions idle for more than 30 minutes.
    let sessions_cleanup_sup = sessions.clone();
    let log_path_sup = log_file.clone();
    tokio::spawn(supervise(
        "session-cleanup",
        Arc::clone(&shutdown),
        move || {
            let sessions_cleanup = sessions_cleanup_sup.clone();
            let log_path = log_path_sup.clone();
            async move {
                let mut sweep_counter = 0u32;
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;

                    // Locked phase: evict and snapshot. The guard is released
                    // when `cleanup_pass` returns.
                    let (_evicted, active_ids) = crate::daemon::session::cleanup_pass(
                        &sessions_cleanup,
                        Instant::now(),
                        Duration::from_secs(1800),
                    );

                    // Unlocked phase: everything blocking happens out here.
                    sweep_counter = sweep_counter.wrapping_add(1);
                    if sweep_counter.is_multiple_of(60) {
                        crate::daemon::utils::sweep_event_segments(
                            startup_config.events.retention_days,
                        );
                        crate::daemon::utils::sweep_session_archives(
                            startup_config.sessions.archive_retention_days,
                            &active_ids,
                        );
                        crate::daemon::utils::sweep_session_logs(
                            startup_config.sessions.log_retention_days,
                            &active_ids,
                        );
                        crate::daemon::utils::sweep_pane_logs(
                            startup_config.retention.pane_log_retention_days,
                        );
                        crate::daemon::utils::sweep_agent_mailboxes(
                            startup_config.retention.mailbox_retention_days,
                        );
                        // Reap the record first so a just-reaped shell's log
                        // is swept on the same tick rather than the next one.
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        crate::shell::reap_exited(
                            crate::shell::registry(),
                            startup_config.shells.exited_retention_secs,
                            now,
                        );
                        let program =
                            crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
                        for reaped in
                            crate::daemon::executor::chat_sandbox::reap_idle_chat_sandboxes(
                                &crate::shell::real_runner(program),
                                crate::shell::registry(),
                                now,
                            )
                            .await
                        {
                            // 08b: the log is read before `rm` (reap_idle_in
                            // orders it), because it survives `rm` and
                            // accumulates across sandboxes that reuse a name.
                            if let Some(audit) = reaped.egress {
                                crate::daemon::utils::log_event(
                                    "sandbox_egress",
                                    crate::daemon::executor::sbx::egress_event(
                                        &reaped.name,
                                        &audit,
                                    ),
                                );
                            }
                            crate::daemon::utils::log_event(
                                "chat_sandbox_teardown",
                                serde_json::json!({ "sandbox": reaped.name, "outcome": format!("{:?}", reaped.teardown) }),
                            );
                        }
                        let mut live_shell_ids = std::collections::BTreeSet::new();
                        crate::shell::with_shells(crate::shell::registry(), |store| {
                            live_shell_ids.extend(store.keys().cloned());
                        });
                        crate::daemon::utils::sweep_shell_logs(
                            startup_config.shells.log_retention_days,
                            &live_shell_ids,
                        );
                        if let Some(ref lp) = log_path
                            && crate::daemon::utils::rotate_log_file(
                                lp,
                                startup_config.logging.log_max_bytes,
                                startup_config.logging.log_keep_count,
                            )
                        {
                            crate::daemon::utils::reattach_log_fds(lp);
                        }
                    }
                }
            }
        },
    ));

    let socket_path: PathBuf = default_socket_path();

    // The instance lock is held, so no other daemon is alive: any socket file at
    // this path is definitionally stale and safe to remove. symlink_metadata()
    // (does not follow symlinks) so a symlink at the socket path removes the
    // symlink itself rather than its target (S3).
    match socket_path.symlink_metadata() {
        Ok(_) => {
            std::fs::remove_file(&socket_path).context("Failed to remove stale socket file")?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Failed to stat socket path"),
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind to socket at {}", socket_path.display()))?;

    // Record the bound socket's identity for identity-checked teardown below.
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
                        let cache_conn = Arc::clone(&cache);
                        let sessions_conn = sessions.clone();
                        let sched_conn = Arc::clone(&schedule_store);
                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_client(stream, cache_conn, sessions_conn, sched_conn).await
                            {
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

    // ── Graceful shutdown ────────────────────────────────────────────────────
    // 1. Remove the socket so new clients get a clean "not running" error.
    // Only unlink the socket this daemon bound. If the identity differs, another
    // process replaced the path and removing it would strip a successor's address.
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

/// Whether the daemon should use tmux when launched outside it: an empty
/// (or all-whitespace) session name, from `--session` or `[daemon]
/// tmux_session`, means no tmux at all.
pub(super) fn tmux_wanted(session_override: Option<&str>, configured: &str) -> bool {
    !session_override.unwrap_or(configured).trim().is_empty()
}

/// Resolve the session the daemon adopts at startup — never creates one.
///
/// Launched inside tmux: adopt that session. Otherwise a non-empty
/// configured `[daemon] tmux_session` that exists is adopted. Anything else
/// is `None`, and the caller logs a single `info` line. Never fatal.
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
            "an empty configured name must yield no session"
        );
    }
}
