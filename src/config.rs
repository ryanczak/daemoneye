use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level configuration loaded from `~/.daemoneye/config.toml`.
/// All sections default to sensible values so the file is optional.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct Config {
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub masking: MaskingConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub ghost: GhostDaemonConfig,
}

/// Daemon-wide limits for autonomous Ghost Shells.
/// These are hard ceilings that individual runbooks cannot exceed.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GhostDaemonConfig {
    /// Hard upper limit on AI turns per ghost shell.
    /// Individual runbooks may set a lower value with `max_ghost_turns`
    /// but can never exceed this ceiling. Default: 20.
    #[serde(default = "default_max_ghost_turns")]
    pub max_ghost_turns: usize,
    /// Maximum number of ghost shells that may run concurrently.
    /// New ghost shells are dropped (with a warning) when this limit is reached.
    /// Set to 0 to disable the cap. Default: 3.
    #[serde(default = "default_max_concurrent_ghosts")]
    pub max_concurrent_ghosts: usize,
}

fn default_max_ghost_turns() -> usize {
    20
}

fn default_max_concurrent_ghosts() -> usize {
    3
}

impl Default for GhostDaemonConfig {
    fn default() -> Self {
        Self {
            max_ghost_turns: default_max_ghost_turns(),
            max_concurrent_ghosts: default_max_concurrent_ghosts(),
        }
    }
}

/// Notification hooks for scheduler/watchdog alerts.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct NotificationsConfig {
    /// Shell command to run when a watchdog alert fires.
    /// Available env vars: `$DAEMONEYE_JOB` (job name), `$DAEMONEYE_MSG` (alert message).
    /// Example: `notify-send '$DAEMONEYE_JOB' '$DAEMONEYE_MSG'`
    #[serde(default)]
    pub on_alert: String,
}

/// Webhook ingestion configuration.
/// When enabled, DaemonEye listens for HTTP POST alerts from Prometheus
/// Alertmanager, Grafana, or any generic JSON alerting tool.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WebhookConfig {
    /// Whether the webhook endpoint is active. Disabled by default.
    #[serde(default)]
    pub enabled: bool,
    /// TCP port to listen on. Default 9393.
    #[serde(default = "default_webhook_port")]
    pub port: u16,
    /// Bearer token for authentication. Empty = no auth required.
    #[serde(default)]
    pub secret: String,
    /// Run runbook-based AI analysis when a matching runbook is found.
    #[serde(default = "default_true")]
    pub auto_analyze: bool,
    /// Minimum severity to trigger AI analysis and fire_notification.
    /// "info" | "warning" | "critical"
    #[serde(default = "default_severity_threshold")]
    pub severity_threshold: String,
    /// Seconds to suppress duplicate alerts by fingerprint. Default 300.
    #[serde(default = "default_dedup_window")]
    pub dedup_window_secs: u64,
    /// IP address to bind the webhook listener to. Default "127.0.0.1" (localhost only).
    /// Set to "0.0.0.0" to accept connections from all interfaces.
    #[serde(default = "default_webhook_bind")]
    pub bind_addr: String,
}

fn default_webhook_port() -> u16 {
    9393
}
fn default_true() -> bool {
    true
}
fn default_severity_threshold() -> String {
    "warning".to_string()
}
fn default_dedup_window() -> u64 {
    300
}
fn default_webhook_bind() -> String {
    "127.0.0.1".to_string()
}

impl Default for WebhookConfig {
    fn default() -> Self {
        WebhookConfig {
            enabled: false,
            port: default_webhook_port(),
            secret: String::new(),
            auto_analyze: default_true(),
            severity_threshold: default_severity_threshold(),
            dedup_window_secs: default_dedup_window(),
            bind_addr: default_webhook_bind(),
        }
    }
}

/// Runtime environment declaration — tells the AI how to calibrate caution,
/// blast-radius assessment, and security posture.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContextConfig {
    /// One of: "personal", "development", "staging", "production".
    /// Defaults to "personal".
    #[serde(default = "default_environment")]
    pub environment: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            environment: default_environment(),
        }
    }
}

fn default_environment() -> String {
    "personal".to_string()
}

/// User-defined additions to the sensitive-data masking filter.
/// Built-in patterns always run; these are appended to the set.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct MaskingConfig {
    /// Additional regex patterns to redact before sending context to the AI.
    /// Each matching substring is replaced with `<REDACTED>`.
    /// Example: `["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]`
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

