//! `daemoneye doctor` — one line per readiness check, so a new host answers
//! "can DaemonEye run here?" with a single command. Runs without a daemon.
//!
//! The split is strict: [`gather`] does every probe (config load, PATH
//! lookups, the daemon socket, the sbx preflight) and hands the facts to the
//! pure [`evaluate`], which decides levels and details with no I/O. The tests
//! build [`DoctorInputs`] by hand and call [`evaluate`] directly, so they are
//! hermetic whatever tmux, sbx or daemon the host has.

use anyhow::Result;

use crate::cli::palette::{self, ColorDepth};

/// How ready one check says the host is. `Warn` is reported, never counted
/// against the exit code: doctor checks readiness, not uptime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// One readiness check: what was probed, what was found, and (when there is a
/// concrete remedy) how to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            level: Level::Ok,
            detail: detail.into(),
            fix: None,
        }
    }

    fn warn(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Check {
            name,
            level: Level::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>, fix: Option<String>) -> Self {
        Check {
            name,
            level: Level::Fail,
            detail: detail.into(),
            fix,
        }
    }
}

/// The facts every check reads, gathered once by [`gather`]. Plain data —
/// booleans, strings, `Option`s — so `evaluate` stays pure and the tests can
/// hand-build it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorInputs {
    /// `Ok(())` when `Config::load()` succeeded, `Err(text)` when not.
    pub config: Result<(), String>,
    /// The default model's resolved key: empty when absent.
    pub api_key: String,
    /// The env var holding the default provider's key ("" for local providers).
    pub api_key_env_var: &'static str,
    /// The default provider's name, for the api-key detail.
    pub api_provider: String,
    /// The login shell PTY hosts start (`shell::login_shell()`).
    pub login_shell: String,
    /// The login shell exists and is executable.
    pub shell_executable: bool,
    /// The daemon's liveness, already probed.
    pub daemon_running: bool,
    /// The daemon answered the socket but not like a DaemonEye daemon.
    pub daemon_unresponsive: bool,
    /// `[sandbox] enabled`.
    pub sandbox_enabled: bool,
    /// The sbx preflight's version line when it ran and passed; `None` when
    /// the sandbox is disabled or preflight did not run.
    pub sandbox_preflight_ok: Option<String>,
    /// The `SbxUnavailable` operator-facing reason when preflight ran and
    /// failed.
    pub sandbox_unavailable: Option<String>,
    /// How many `[hosts.*]` entries are configured.
    pub host_count: usize,
    /// Whether `ssh` is on `PATH`.
    pub ssh_present: bool,
    /// Whether the `tmux` binary is on `PATH`.
    pub tmux_present: bool,
    /// Whether `$TMUX` is set (inside a tmux session).
    pub inside_tmux: bool,
}

impl Default for DoctorInputs {
    fn default() -> Self {
        Self {
            config: Ok(()),
            api_key: String::new(),
            api_key_env_var: "",
            api_provider: "anthropic".to_string(),
            login_shell: String::new(),
            shell_executable: false,
            daemon_running: false,
            daemon_unresponsive: false,
            sandbox_enabled: false,
            sandbox_preflight_ok: None,
            sandbox_unavailable: None,
            host_count: 0,
            ssh_present: false,
            tmux_present: false,
            inside_tmux: false,
        }
    }
}

/// The 1 when any check is `Fail`, the 0 otherwise.
pub fn exit_code(checks: &[Check]) -> i32 {
    if checks.iter().any(|c| c.level == Level::Fail) {
        1
    } else {
        0
    }
}

/// The pure decision: facts in, checks out, in the Spec's fixed order.
pub fn evaluate(inputs: &DoctorInputs) -> Vec<Check> {
    vec![
        config_check(inputs),
        api_key_check(inputs),
        shell_check(inputs),
        daemon_check(inputs),
        sandbox_check(inputs),
        hosts_check(inputs),
        tmux_check(inputs),
    ]
}

fn config_check(inputs: &DoctorInputs) -> Check {
    match &inputs.config {
        Ok(()) => Check::ok("config", "loaded"),
        Err(text) => Check::fail(
            "config",
            text.clone(),
            Some("fix config.toml in ~/.daemoneye/etc/".to_string()),
        ),
    }
}

fn api_key_check(inputs: &DoctorInputs) -> Check {
    if !inputs.api_key.is_empty() {
        return Check::ok("api key", format!("set ({})", inputs.api_provider));
    }
    let fix = if inputs.api_key_env_var.is_empty() {
        format!(
            "set api_key in [models.default] in ~/.daemoneye/etc/config.toml (provider {})",
            inputs.api_provider
        )
    } else {
        format!(
            "export {}=<your-key> (or set api_key in [models.default])",
            inputs.api_key_env_var
        )
    };
    Check::fail(
        "api key",
        format!("no key for provider {}", inputs.api_provider),
        Some(fix),
    )
}

