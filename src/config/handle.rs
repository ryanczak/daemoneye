//! The process-wide config handle: one `Config` the daemon swaps on reload.

use super::*;
use crate::util::UnpoisonExt;
use std::sync::RwLock;

/// The handle the daemon installs at startup. `reload()` re-reads
/// `config.toml`, keeps restart-only sections at their running values, and
/// reports which keys changed (applied now) and which wait for a restart.
pub struct ConfigHandle {
    running: RwLock<std::sync::Arc<Config>>,
}

impl ConfigHandle {
    /// Build a handle from an already-loaded `Config` (the daemon's startup load).
    pub fn new(start: Config) -> Self {
        ConfigHandle {
            running: RwLock::new(std::sync::Arc::new(start)),
        }
    }

    /// A clone of the running `Arc<Config>` — hand it out and drop the lock.
    pub fn current(&self) -> std::sync::Arc<Config> {
        self.running.read().unwrap_or_log().clone()
    }

    /// Re-read `config.toml` the way `Config::load` does and swap what is
    /// safe to swap. Returns the reload report; the running config is only
    /// touched when the new file parses and deserializes.
    pub fn reload(&self) -> ReloadReport {
        match Config::load() {
            Err(e) => ReloadReport {
                applied: Vec::new(),
                restart_required: Vec::new(),
                error: Some(e),
            },
            Ok(mut fresh) => {
                let old = self.current();
                // Restart-only sections keep their running values; keys that
                // changed inside one are reported as waiting for a restart.
                let mut restart_required = Vec::new();
                for name in RESTART_ONLY_SECTIONS {
                    let (old_v, new_v) = (section_json(&old, name), section_json(&fresh, name));
                    if old_v != new_v {
                        restart_required.extend(dotted_diff(
                            name,
                            old_v.as_ref().unwrap_or(&serde_json::Value::Null),
                            new_v.as_ref().unwrap_or(&serde_json::Value::Null),
                        ));
                    }
                }
                fresh.daemon = old.daemon.clone();
                fresh.webhook = old.webhook.clone();
                fresh.sandbox = old.sandbox.clone();
                fresh.execution = old.execution.clone();

                let applied = dotted_diff_map(&old, &fresh);
                {
                    let mut live = self.running.write().unwrap_or_log();
                    *live = std::sync::Arc::new(fresh);
                }
                ReloadReport {
                    applied,
                    restart_required: sorted(restart_required),
                    error: None,
                }
            }
        }
    }
}

// ── Install once from run_daemon, read everywhere ────────────────────────────

static HANDLE: RwLock<Option<std::sync::Arc<ConfigHandle>>> = RwLock::new(None);

/// The startup load's error, carried beside the handle. `Some` means the
/// daemon is running on **fallback defaults** because its startup
/// `Config::load()` failed — the condition the ghost foreground gate fails
/// closed on (bug-19-1). A successful reload clears only the parts of the
/// condition the swap actually covered: `sandbox` is restart-only, so a
/// ghost stays refused until a restart with a readable file.
static STARTUP_ERROR: RwLock<Option<anyhow::Error>> = RwLock::new(None);

/// Install the process-wide handle. Called once by `run_daemon` at startup.
pub fn install(handle: std::sync::Arc<ConfigHandle>) {
    *HANDLE.write().unwrap_or_log() = Some(handle);
}
/// Record the startup load's outcome beside the handle. Called once by
/// `run_daemon`: `Err` when it fell back to defaults, `Ok(())` otherwise.
pub fn set_startup_error(result: anyhow::Result<()>) {
    *STARTUP_ERROR.write().unwrap_or_log() = result.err();
}

/// The startup load's error, when the daemon runs on fallback defaults.
/// Uninstalled (CLI processes, unit tests), there is no fallback condition
/// and `None` is returned.
pub fn startup_error() -> Option<anyhow::Error> {
    if !installed() {
        return None;
    }
    STARTUP_ERROR
        .read()
        .unwrap_or_log()
        .as_ref()
        .map(|e| anyhow::Error::msg(e.to_string()))
}
/// Is a handle installed? Tests that own a handle never install it.
pub fn installed() -> bool {
    HANDLE.read().unwrap_or_log().is_some()
}

/// Read the running config as an owned `Config`. Structured callers (the
/// scheduler task passes a `Config` into a spawned job) use this over
/// [`current`]; the clone is one arc-deref deep, not a model repaint.
pub fn schedule_config() -> Config {
    (*current()).clone()
}

/// Read the running config. Uninstalled, this loads from disk on each call —
/// the behaviour `Config::load().unwrap_or_default()` had at every per-request
/// site, so CLI processes and unit tests see no change.
pub fn current() -> std::sync::Arc<Config> {
    if let Some(h) = HANDLE.read().unwrap_or_log().clone() {
        return h.current();
    }
    std::sync::Arc::new(Config::load().unwrap_or_default())
}