/// AI provider settings from the `[ai]` section of `config.toml`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AiConfig {
    /// "anthropic" | "openai" | "gemini" | "ollama" | "lmstudio"
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Name of a prompt file in ~/.daemoneye/prompts/ (without .toml extension).
    /// Defaults to "sre".
    #[serde(default = "default_prompt")]
    pub prompt: String,
    /// "bottom" | "left" | "right"
    #[serde(default = "default_position")]
    pub position: String,
    /// Override the API base URL. Useful for pointing at a custom Ollama host,
    /// a LMStudio instance on another machine, or any OpenAI-compatible proxy.
    /// Defaults: ollama → http://localhost:11434/v1,
    ///           lmstudio → http://localhost:1234/v1,
    ///           openai → https://api.openai.com/v1 (or $OPENAI_API_BASE).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Override the model's context-window size in tokens.
    /// Set this for local models where the automatic lookup is wrong.
    /// Example: 8192 for a 4-bit quantised 8 B model loaded with 8 k context.
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
}

fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}
fn default_prompt() -> String {
    "sre".to_string()
}
fn default_position() -> String {
    "bottom".to_string()
}

impl AiConfig {
    pub fn api_key_env_var(&self) -> &'static str {
        match self.provider.as_str() {
            "openai" => "OPENAI_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            "ollama" | "lmstudio" => "",
            _ => "ANTHROPIC_API_KEY",
        }
    }

    pub fn resolve_api_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        // Local providers don't require a real key — use a dummy so the OpenAI
        // client can still set the Authorization header without panicking.
        match self.provider.as_str() {
            "ollama" | "lmstudio" => return "local".to_string(),
            _ => {}
        }
        let env_var = self.api_key_env_var();
        if env_var.is_empty() {
            return String::new();
        }
        std::env::var(env_var).unwrap_or_default()
    }

    /// Resolve the effective API base URL for the configured provider.
    /// Priority: explicit `base_url` in config → provider default → env var fallback (openai).
    pub fn effective_base_url(&self) -> String {
        if let Some(ref u) = self.base_url {
            return u.clone();
        }
        match self.provider.as_str() {
            "ollama" => "http://localhost:11434/v1".to_string(),
            "lmstudio" => "http://localhost:1234/v1".to_string(),
            "openai" => std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            _ => String::new(), // anthropic / gemini don't use this
        }
    }

    /// Return the context-window size (in tokens) for the configured model.
    /// `context_window_tokens` in config always wins; otherwise a built-in table
    /// is consulted. Local models default to 32 768 (a conservative 32 k window).
    pub fn context_window(&self) -> u32 {
        if let Some(override_val) = self.context_window_tokens {
            return override_val;
        }
        let m = self.model.as_str();
        if m.starts_with("claude") {
            200_000
        } else if m.starts_with("gemini-1.5-pro") {
            2_000_000
        } else if m.starts_with("gemini") {
            1_000_000
        } else if m.starts_with("gpt-4o") || m.starts_with("gpt-4-turbo") {
            128_000
        } else if m.starts_with("gpt-3.5") {
            16_000
        } else {
            // Local / unknown models: conservative 32 k default.
            // Users should set context_window_tokens in config.toml.
            32_768
        }
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            provider: default_provider(),
            api_key: String::new(),
            model: default_model(),
            prompt: default_prompt(),
            position: default_position(),
            base_url: None,
            context_window_tokens: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt definitions
// ---------------------------------------------------------------------------

/// A loaded prompt definition (system message).
/// Loaded from `~/.daemoneye/prompts/<name>.toml` or falling back to built-ins.
#[derive(Debug, Deserialize, Clone)]
pub struct PromptDef {
    pub system: String,
}

impl PromptDef {
    /// Fallback used when no prompt file can be found.
    pub fn builtin_minimal() -> Self {
        PromptDef {
            system: "You are a helpful terminal assistant. \
                     When suggesting commands put each on its own line."
                .to_string(),
        }
    }
}

/// Returns `~/.daemoneye/` (or `/tmp/.daemoneye/` if HOME is unset).
pub fn config_dir() -> PathBuf {
    let mut p = dirs_next();
    p.push(".daemoneye");
    p
}

/// Default path for the daemon log file: `~/.daemoneye/daemon.log`.
pub fn default_log_path() -> PathBuf {
    config_dir().join("daemon.log")
}

/// Default path for the Unix domain socket: `~/.daemoneye/daemoneye.sock`.
///
/// Using the user's home directory rather than `/tmp` prevents other local users
/// from pre-creating a symlink or connecting to the socket.
pub fn default_socket_path() -> PathBuf {
    config_dir().join("daemoneye.sock")
}

/// Directory where user prompt TOML files are stored: `~/.daemoneye/prompts/`.
pub fn prompts_dir() -> PathBuf {
    let mut p = config_dir();
    p.push("prompts");
    p
}

/// Directory where per-session JSONL history files are stored: `~/.daemoneye/sessions/`.
pub fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

/// Resolves the user's home directory from the `HOME` env var.
/// Falls back to `/tmp` on systems where HOME is unset (unusual but possible).
fn dirs_next() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

impl Config {
    /// Load configuration from `~/.daemoneye/config.toml`.
    /// Returns `Config::default()` if the file does not exist yet.
    pub fn load() -> Result<Self> {
        let path = config_dir().join("config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// Return the path to the scripts directory: `~/.daemoneye/scripts/`.
    pub fn scripts_dir() -> PathBuf {
        config_dir().join("scripts")
    }

    /// Return the path to the runbooks directory: `~/.daemoneye/runbooks/`.
    pub fn runbooks_dir() -> PathBuf {
        config_dir().join("runbooks")
    }

    /// Return the path to the schedules JSON store: `~/.daemoneye/schedules.json`.
    pub fn schedules_path() -> PathBuf {
        config_dir().join("schedules.json")
    }

    /// Ensure the config directory, example config, and default prompt exist.
    pub fn ensure_dirs() -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let pd = prompts_dir();
        std::fs::create_dir_all(&pd)?;
        std::fs::create_dir_all(sessions_dir())?;
        std::fs::create_dir_all(Self::scripts_dir())?;
        std::fs::create_dir_all(Self::runbooks_dir())?;

        let cfg_path = dir.join("config.toml");
        if !cfg_path.exists() {
            std::fs::write(
                &cfg_path,
                r#"[ai]
provider = "anthropic"
api_key  = ""
model    = "claude-sonnet-4-6"
prompt   = "sre"

# --- Local LLM examples (no API key required) ---
# [ai]
# provider = "ollama"
# model    = "llama3.2"          # any model pulled via `ollama pull`
# # base_url = "http://localhost:11434/v1"  # default; change if Ollama runs elsewhere
# # context_window_tokens = 8192           # set if the model uses a non-32k context
#
# [ai]
# provider = "lmstudio"
# model    = "lmstudio-community/Meta-Llama-3-8B-Instruct-GGUF"
# # base_url = "http://localhost:1234/v1"   # default LM Studio port
# # context_window_tokens = 8192

# [context]
# Declare the operating environment so the AI calibrates its advice accordingly.
# Values: "personal" | "development" | "staging" | "production"
# environment = "personal"

# [masking]
# Add org-specific patterns to redact before context is sent to the AI.
# Built-in patterns (AWS keys, JWTs, DB URLs, private keys, etc.) always run.
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]

# [notifications]
# Shell command to run when a watchdog alert fires.
# Available env vars: $DAEMONEYE_JOB (job name), $DAEMONEYE_MSG (alert message).
# on_alert = "notify-send '$DAEMONEYE_JOB' '$DAEMONEYE_MSG'"

# [webhook]
# enabled = false
# port = 9393
# secret = ""            # Bearer token; empty = no auth
# auto_analyze = true
# severity_threshold = "warning"   # "info" | "warning" | "critical"
# dedup_window_secs = 300
# bind_addr = "127.0.0.1"          # "0.0.0.0" to accept from all interfaces
"#,
            )?;
        }

        // Write the built-in SRE prompt if it doesn't already exist.
        let sre_path = pd.join("sre.toml");
        if !sre_path.exists() {
            std::fs::write(&sre_path, SRE_PROMPT_TOML)?;
        }

        // Seed built-in knowledge memories if they don't already exist.
        // User edits are preserved — we only write on first run.
        seed_knowledge_memory("webhook-setup", WEBHOOK_SETUP_MEMORY)?;
        seed_knowledge_memory("runbook-format", RUNBOOK_FORMAT_MEMORY)?;
        seed_knowledge_memory("runbook-ghost-template", RUNBOOK_GHOST_TEMPLATE_MEMORY)?;
        seed_knowledge_memory("ghost-shell-guide", GHOST_SHELL_GUIDE_MEMORY)?;
        seed_knowledge_memory("scheduling-guide", SCHEDULING_GUIDE_MEMORY)?;
        seed_knowledge_memory("scripts-and-sudoers", SCRIPTS_AND_SUDOERS_MEMORY)?;

        Ok(())
    }
}

/// Write a knowledge memory file only if it does not already exist.
fn seed_knowledge_memory(key: &str, content: &str) -> Result<()> {
    let dir = config_dir().join("memory").join("knowledge");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.md", key));
    if !path.exists() {
        std::fs::write(&path, content)
            .with_context(|| format!("seeding knowledge memory '{}'", key))?;
    }
    Ok(())
}

/// Load a named prompt from ~/.daemoneye/prompts/<name>.toml.
/// Falls back to the built-in SRE prompt for "sre", then to the minimal default.
pub fn load_named_prompt(name: &str) -> PromptDef {
    // First try the file on disk.
    let path = prompts_dir().join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(def) = toml::from_str::<PromptDef>(&text) {
            return def;
        }
    }
    // Fall back to the compiled-in SRE prompt.
    if name == "sre" {
        if let Ok(def) = toml::from_str::<PromptDef>(SRE_PROMPT_TOML) {
            return def;
        }
    }
    PromptDef::builtin_minimal()
}