fn shell_check(inputs: &DoctorInputs) -> Check {
    if inputs.shell_executable {
        Check::ok("shell", inputs.login_shell.clone())
    } else {
        Check::fail(
            "shell",
            format!("{} not found or not executable", inputs.login_shell),
            Some("set SHELL to an existing shell".to_string()),
        )
    }
}

fn daemon_check(inputs: &DoctorInputs) -> Check {
    if inputs.daemon_running {
        return Check::ok("daemon", "running");
    }
    let detail = if inputs.daemon_unresponsive {
        "not answering".to_string()
    } else {
        "not running".to_string()
    };
    Check::warn("daemon", detail, "daemoneye daemon".to_string())
}

fn sandbox_check(inputs: &DoctorInputs) -> Check {
    if !inputs.sandbox_enabled {
        return Check::ok("sandbox", "disabled");
    }
    if let Some(version) = &inputs.sandbox_preflight_ok {
        return Check::ok("sandbox", version.clone());
    }
    match &inputs.sandbox_unavailable {
        Some(reason) => Check::fail("sandbox", reason.clone(), None),
        None => Check::fail(
            "sandbox",
            "enabled but its preflight never ran".to_string(),
            None,
        ),
    }
}

fn hosts_check(inputs: &DoctorInputs) -> Check {
    if inputs.host_count == 0 {
        return Check::ok("hosts", "none configured");
    }
    let count = if inputs.host_count == 1 {
        "1 host".to_string()
    } else {
        format!("{} hosts", inputs.host_count)
    };
    if inputs.ssh_present {
        Check::ok("hosts", format!("{count} configured; ssh found"))
    } else {
        Check::fail(
            "hosts",
            format!("{count} configured but ssh is not on PATH"),
            Some("install openssh-clients or add ssh to PATH".to_string()),
        )
    }
}

fn tmux_check(inputs: &DoctorInputs) -> Check {
    match (inputs.tmux_present, inputs.inside_tmux) {
        (false, false) => Check::ok("tmux", "not installed (optional)"),
        (true, false) => Check::ok("tmux", "installed; not inside a session"),
        (true, true) => Check::ok("tmux", "read-only pane context available"),
        (false, true) => Check::warn(
            "tmux",
            "$TMUX is set but the tmux binary is missing",
            "install tmux or unset $TMUX".to_string(),
        ),
    }
}

/// The I/O side: run every probe, keep going when one errors, hand the facts
/// to [`evaluate`]. Never fails as a whole — a probe that errors becomes that
/// check's `Fail` or `Warn`.
pub async fn gather() -> DoctorInputs {
    let (config, api_key, api_key_env_var, api_provider, sandbox_enabled, host_count) =
        match crate::config::Config::load() {
            Ok(cfg) => {
                let model = cfg.resolve_model(None);
                (
                    Ok(()),
                    model.resolve_api_key(),
                    model.api_key_env_var(),
                    model.provider.clone(),
                    cfg.sandbox.enabled,
                    cfg.hosts.len(),
                )
            }
            // A broken config fails only the config check; the rest of the
            // probes still run, because doctor reports rather than stops.
            Err(e) => (
                Err(e.to_string()),
                String::new(),
                "",
                "anthropic".to_string(),
                false,
                0,
            ),
        };

    let daemon = crate::daemon::daemon_liveness().await;
    let login_shell = crate::shell::login_shell();
    let shell_executable = login_shell_executable(&login_shell);

    let mut inputs = DoctorInputs {
        config,
        api_key,
        api_key_env_var,
        api_provider,
        login_shell,
        shell_executable,
        daemon_running: daemon == crate::daemon::DaemonLiveness::Running,
        daemon_unresponsive: matches!(
            daemon,
            crate::daemon::DaemonLiveness::Unresponsive | crate::daemon::DaemonLiveness::Confused
        ),
        sandbox_enabled,
        sandbox_preflight_ok: None,
        sandbox_unavailable: None,
        host_count,
        ssh_present: binary_present("ssh"),
        tmux_present: binary_present("tmux"),
        inside_tmux: std::env::var("TMUX").is_ok_and(|v| !v.is_empty()),
    };

    if inputs.sandbox_enabled {
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        let run = crate::shell::real_runner(program);
        match crate::daemon::executor::sbx_lifecycle::preflight(&run).await {
            Ok(version) => inputs.sandbox_preflight_ok = Some(version),
            Err(reason) => {
                inputs.sandbox_unavailable =
                    Some(crate::daemon::executor::sbx::describe_unavailable(&reason))
            }
        }
    }
    inputs
}