/// The daemon-side reload: re-read the file, swap what is safe, re-init
/// masking and the stream-timeout budgets, and hand back the two lists the
/// IPC reply carries. `Ok` means the file was accepted (a no-change reload
/// gives two empty lists); `Err` is the refusal — the running config is
/// untouched and the error carries its path and line.
pub fn daemon_reload() -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let handle = HANDLE.read().unwrap_or_log().clone().ok_or_else(|| {
        anyhow::anyhow!("no config handle is installed — reload is a daemon-side op")
    })?;
    let report = handle.reload();
    match report.error {
        Some(e) => Err(e),
        None => {
            // Masking and the stream budgets follow the swap. A turn already
            // streaming keeps the budgets it started with.
            crate::ai::filter::init_masking(&handle.current().masking.extra_patterns);
            crate::ai::init_stream_timeouts(&handle.current().ai);
            Ok((report.applied, report.restart_required))
        }
    }
}

#[derive(Debug)]
pub struct ReloadReport {
    /// Dotted keys swapped into the running config, sorted, values never named.
    pub applied: Vec<String>,
    /// Dotted keys under a restart-only section that changed on disk.
    pub restart_required: Vec<String>,
    /// Refusal: the file failed to load; the running config is untouched.
    pub error: Option<anyhow::Error>,
}

/// The sections a reload refuses to swap live: binding, sandbox creation and
/// the execution substrate all latch at startup.
const RESTART_ONLY_SECTIONS: [&str; 4] = ["daemon", "webhook", "sandbox", "execution"];

/// Serialize one top-level section to JSON so two `Config`s can be compared
/// section by section. `name` is a `[models.*]`-free top-level key.
fn section_json(cfg: &Config, name: &str) -> Option<serde_json::Value> {
    let value = match name {
        "ai" => serde_json::to_value(&cfg.ai),
        "models" => serde_json::to_value(&cfg.models),
        "hosts" => serde_json::to_value(&cfg.hosts),
        "masking" => serde_json::to_value(&cfg.masking),
        "context" => serde_json::to_value(&cfg.context),
        "notifications" => serde_json::to_value(&cfg.notifications),
        "webhook" => serde_json::to_value(&cfg.webhook),
        "ghost" => serde_json::to_value(&cfg.ghost),
        "daemon" => serde_json::to_value(&cfg.daemon),
        "digest" => serde_json::to_value(&cfg.digest),
        "approvals" => serde_json::to_value(&cfg.approvals),
        "limits" => serde_json::to_value(&cfg.limits),
        "compaction" => serde_json::to_value(&cfg.compaction),
        "sessions" => serde_json::to_value(&cfg.sessions),
        "events" => serde_json::to_value(&cfg.events),
        "logging" => serde_json::to_value(&cfg.logging),
        "retention" => serde_json::to_value(&cfg.retention),
        "sandbox" => serde_json::to_value(&cfg.sandbox),
        "execution" => serde_json::to_value(&cfg.execution),
        "shells" => serde_json::to_value(&cfg.shells),
        "budget" => serde_json::to_value(&cfg.budget),
        _ => return None,
    };
    value.ok()
}

/// Dotted keys whose values differ between two configs, one section at a
/// time, then sorted. Values are compared but never carried — the report
/// names keys only.
fn dotted_diff_map(old: &Config, fresh: &Config) -> Vec<String> {
    let mut keys = Vec::new();
    for name in ALL_SECTIONS {
        let (Some(a), Some(b)) = (section_json(old, name), section_json(fresh, name)) else {
            continue;
        };
        if a != b {
            keys.extend(dotted_diff(name, &a, &b));
        }
    }
    sorted(keys)
}

/// All top-level section names, in `{}`-iterable order.
const ALL_SECTIONS: &[&str] = &[
    "ai",
    "models",
    "hosts",
    "masking",
    "context",
    "notifications",
    "webhook",
    "ghost",
    "daemon",
    "digest",
    "approvals",
    "limits",
    "compaction",
    "sessions",
    "events",
    "logging",
    "retention",
    "sandbox",
    "execution",
    "shells",
    "budget",
];

