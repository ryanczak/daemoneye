use anyhow::{Context, Result};
use std::path::PathBuf;

use super::seeds::SRE_PROMPT_TOML;
use super::types::*;

/// Returns `~/.daemoneye/` (or `/tmp/.daemoneye/` if HOME is unset).
pub fn config_dir() -> PathBuf {
    let mut p = dirs_next();
    p.push(".daemoneye");
    p
}

/// `~/.daemoneye/etc/` — user-editable configuration files.
pub fn etc_dir() -> PathBuf {
    config_dir().join("etc")
}

/// `~/.daemoneye/var/run/` — sockets, lock files, mutable runtime state.
pub fn var_run_dir() -> PathBuf {
    config_dir().join("var/run")
}

/// `~/.daemoneye/var/log/` — application and pane interaction logs.
pub fn var_log_dir() -> PathBuf {
    config_dir().join("var/log")
}

/// `~/.daemoneye/var/log/pipe/` — per-pane pipe-pane capture logs.
pub fn pipe_log_dir() -> PathBuf {
    config_dir().join("var/log/pipe")
}

/// `~/.daemoneye/var/log/panes/` — archived background-window scrollback logs.
pub fn pane_logs_dir() -> PathBuf {
    config_dir().join("var/log/panes")
}

/// `~/.daemoneye/bin/` — symlinks/wrappers for the compiled agent and scripts.
pub fn bin_dir() -> PathBuf {
    config_dir().join("bin")
}

/// Default path for the daemon log file: `~/.daemoneye/var/log/daemon.log`.
pub fn default_log_path() -> PathBuf {
    var_log_dir().join("daemon.log")
}

/// Default path for the Unix domain socket: `~/.daemoneye/var/run/daemoneye.sock`.
///
/// Using the user's home directory rather than `/tmp` prevents other local users
/// from pre-creating a symlink or connecting to the socket.
pub fn default_socket_path() -> PathBuf {
    var_run_dir().join("daemoneye.sock")
}

/// Default path for the instance lock / PID file:
/// `~/.daemoneye/var/run/daemoneye.pid`.
pub fn default_pid_path() -> PathBuf {
    var_run_dir().join("daemoneye.pid")
}

/// Path for the structured event log: `~/.daemoneye/var/log/events.jsonl`.
pub fn events_path() -> PathBuf {
    var_log_dir().join("events.jsonl")
}

/// Directory holding dated event segments (`events-YYYYMMDD.jsonl`).
pub fn events_dir() -> PathBuf {
    var_log_dir().join("events")
}

/// The segment file that `log_event` writes to right now (today, UTC).
pub fn current_event_segment_path() -> PathBuf {
    events_dir().join(format!(
        "events-{}.jsonl",
        chrono::Utc::now().format("%Y%m%d")
    ))
}

/// Directory where user prompt TOML files are stored: `~/.daemoneye/etc/prompts/`.
pub fn prompts_dir() -> PathBuf {
    etc_dir().join("prompts")
}

/// Directory where per-session JSONL history files are stored: `~/.daemoneye/var/log/sessions/`.
pub fn sessions_dir() -> PathBuf {
    var_log_dir().join("sessions")
}

/// Resolves the user's home directory from the `HOME` env var.
/// Falls back to `/tmp` on systems where HOME is unset (unusual but possible).
fn dirs_next() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

impl Config {
    /// Resolve a named model entry.  `name = None` resolves the `"default"` model.
    /// Falls back to `"default"` if the named key is absent, then to any first
    /// entry.  Panics only if the models map is completely empty (should never
    /// happen with `Default::default()`).
    pub fn resolve_model(&self, name: Option<&str>) -> &ModelEntry {
        let key = name.unwrap_or("default");
        self.models
            .get(key)
            .or_else(|| self.models.get("default"))
            .or_else(|| self.models.values().next())
            // INVARIANT: Config::load() validates that at least one model entry is present
            .expect("models map must not be empty")
    }

    /// Return a sorted list of all configured model names.
    pub fn available_models(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.models.keys().map(|s| s.as_str()).collect();
        keys.sort();
        keys
    }

    /// Load configuration from `~/.daemoneye/etc/config.toml`.
    /// Returns `Config::default()` if the file does not exist yet.
    pub fn load() -> Result<Self> {
        let path = etc_dir().join("config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate_pricing();
        cfg.validate_compaction();
        Ok(cfg)
    }

    /// Walk `[models.*]` at startup and log `warn!` for any model where
    /// pricing cannot be resolved (unknown model on a non-local provider).
    /// Called from `Config::load()` — emits at most one warning per model.
    pub fn validate_pricing(&self) {
        for (name, entry) in &self.models {
            if entry.provider == "ollama" || entry.provider == "lmstudio" {
                continue;
            }
            if entry.pricing().is_none() {
                log::warn!(
                    "[pricing] model '{}' (provider='{}', model='{}') has no known pricing — \
                     cost accounting will report $0 for this model. \
                     Set input_cost_per_mtok / output_cost_per_mtok in config.toml to fix.",
                    name,
                    entry.provider,
                    entry.model
                );
            }
        }
    }

    /// Validate compaction thresholds. Warns if `target_pct >= compact_at_pct`
    /// (hysteresis would be lost) and falls back to defaults for the pair so the
    /// compactor always frees a positive margin.
    pub fn validate_compaction(&mut self) {
        if self.compaction.target_pct >= self.compaction.compact_at_pct {
            let defaults = CompactionConfig::default();
            log::warn!(
                "[compaction] target_pct ({}) >= compact_at_pct ({}): hysteresis is lost. \
                 Falling back to defaults target_pct={}, compact_at_pct={}.",
                self.compaction.target_pct,
                self.compaction.compact_at_pct,
                defaults.target_pct,
                defaults.compact_at_pct,
            );
            self.compaction.target_pct = defaults.target_pct;
            self.compaction.compact_at_pct = defaults.compact_at_pct;
        }
    }

    /// Return the path to the scripts directory: `~/.daemoneye/scripts/`.
    pub fn scripts_dir() -> PathBuf {
        config_dir().join("scripts")
    }

    /// Return the path to the runbooks directory: `~/.daemoneye/runbooks/`.
    pub fn runbooks_dir() -> PathBuf {
        config_dir().join("runbooks")
    }

    /// Return the path to the schedules JSON store: `~/.daemoneye/var/run/schedules.json`.
    pub fn schedules_path() -> PathBuf {
        var_run_dir().join("schedules.json")
    }
}

/// Load a named prompt from ~/.daemoneye/prompts/<name>.toml.
/// Falls back to the built-in SRE prompt for "sre", then to the minimal default.
pub fn load_named_prompt(name: &str) -> PromptDef {
    // First try the file on disk.
    let path = prompts_dir().join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(def) = toml::from_str::<PromptDef>(&text)
    {
        return def;
    }
    // Fall back to the compiled-in SRE prompt.
    if name == "sre"
        && let Ok(def) = toml::from_str::<PromptDef>(SRE_PROMPT_TOML)
    {
        return def;
    }
    PromptDef::builtin_minimal()
}
