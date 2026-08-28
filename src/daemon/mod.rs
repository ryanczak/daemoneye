use crate::config::{Config, default_socket_path};
use crate::ipc::{Request, Response};
use crate::scheduler::ScheduleStore;
pub use crate::tmux::cache::SessionCache;
pub use crate::util::UnpoisonExt;
use anyhow::{Context, Result};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

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
pub mod background;
pub mod briefing;
pub mod cancel;
pub mod context;
pub mod digest;
pub mod executor;
pub mod ghost;
pub mod hook;
pub mod instance;
pub mod memory_prompt;
pub mod policy;
pub mod prompt;
pub mod ready;
pub mod scheduled;
pub mod server;
pub mod session;
pub mod situational;
pub mod stats;
pub mod stream;
pub mod utils;

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

/// Install per-session tmux hooks so the daemon is notified of focus changes,
/// window switches, and bell events without waiting for the 2 s poll cycle.
///
/// Hooks installed:
/// - `alert-bell`             — background pane rang the bell (existing)
/// - `pane-focus-in`          — instant active-pane tracking (N1)
/// - `session-window-changed` — instant window-switch awareness (N2)
/// - `client-resized`         — viewport dimension updates (N8)
///
/// The global `pane-died` and `after-new-session` hooks must be installed
/// separately (see `run_daemon`).
pub fn install_session_hooks(session_name: &str, hook_exe: &str) {
    let escaped = crate::daemon::utils::shell_escape_arg(session_name);

    // alert-bell: fire when a background pane rings the terminal bell.
    let bell_cmd = format!(
        "run-shell -b '{} notify activity #{{pane_id}} 0 \"{}\"'",
        hook_exe, escaped,
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args(["set-hook", "-t", session_name, "alert-bell", &bell_cmd])
        .output()
    {
        log::warn!(
            "Failed to register alert-bell hook for '{}': {}",
            session_name,
            e
        );
    }

    // pane-focus-in (N1): update active-pane cache instantly when focus moves.
    let focus_cmd = format!(
        "run-shell -b '{} notify focus #{{pane_id}} \"{}\"'",
        hook_exe, escaped,
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args(["set-hook", "-t", session_name, "pane-focus-in", &focus_cmd])
        .output()
    {
        log::warn!(
            "Failed to register pane-focus-in hook for '{}': {}",
            session_name,
            e
        );
    }

    // session-window-changed (N2): refresh window topology when the user switches windows.
    let window_cmd = format!(
        "run-shell -b '{} notify window-changed \"{}\"'",
        hook_exe, escaped,
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args([
            "set-hook",
            "-t",
            session_name,
            "session-window-changed",
            &window_cmd,
        ])
        .output()
    {
        log::warn!(
            "Failed to register session-window-changed hook for '{}': {}",
            session_name,
            e
        );
    }

    // client-resized (N8): update cached viewport dimensions when the terminal is resized.
    let resize_cmd = format!(
        "run-shell -b '{} notify resize #{{client_width}} #{{client_height}} \"{}\"'",
        hook_exe, escaped,
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args([
            "set-hook",
            "-t",
            session_name,
            "client-resized",
            &resize_cmd,
        ])
        .output()
    {
        log::warn!(
            "Failed to register client-resized hook for '{}': {}",
            session_name,
            e
        );
    }

    // session-closed (A6): clean up daemon state when this session is destroyed.
    // The session name is embedded directly (escaped) rather than via #{session_name}
    // to avoid relying on tmux format expansion after the session is already gone.
    let closed_cmd = format!(
        "run-shell -b '{} notify session-closed \"{}\"'",
        hook_exe, escaped,
    );
    if let Err(e) = std::process::Command::new("tmux")
        .args([
            "set-hook",
            "-t",
            session_name,
            "session-closed",
            &closed_cmd,
        ])
        .output()
    {
        log::warn!(
            "Failed to register session-closed hook for '{}': {}",
            session_name,
            e
        );
    }

    log::info!("Session hooks installed for: {}", session_name);
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

/// Log the outcome of a global tmux hook installation.
///
/// A `set-hook` that tmux rejects (e.g. a syntax error in the hook value)
/// still returns `Ok(output)` from the spawn — only the exit status and
/// stderr reveal the failure. Checking just the `io::Result` let the
/// `90567c3` quoting regression pass silently, so both halves are checked
/// here.
fn log_hook_install_result(hook: &str, res: std::io::Result<std::process::Output>) {
    match res {
        Err(e) => log::error!("Failed to register global tmux {hook} hook: {e}"),
        Ok(out) if !out.status.success() => log::error!(
            "Failed to register global tmux {hook} hook ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Ok(_) => {}
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

    // Determine the initial tmux session and whether the daemon owns it.
    //
    // Priority:
    //   1. If launched inside tmux ($TMUX is set), adopt the current session.
    //      The daemon does not own this session and will not recreate it if destroyed.
    //   2. Otherwise, use the configured name (CLI override > config.daemon.tmux_session
    //      > default "daemoneye").  If the session already exists, adopt it; if not,
    //      create it with `tmux new-session -d -s <name>` and bail on failure.
    //
    // A tmux session is always required — there is no degraded "no session" mode.
    let inside_session = crate::tmux::off_runtime("detect-session", detect_session)
        .await
        .flatten();
    let (initial_session, managed_session): (Option<String>, Option<String>) = if let Some(name) =
        inside_session
    {
        log::info!("Launched inside tmux — adopting session '{}'.", name);
        (Some(name), None)
    } else {
        let name = session_override.unwrap_or_else(|| startup_config.daemon.tmux_session.clone());
        let n = name.clone();
        let exists =
            crate::tmux::off_runtime("session-exists", move || crate::tmux::session_exists(&n))
                .await
                .unwrap_or(false);
        if exists {
            log::info!("Managed tmux session '{}' already exists — adopting.", name);
            (Some(name.clone()), Some(name))
        } else {
            let n = name.clone();
            let created = crate::tmux::off_runtime("new-session", move || {
                std::process::Command::new("tmux")
                    .args(["new-session", "-d", "-s", &n])
                    .output()
            })
            .await;

            match created {
                Some(Ok(o)) if o.status.success() => {
                    log::info!("Created managed tmux session '{}'.", name);
                    (Some(name.clone()), Some(name))
                }
                Some(Ok(o)) => {
                    let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    anyhow::bail!("Failed to create tmux session '{}': {}", name, stderr);
                }
                Some(Err(e)) => {
                    anyhow::bail!("tmux new-session failed for '{}': {}", name, e);
                }
                None => {
                    anyhow::bail!("timed out creating tmux session '{}'", name);
                }
            }
        }
    };

    log_event(
        "daemon_start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "session": initial_session.as_deref().unwrap_or(""),
            "socket":  default_socket_path().display().to_string(),
        }),
    );

    let hook_exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    // pane-died is a global hook — install it so it fires for all sessions.
    // M3: `#{q:session_name}` makes tmux shell-quote the expansion, so a
    // session name is inert in the run-shell body ("$()`, backticks, spaces,
    // even a literal ') — the name is unknown at install time for global
    // hooks, so it cannot be pre-escaped in Rust. Do NOT wrap the placeholder
    // in extra quotes: nested quotes are a tmux syntax error that makes
    // set-hook fail and leaves the hook unset.
    let global_notify_cmd = format!(
        "run-shell -b '{} notify activity #{{pane_id}} 0 #{{q:session_name}}'",
        hook_exe_path,
    );
    let c = global_notify_cmd.clone();
    let res = crate::tmux::off_runtime("set-hook-pane-died", move || {
        std::process::Command::new("tmux")
            .args(["set-hook", "-g", "pane-died", &c])
            .output()
    })
    .await
    .unwrap_or_else(|| Err(std::io::Error::other("timed out installing hook")));
    log_hook_install_result("pane-died", res);

    // after-new-session (N14): auto-install per-session hooks for any new tmux session,
    // so monitoring works immediately without requiring a first `daemoneye chat` invocation.
    let session_created_cmd = format!(
        "run-shell -b '{} notify session-created #{{q:session_name}}'",
        hook_exe_path,
    );
    let c = session_created_cmd.clone();
    let res = crate::tmux::off_runtime("set-hook-after-new-session", move || {
        std::process::Command::new("tmux")
            .args(["set-hook", "-g", "after-new-session", &c])
            .output()
    })
    .await
    .unwrap_or_else(|| Err(std::io::Error::other("timed out installing hook")));
    log_hook_install_result("after-new-session", res);

    // client-attached (N15): notify daemon when a terminal client re-attaches so it
    // can clear pending detach state and suppress the catch-up brief.
    let client_attached_cmd = format!(
        "run-shell -b '{} notify client-attached #{{q:session_name}}'",
        hook_exe_path,
    );
    let c = client_attached_cmd.clone();
    let res = crate::tmux::off_runtime("set-hook-client-attached", move || {
        std::process::Command::new("tmux")
            .args(["set-hook", "-g", "client-attached", &c])
            .output()
    })
    .await
    .unwrap_or_else(|| Err(std::io::Error::other("timed out installing hook")));
    log_hook_install_result("client-attached", res);

    // client-detached (N15): notify daemon when the terminal client detaches so it
    // can record the time and generate a catch-up brief on the next Ask.
    let client_detached_cmd = format!(
        "run-shell -b '{} notify client-detached #{{q:session_name}}'",
        hook_exe_path,
    );
    let c = client_detached_cmd.clone();
    let res = crate::tmux::off_runtime("set-hook-client-detached", move || {
        std::process::Command::new("tmux")
            .args(["set-hook", "-g", "client-detached", &c])
            .output()
    })
    .await
    .unwrap_or_else(|| Err(std::io::Error::other("timed out installing hook")));
    log_hook_install_result("client-detached", res);

    // Install per-session hooks if we already know the session.
    if let Some(ref sn) = initial_session {
        let sn = sn.clone();
        let hp = hook_exe_path.clone();
        let _ = crate::tmux::off_runtime("install-session-hooks", move || {
            install_session_hooks(&sn, &hp)
        })
        .await;
    }

    // Wrap managed_session in an Arc so it can be shared cheaply across
    // all spawned handle_client tasks.
    let managed_session: Arc<Option<String>> = Arc::new(managed_session);

    // bg_session is the tmux session used for background/scheduled job windows.
    // Starts empty when started by systemd; adopted from the first connecting client.
    let bg_session: Arc<Mutex<String>> =
        Arc::new(Mutex::new(initial_session.clone().unwrap_or_default()));

    let cache = Arc::new(SessionCache::new(initial_session.as_deref().unwrap_or("")));

    // N7: seed the initial client viewport dimensions now that the cache exists.
    if let Some(ref sn) = initial_session {
        let s = sn.to_string();
        let (w, h) = crate::tmux::off_runtime("client-dimensions", move || {
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
        let bg_sn_sup = Arc::clone(&bg_session);
        let cfg_sup = startup_config.clone();
        let sessions_sup = sessions.clone();
        let cache_sup = Arc::clone(&cache);
        tokio::spawn(supervise("scheduler", Arc::clone(&shutdown), move || {
            let store = Arc::clone(&store_sup);
            let bg_sn = Arc::clone(&bg_sn_sup);
            let cfg = cfg_sup.clone();
            let sessions_sched = sessions_sup.clone();
            let cache_sched = Arc::clone(&cache_sup);
            async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tick.tick().await;
                    let sn = bg_sn.lock().unwrap_or_log().clone();
                    if sn.is_empty() {
                        continue; // No session adopted yet; skip until a client connects.
                    }
                    let due = store.take_due();
                    for job in due {
                        let store2 = Arc::clone(&store);
                        let sn2 = sn.clone();
                        let cfg2 = cfg.clone();
                        let sessions2 = sessions_sched.clone();
                        let cache2 = Arc::clone(&cache_sched);
                        tokio::spawn(async move {
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
                    let (evicted, active_ids) = crate::daemon::session::cleanup_pass(
                        &sessions_cleanup,
                        Instant::now(),
                        Duration::from_secs(1800),
                    );

                    // Unlocked phase: everything blocking happens out here.
                    for entry in &evicted {
                        let teardown = entry.bg_teardown();
                        let _ = crate::tmux::off_runtime("bg-teardown", move || {
                            crate::daemon::session::run_bg_teardown(teardown)
                        })
                        .await;
                    }

                    sweep_counter = sweep_counter.wrapping_add(1);
                    if sweep_counter.is_multiple_of(60) {
                        crate::daemon::utils::sweep_event_segments(
                            startup_config.events.retention_days,
                        );
                        crate::daemon::utils::sweep_session_archives(
                            startup_config.sessions.archive_retention_days,
                            &active_ids,
                        );
                        crate::daemon::utils::sweep_pane_logs(
                            startup_config.retention.pane_log_retention_days,
                        );
                        crate::daemon::utils::sweep_agent_mailboxes(
                            startup_config.retention.mailbox_retention_days,
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

    // Periodic GC of background windows: kills dead, idle-completed, and orphaned windows.
    let sessions_gc_sup = sessions.clone();
    tokio::spawn(supervise(
        "bg-window-gc",
        Arc::clone(&shutdown),
        move || {
            let sessions_gc = sessions_gc_sup.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let s = sessions_gc.clone();
                    let _ = crate::tmux::off_runtime("gc-bg-windows", move || {
                        crate::daemon::background::gc_bg_windows(&s)
                    })
                    .await;
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
                        let bg_conn = Arc::clone(&bg_session);
                        let managed_conn = Arc::clone(&managed_session);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, cache_conn, sessions_conn, sched_conn, bg_conn, managed_conn).await {
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

    // 2. Uninstall global tmux hooks so they don't fire against a dead daemon.
    for hook in &[
        "pane-died",
        "after-new-session",
        "client-attached",
        "client-detached",
    ] {
        let h = hook.to_string();
        let res = crate::tmux::off_runtime("set-hook-unset", move || {
            std::process::Command::new("tmux")
                .args(["set-hook", "-gu", &h])
                .output()
        })
        .await
        .unwrap_or_else(|| Err(std::io::Error::other("timed out uninstalling hook")));
        if let Err(e) = res {
            log::warn!("Failed to uninstall global tmux hook '{}': {}", hook, e);
        }
    }

    // 3. Stop pipe-pane logs but leave background windows alive.
    //    Killing daemon-managed (de-*) windows during shutdown can deplete the
    //    session's window count and cause tmux to destroy the session, losing any
    //    user-created panes and windows.  Windows are left intact so the session
    //    survives; orphaned de-* windows from this run are cleaned up automatically
    //    the next time the session's 30-minute GC fires or on daemon restart.
    {
        let pipe_panes: Vec<String> = crate::daemon::session::with_sessions(&sessions, |store| {
            store
                .values()
                .filter_map(|entry| entry.pipe_source_pane.clone())
                .filter(|pane_id| !pane_id.is_empty())
                .collect()
        });
        for pane_id in &pipe_panes {
            let p = pane_id.clone();
            let _ =
                crate::tmux::off_runtime("stop-pipe-pane", move || crate::tmux::stop_pipe_pane(&p))
                    .await;
        }
    }

    log::info!("Daemon stopped cleanly.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    // ---------------------------------------------------------------------------
    // D6 predicate tests — pure, no tmux or cache
    // ---------------------------------------------------------------------------

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