/// Flatten a JSON diff into `section.key` leaf paths. Two objects are walked
/// key-wise; anything else compares whole. A leaf that differs becomes
/// `"{prefix}.{key}"`; a `models.<name>.api_key` shape comes out as-is.
fn dotted_diff(prefix: &str, old: &serde_json::Value, fresh: &serde_json::Value) -> Vec<String> {
    match (old, fresh) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            let mut out = Vec::new();
            for (key, bv) in b {
                let av = a.get(key);
                if av != Some(bv) {
                    out.extend(dotted_diff(
                        &format!("{prefix}.{key}"),
                        &av.cloned().unwrap_or(serde_json::Value::Null),
                        bv,
                    ));
                }
            }
            for key in a.keys() {
                if !b.contains_key(key) {
                    out.push(format!("{prefix}.{key}"));
                }
            }
            out
        }
        _ => vec![prefix.to_string()],
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::etc_dir;

    /// Write `etc/config.toml` under the test HOME, so handle reloads see it.
    fn write_config(text: &str) {
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(etc_dir().join("config.toml"), text).expect("write config.toml");
    }

    #[test]
    fn reload_swaps_a_runtime_section_and_names_the_changed_key() {
        let (_guard, _home) = crate::test_home();
        write_config("[limits]\ntotal_tool_calls_per_turn = 7\n");
        let start = Config::load().expect("start config");
        let handle = std::sync::Arc::new(ConfigHandle::new(start));

        write_config("[limits]\ntotal_tool_calls_per_turn = 42\n");
        let report = handle.reload();

        assert!(
            report.error.is_none(),
            "a valid file reloads cleanly: {:?}",
            report.error
        );
        assert_eq!(
            report.applied,
            vec!["limits.total_tool_calls_per_turn".to_string()],
            "the changed key is named by its dotted path, got {:?}",
            report.applied
        );
        assert!(
            report.restart_required.is_empty(),
            "limits is runtime-swappable"
        );
        assert_eq!(
            handle.current().limits.total_tool_calls_per_turn,
            42,
            "the value followed the swap"
        );
    }

    #[test]
    fn reload_refuses_an_invalid_file_and_keeps_the_running_config() {
        let _guard = crate::test_home();
        write_config("[limits]\ntotal_tool_calls_per_turn = 7\n");
        let handle = std::sync::Arc::new(ConfigHandle::new(Config::load().expect("start")));

        write_config("limits = total [nonsense");
        let report = handle.reload();

        let err = report
            .error
            .as_ref()
            .expect("a broken file is refused, not defaulted");
        let text = format!("{err:#}");
        assert!(
            text.contains("config.toml"),
            "the refusal names the file: {text}"
        );
        assert!(
            text.contains("line"),
            "the refusal carries the toml line: {text}"
        );
        assert!(report.applied.is_empty() && report.restart_required.is_empty());
        assert_eq!(
            handle.current().limits.total_tool_calls_per_turn,
            7,
            "the running config survives the refusal"
        );
    }

    #[test]
    fn reload_holds_restart_only_sections_and_names_them() {
        let _guard = crate::test_home();
        let handle = std::sync::Arc::new(ConfigHandle::new(Config::default()));

        write_config("[sandbox]\nenabled = true\n");
        let report = handle.reload();

        assert!(
            report.error.is_none(),
            "sandbox.enabled parses: {:?}",
            report.error
        );
        assert!(
            report
                .restart_required
                .contains(&"sandbox.enabled".to_string()),
            "a changed [sandbox] key waits for a restart: {:?}",
            report.restart_required
        );
        assert!(
            !report.applied.contains(&"sandbox.enabled".to_string()),
            "a restart-only key is never reported as applied: {:?}",
            report.applied
        );
        assert!(
            !handle.current().sandbox.enabled,
            "the running sandbox stays off until a restart"
        );
    }

    #[test]
    fn reload_reply_names_keys_never_values() {
        let _guard = crate::test_home();
        write_config(
            "[models.default]\nprovider = \"anthropic\"\nmodel = \"claude\"\napi_key = \"sk-old\"\n[limits]\ntotal_tool_calls_per_turn = 1\n",
        );
        let handle = std::sync::Arc::new(ConfigHandle::new(Config::load().expect("start")));

        write_config(
            "[models.default]\nprovider = \"anthropic\"\nmodel = \"claude\"\napi_key = \"sk-new-secret-value\"\n[limits]\ntotal_tool_calls_per_turn = 1\n",
        );
        let report = handle.reload();

        let named = report.applied.join(";");
        assert!(
            named.contains("models.default.api_key"),
            "the changed key is named: {named}"
        );
        assert_eq!(
            handle
                .current()
                .models
                .get("default")
                .map(|m| m.api_key.clone()),
            Some("sk-new-secret-value".to_string()),
            "models swaps live: the running config carries the new key"
        );
        let all = format!("{report:?}");
        assert!(
            !all.contains("sk-new-secret-value") && !all.contains("sk-old"),
            "no value is ever printed: {all}"
        );
    }

    #[test]
    fn reload_with_no_change_reports_nothing_changed() {
        let _guard = crate::test_home();
        write_config("[limits]\ntotal_tool_calls_per_turn = 7\n");
        let handle = std::sync::Arc::new(ConfigHandle::new(Config::load().expect("start")));

        let report = handle.reload();
        assert!(report.error.is_none());
        assert!(
            report.applied.is_empty() && report.restart_required.is_empty(),
            "no change gives two empty lists: applied={:?} restart={:?}",
            report.applied,
            report.restart_required
        );
    }

    #[test]
    fn an_uninstalled_handle_reads_the_file_each_call() {
        let _guard = crate::test_home();
        assert!(!installed(), "unit tests never install the global handle");

        write_config("[limits]\ntotal_tool_calls_per_turn = 7\n");
        assert_eq!(super::current().limits.total_tool_calls_per_turn, 7);

        write_config("[limits]\ntotal_tool_calls_per_turn = 9\n");
        assert_eq!(
            super::current().limits.total_tool_calls_per_turn,
            9,
            "uninstalled, current() re-reads the file on every call"
        );
    }
}