/// True when `shell` resolves on PATH (or absolutely) and is executable.
/// A spawn failure of any other kind reads as not-executable, which is the
/// answer doctor would give anyway.
fn login_shell_executable(shell: &str) -> bool {
    let candidate = std::path::Path::new(shell);
    if candidate.is_absolute() {
        return is_executable_file(candidate);
    }
    std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .map(|dir| dir.join(shell))
        .any(|p| is_executable_file(&p))
}

fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Is `name` on PATH as a plain file? The same shape `which_ssh` / `which_sbx`
/// use.
fn binary_present(name: &str) -> bool {
    std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .any(|dir| dir.join(name).is_file())
}

// ── Colour helpers (the same palette status.rs renders with) ─────────────────

fn depth() -> ColorDepth {
    static DEPTH: std::sync::OnceLock<ColorDepth> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(palette::detect_from_env)
}

fn c_ok(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (80, 210, 130), 78, 92)
    )
}
fn c_warn(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (250, 190, 50), 214, 93)
    )
}
fn c_err(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (230, 80, 80), 167, 91)
    )
}
fn c_dim(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (80, 80, 105), 60, 90)
    )
}

fn level_tag(level: Level) -> String {
    match level {
        Level::Ok => c_ok("OK "),
        Level::Warn => c_warn("WARN"),
        Level::Fail => c_err("FAIL"),
    }
}

/// Print one line per check plus the `fix:` hints and the summary line.
fn print_report(checks: &[Check]) {
    let mut failed = 0usize;
    let mut warned = 0usize;
    for check in checks {
        println!(
            "{}  {}: {}",
            level_tag(check.level),
            check.name,
            check.detail
        );
        if let Some(fix) = &check.fix {
            println!("         fix: {}", c_dim(fix));
        }
        match check.level {
            Level::Fail => failed += 1,
            Level::Warn => warned += 1,
            Level::Ok => {}
        }
    }
    let ok = checks.len() - failed - warned;
    let failed_s = if failed > 0 {
        c_err(&format!("{failed} failed"))
    } else {
        c_ok(&format!("{failed} failed"))
    };
    println!(
        "\n{}  {}  {}",
        c_ok(&format!("{ok} ok")),
        c_warn(&format!("{warned} warn")),
        failed_s
    );
}