// ---------------------------------------------------------------------------
// Built-in SRE prompt (also written to ~/.daemoneye/prompts/sre.toml on startup)
// ---------------------------------------------------------------------------

const SRE_PROMPT_TOML: &str = include_str!("../assets/prompts/sre.toml");

// ---------------------------------------------------------------------------
// Seeded knowledge memories (written to ~/.daemoneye/memory/knowledge/ on first run)
// ---------------------------------------------------------------------------

const WEBHOOK_SETUP_MEMORY: &str = include_str!("../assets/memory/webhook-setup.md");
const RUNBOOK_FORMAT_MEMORY: &str = include_str!("../assets/memory/runbook-format.md");
const RUNBOOK_GHOST_TEMPLATE_MEMORY: &str = include_str!("../assets/memory/runbook-ghost-template.md");
const GHOST_SHELL_GUIDE_MEMORY: &str = include_str!("../assets/memory/ghost-shell-guide.md");
const SCHEDULING_GUIDE_MEMORY: &str = include_str!("../assets/memory/scheduling-guide.md");
const SCRIPTS_AND_SUDOERS_MEMORY: &str = include_str!("../assets/memory/scripts-and-sudoers.md");

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default values ───────────────────────────────────────────────────────

    #[test]
    fn default_config_ai_provider() {
        assert_eq!(Config::default().ai.provider, "anthropic");
    }

    #[test]
    fn default_config_ai_model() {
        assert_eq!(Config::default().ai.model, "claude-sonnet-4-6");
    }

    #[test]
    fn default_config_ai_prompt() {
        assert_eq!(Config::default().ai.prompt, "sre");
    }

    #[test]
    fn default_config_environment() {
        assert_eq!(Config::default().context.environment, "personal");
    }

    #[test]
    fn default_config_masking_empty() {
        assert!(Config::default().masking.extra_patterns.is_empty());
    }

    // ── TOML parsing ─────────────────────────────────────────────────────────

    #[test]
    fn parse_ai_section() {
        let toml = r#"
            [ai]
            provider = "openai"
            model    = "gpt-4o"
            prompt   = "custom"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.ai.provider, "openai");
        assert_eq!(cfg.ai.model, "gpt-4o");
        assert_eq!(cfg.ai.prompt, "custom");
    }

    #[test]
    fn parse_context_section() {
        let toml = r#"
            [context]
            environment = "production"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.context.environment, "production");
    }

    #[test]
    fn parse_masking_section() {
        let toml = r#"
            [masking]
            extra_patterns = ["MYCO-[A-Z0-9]{8}", "sk_live_\\w+"]
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.masking.extra_patterns.len(), 2);
        assert_eq!(cfg.masking.extra_patterns[0], "MYCO-[A-Z0-9]{8}");
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.ai.provider, "anthropic");
        assert_eq!(cfg.context.environment, "personal");
        assert!(cfg.masking.extra_patterns.is_empty());
    }

    #[test]
    fn partial_ai_section_fills_missing_fields() {
        // Only override provider — model, prompt, position stay at defaults.
        let toml = r#"
            [ai]
            provider = "gemini"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.ai.provider, "gemini");
        assert_eq!(cfg.ai.model, "claude-sonnet-4-6");
        assert_eq!(cfg.ai.prompt, "sre");
    }

    // ── Builtin prompt ───────────────────────────────────────────────────────

    #[test]
    fn builtin_sre_prompt_parses() {
        let def = toml::from_str::<PromptDef>(SRE_PROMPT_TOML);
        assert!(def.is_ok(), "SRE_PROMPT_TOML must be valid TOML");
        let def = def.unwrap();
        assert!(!def.system.is_empty());
    }

    #[test]
    fn builtin_minimal_prompt_is_nonempty() {
        let def = PromptDef::builtin_minimal();
        assert!(!def.system.is_empty());
    }

    // ── load_named_prompt fallback chain ─────────────────────────────────────

    #[test]
    fn load_sre_prompt_falls_back_to_builtin() {
        // "sre" should always succeed even without a file on disk (compiled-in fallback).
        let def = load_named_prompt("sre");
        assert!(!def.system.is_empty());
    }

    #[test]
    fn load_unknown_prompt_returns_minimal() {
        let def = load_named_prompt("__nonexistent_prompt_xyz__");
        assert!(!def.system.is_empty());
    }
}