/// `daemoneye doctor` — run the checks, print them, exit with [`exit_code`].
pub async fn run_doctor() -> Result<()> {
    let inputs = gather().await;
    let checks = evaluate(&inputs);
    print_report(&checks);
    let code = exit_code(&checks);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs_base() -> DoctorInputs {
        DoctorInputs {
            config: Ok(()),
            api_key: "sk-test".to_string(),
            api_key_env_var: "ANTHROPIC_API_KEY",
            api_provider: "anthropic".to_string(),
            login_shell: "/bin/bash".to_string(),
            shell_executable: true,
            daemon_running: true,
            daemon_unresponsive: false,
            sandbox_enabled: false,
            sandbox_preflight_ok: None,
            sandbox_unavailable: None,
            host_count: 0,
            ssh_present: true,
            tmux_present: false,
            inside_tmux: false,
        }
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no {name} check in {checks:?}"))
    }

    #[test]
    fn doctor_passes_on_a_host_without_tmux() {
        let inputs = DoctorInputs {
            tmux_present: false,
            inside_tmux: false,
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        assert!(
            checks.iter().all(|c| c.level != Level::Fail),
            "no check may fail on a healthy tmux-less host: {checks:?}"
        );
        let tmux = find(&checks, "tmux");
        assert_eq!(tmux.level, Level::Ok);
        assert!(
            tmux.detail.contains("optional"),
            "the tmux detail must say optional: {}",
            tmux.detail
        );
        assert_eq!(exit_code(&checks), 0);
    }

    #[test]
    fn doctor_fails_on_a_missing_api_key() {
        let inputs = DoctorInputs {
            api_key: String::new(),
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let key = find(&checks, "api key");
        assert_eq!(key.level, Level::Fail);
        let fix = key.fix.as_deref().expect("the api-key fix must exist");
        assert!(
            fix.contains("ANTHROPIC_API_KEY"),
            "the fix must name the env var: {fix}"
        );
        assert_eq!(exit_code(&checks), 1);
    }

    #[test]
    fn doctor_fails_when_the_sandbox_is_enabled_but_sbx_is_unavailable() {
        let inputs = DoctorInputs {
            sandbox_enabled: true,
            sandbox_unavailable: Some("sandbox unavailable: the OS keychain is locked".into()),
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let sandbox = find(&checks, "sandbox");
        assert_eq!(sandbox.level, Level::Fail);
        assert!(
            sandbox.detail.contains("keychain"),
            "the detail carries the SbxUnavailable reason: {}",
            sandbox.detail
        );
        assert_eq!(exit_code(&checks), 1);
    }

    #[test]
    fn doctor_warns_but_passes_when_the_daemon_is_down() {
        let inputs = DoctorInputs {
            daemon_running: false,
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let daemon = find(&checks, "daemon");
        assert_eq!(daemon.level, Level::Warn);
        assert_eq!(
            daemon.fix.as_deref(),
            Some("daemoneye daemon"),
            "the fix must name the start command"
        );
        assert_eq!(exit_code(&checks), 0);
    }

    #[test]
    fn doctor_never_fails_on_tmux() {
        for (present, inside, want) in [
            (false, false, (Level::Ok, "optional")),
            (true, false, (Level::Ok, "installed")),
            (true, true, (Level::Ok, "pane context")),
            (false, true, (Level::Warn, "$TMUX")),
        ] {
            let inputs = DoctorInputs {
                tmux_present: present,
                inside_tmux: inside,
                ..inputs_base()
            };
            let checks = evaluate(&inputs);
            let tmux = find(&checks, "tmux");
            assert_eq!(
                tmux.level, want.0,
                "tmux present={present} inside={inside} must be {:?}",
                want.0
            );
            assert!(
                tmux.detail.contains(want.1),
                "detail must mention `{}`: {}",
                want.1,
                tmux.detail
            );
            assert_ne!(
                tmux.level,
                Level::Fail,
                "tmux never fails, at present={present} inside={inside}"
            );
        }
    }

    #[test]
    fn doctor_fails_on_hosts_without_ssh() {
        let inputs = DoctorInputs {
            host_count: 1,
            ssh_present: false,
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let hosts = find(&checks, "hosts");
        assert_eq!(hosts.level, Level::Fail);
        assert_eq!(exit_code(&checks), 1);
    }

    #[test]
    fn doctor_ok_on_hosts_with_ssh() {
        let inputs = DoctorInputs {
            host_count: 2,
            ssh_present: true,
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let hosts = find(&checks, "hosts");
        assert_eq!(hosts.level, Level::Ok);
        assert!(
            hosts.detail.contains('2'),
            "the detail names the count: {}",
            hosts.detail
        );
    }

    #[test]
    fn doctor_fails_on_a_config_load_error() {
        let inputs = DoctorInputs {
            config: Err("parsing ~/.daemoneye/etc/config.toml: bad value".into()),
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let config = find(&checks, "config");
        assert_eq!(config.level, Level::Fail);
        assert!(
            config.detail.contains("bad value"),
            "the detail carries the error text: {}",
            config.detail
        );
        assert_eq!(exit_code(&checks), 1);
    }

    #[test]
    fn doctor_shell_fails_when_not_executable() {
        let inputs = DoctorInputs {
            login_shell: "/bin/nope".to_string(),
            shell_executable: false,
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let shell = find(&checks, "shell");
        assert_eq!(shell.level, Level::Fail);
        assert!(
            shell.detail.contains("/bin/nope"),
            "the detail names the shell: {}",
            shell.detail
        );
        assert_eq!(exit_code(&checks), 1);
    }

    #[test]
    fn doctor_sandbox_ok_carries_the_version_line() {
        let inputs = DoctorInputs {
            sandbox_enabled: true,
            sandbox_preflight_ok: Some("sbx v0.46.0".to_string()),
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        let sandbox = find(&checks, "sandbox");
        assert_eq!(sandbox.level, Level::Ok);
        assert!(
            sandbox.detail.contains("v0.46.0"),
            "the Ok detail is the version line: {}",
            sandbox.detail
        );
    }

    #[test]
    fn doctor_local_provider_key_never_fails() {
        let inputs = DoctorInputs {
            api_key: "local".to_string(),
            api_key_env_var: "",
            api_provider: "ollama".to_string(),
            ..inputs_base()
        };
        let checks = evaluate(&inputs);
        assert_eq!(find(&checks, "api key").level, Level::Ok);
    }

    #[test]
    fn doctor_orders_the_seven_checks() {
        let checks = evaluate(&inputs_base());
        let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            vec![
                "config", "api key", "shell", "daemon", "sandbox", "hosts", "tmux"
            ],
            "the Spec's fixed order"
        );
    }
}
