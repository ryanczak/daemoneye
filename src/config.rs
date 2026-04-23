use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level configuration loaded from `~/.daemoneye/etc/config.toml`.
/// All sections default to sensible values so the file is optional.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub ai: AiConfig,
    /// Named model configurations.  At minimum a `[models.default]` entry should
    /// be present; it is used when no session-level override is active.
    #[serde(default = "default_models")]
    pub models: std::collections::HashMap<String, ModelEntry>,
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
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub digest: DigestConfig,
    #[serde(default)]
    pub approvals: ApprovalsConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ai: AiConfig::default(),
            models: default_models(),
            masking: MaskingConfig::default(),
            context: ContextConfig::default(),
            notifications: NotificationsConfig::default(),
            webhook: WebhookConfig::default(),
            ghost: GhostDaemonConfig::default(),
            daemon: DaemonConfig::default(),
            digest: DigestConfig::default(),
            approvals: ApprovalsConfig::default(),
            limits: LimitsConfig::default(),
        }
    }
}

/// Session-compaction digest configuration.
///
/// The structured digest (event tallies + artifact scans) always runs when
/// token pressure crosses the digest threshold.  The optional *narrative*
/// step calls a cheap AI model to turn the about-to-be-dropped turns into a
/// short natural-language summary; it is off by default because it costs an
/// extra API call per compaction.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct DigestConfig {
    /// When true, each digest pass calls the `[models.digest]` entry (falling
    /// back to `[models.default]`) to generate a narrative summary of the
    /// compacted turns; the narrative is prepended to the structured tally.
    /// Default: false.  Enable when you want richer post-compaction context
    /// and are willing to pay for one additional small-model call per digest.
    #[serde(default)]
    pub narrative_enabled: bool,
}

/// Default approval state for each action class at the start of every chat session.
///
/// All defaults preserve current behaviour — only `commands` starts as `true` because
/// non-sudo commands are bounded by OS permissions and require no additional trust grant.
/// Set any field to `true` to skip the per-call approval prompt for that class from the
/// moment a new session opens.  Individual approvals can always be revoked at runtime
/// with `/approvals revoke [class]`; `revoke` always gates everything regardless of
/// these defaults.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApprovalsConfig {
    /// Non-sudo terminal commands auto-approve at session start.
    /// Default: `true` — non-sudo commands run as the daemon user and are bounded
    /// by OS file permissions, the same trust model used by ghost shells.
    #[serde(default = "default_true")]
    pub commands: bool,
    /// Sudo terminal commands auto-approve at session start.  Default: `false`.
    #[serde(default)]
    pub sudo: bool,
    /// All `write_script` calls auto-approve at session start.  Default: `false`.
    #[serde(default)]
    pub scripts: bool,
    /// All `write_runbook` calls auto-approve at session start.  Default: `false`.
    #[serde(default)]
    pub runbooks: bool,
    /// All `edit_file` calls auto-approve at session start.  Default: `false`.
    #[serde(default)]
    pub file_edits: bool,
    /// Ghost shells: allow non-sudo commands without requiring the script to be
    /// listed in `auto_approve_scripts`.  Can also be set per-runbook via the
    /// `auto_approve_commands: true` frontmatter field.  Default: `false`.
    #[serde(default)]
    pub ghost_commands: bool,
}

impl Default for ApprovalsConfig {
    fn default() -> Self {
        Self {
            commands: true,
            sudo: false,
            scripts: false,
            runbooks: false,
            file_edits: false,
            ghost_commands: false,
        }
    }
}

/// Daemon startup and session management configuration.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DaemonConfig {
    /// Tmux session name the daemon creates or adopts at startup.
    ///
    /// Used when the daemon is launched outside of tmux (e.g. as a systemd service).
    /// If the named session already exists it is adopted; if not, the daemon creates
    /// it with `tmux new-session -d -s <name>` so ghost shells, scheduled jobs, and
    /// webhook-triggered automation are available immediately.
    ///
    /// When the daemon is launched from *inside* an active tmux session, it adopts
    /// that session directly and this setting is ignored.
    ///
    /// Default: `"daemoneye"`.
    #[serde(default = "default_tmux_session")]
    pub tmux_session: String,
}

fn default_tmux_session() -> String {
    "daemoneye".to_string()
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            tmux_session: default_tmux_session(),
        }
    }
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

/// Daemon-wide caps on tool call frequency and result size.
/// All limits default to the values previously baked into the source.
/// Set any field to `0` to remove that cap entirely.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LimitsConfig {
    /// Maximum times any single no-approval tool (e.g. `read_file`,
    /// `search_repository`) may be called within one assistant turn.
    /// Approval-gated tools (`run_terminal_command`, `edit_file`, etc.) are
    /// always exempt — the user's per-call approval prompt is their gate.
    /// Default: 100.  Set to 0 for no cap.
    #[serde(default = "default_per_tool_batch")]
    pub per_tool_batch: u32,

    /// Maximum total tool calls (across all non-approval-gated tools) the AI
    /// may make in a single assistant turn.
    /// Approval-gated tools are always exempt, same as for `per_tool_batch`.
    /// Default: 0 (no cap).
    #[serde(default)]
    pub total_tool_calls_per_turn: u32,

    /// Maximum characters stored for each tool result in the conversation
    /// history.  The full result is still streamed live to the AI; only the
    /// copy kept in message history is capped to limit context bloat.
    /// Default: 16000.  Set to 0 for no cap.
    #[serde(default = "default_tool_result_chars")]
    pub tool_result_chars: usize,

    /// Maximum messages retained per session in memory and on disk.
    /// When the session reaches this length, the digest compaction pass runs
    /// regardless of token pressure.
    /// Setting to 0 makes history unbounded but does NOT disable the digest —
    /// compaction still fires on token pressure alone.
    /// Default: 80.  Set to 0 for unbounded history.
    #[serde(default = "default_max_history")]
    pub max_history: usize,

    /// Maximum AI turns allowed per interactive chat session.
    /// Ghost shells use `[ghost] max_ghost_turns` instead — this field has
    /// no effect on ghost sessions.
    /// Default: 0 (no cap).
    #[serde(default)]
    pub max_turns: usize,

    /// Maximum cumulative tool calls across all turns in a single session.
    /// Default: 0 (no cap).
    #[serde(default)]
    pub max_tool_calls_per_session: usize,

    /// Per-tool overrides for `per_tool_batch`.  Named entries win over the
    /// global value for that tool only.  Approval-gated tools are always
    /// exempt; any entry for them emits a warning at config load.
    /// Example: `read_file = 200` raises the cap for that tool only.
    #[serde(default)]
    pub per_tool: std::collections::HashMap<String, u32>,
}

fn default_per_tool_batch() -> u32 {
    100
}
fn default_tool_result_chars() -> usize {
    16_000
}
fn default_max_history() -> usize {
    80
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            per_tool_batch: default_per_tool_batch(),
            total_tool_calls_per_turn: 0,
            tool_result_chars: default_tool_result_chars(),
            max_history: default_max_history(),
            max_turns: 0,
            max_tool_calls_per_session: 0,
            per_tool: std::collections::HashMap::new(),
        }
    }
}

impl LimitsConfig {
    /// Translates the `0 = unlimited` sentinel to `Option`.
    /// Returns `None` (uncapped) when value is 0, `Some(n)` otherwise.
    /// All u32 limit enforcement code should call this rather than comparing to 0 directly.
    pub fn cap_u32(value: u32) -> Option<u32> {
        if value == 0 { None } else { Some(value) }
    }

    /// Same sentinel translation for usize limits (history, turns, session totals).
    pub fn cap_usize(value: usize) -> Option<usize> {
        if value == 0 { None } else { Some(value) }
    }

    /// Effective per-turn batch cap for `tool_name`, applying any per-tool override.
    /// Returns `None` if uncapped.  Callers must check whether the tool is
    /// approval-gated before consulting this — approval-gated tools are always exempt.
    pub fn per_tool_cap(&self, tool_name: &str) -> Option<u32> {
        let raw = self
            .per_tool
            .get(tool_name)
            .copied()
            .unwrap_or(self.per_tool_batch);
        Self::cap_u32(raw)
    }

    /// Emit warnings for configuration that is likely unintentional.
    /// Call once at daemon startup after the config is loaded.
    pub fn validate(&self, digest: &DigestConfig) {
        // These tools are approval-gated: per_tool entries for them are silently
        // ignored at runtime, so surface the misconfiguration early.
        // Keep in sync with per_tool_limit() in src/daemon/server.rs.
        const APPROVAL_GATED: &[&str] = &[
            "run_terminal_command",
            "edit_file",
            "write_script",
            "write_runbook",
            "schedule_command",
            "spawn_ghost_shell",
            "delete_script",
            "delete_runbook",
            "delete_schedule",
        ];
        for tool in APPROVAL_GATED {
            if self.per_tool.contains_key(*tool) {
                log::warn!(
                    "[limits] per_tool.{tool} is set but {tool} is approval-gated and \
                     exempt from per-tool caps — this entry has no effect"
                );
            }
        }

        // Warn about the footgun: unbounded history with no narrative digest means
        // very long sessions accumulate context with no compaction of dropped turns.
        if self.max_history == 0 && !digest.narrative_enabled {
            log::warn!(
                "[limits] max_history = 0 (unbounded) and digest.narrative_enabled = false: \
                 long sessions will not compact narrative context. Consider enabling \
                 digest.narrative_enabled or setting a max_history ceiling."
            );
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

/// Per-model AI provider configuration.  Define one or more named entries in
/// `config.toml` under `[models.<name>]`.  A `[models.default]` entry is
/// required; it is used when no model override is in effect.
///
/// Example:
/// ```toml
/// [models.default]
/// provider = "anthropic"
/// model    = "claude-sonnet-4-6"
///
/// [models.opus]
/// provider = "anthropic"
/// model    = "claude-opus-4-6"
///
/// [models.local]
/// provider = "ollama"
/// model    = "llama3:70b"
/// base_url = "http://localhost:11434/v1"
/// context_window_tokens = 8192
/// ```
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelEntry {
    /// "anthropic" | "openai" | "gemini" | "ollama" | "lmstudio"
    #[serde(default = "default_provider")]
    pub provider: String,
    /// API key.  Empty → resolved from the provider's environment variable
    /// (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`).
    #[serde(default)]
    pub api_key: String,
    /// Model identifier passed to the API (e.g. `"claude-sonnet-4-6"`,
    /// `"gpt-4o"`, `"gemini-2.5-pro"`, `"llama3:70b"`).
    #[serde(default = "default_model")]
    pub model: String,
    /// Override the API base URL.  Useful for custom Ollama/LMStudio hosts or
    /// any OpenAI-compatible proxy.
    /// Defaults: ollama → http://localhost:11434/v1,
    ///           lmstudio → http://localhost:1234/v1,
    ///           openai → https://api.openai.com/v1 (or $OPENAI_API_BASE).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Override the model's context-window size in tokens.
    /// Set this for local models where the automatic lookup is wrong.
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
}

fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}

impl Default for ModelEntry {
    fn default() -> Self {
        ModelEntry {
            provider: default_provider(),
            api_key: String::new(),
            model: default_model(),
            base_url: None,
            context_window_tokens: None,
        }
    }
}

impl ModelEntry {
    /// The environment variable name that holds the API key for this provider.
    pub fn api_key_env_var(&self) -> &'static str {
        match self.provider.as_str() {
            "openai" => "OPENAI_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            "ollama" | "lmstudio" => "",
            _ => "ANTHROPIC_API_KEY",
        }
    }

    /// Resolve the API key: explicit config value → env var → dummy for local providers.
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

    /// Resolve the effective API base URL.
    /// Priority: explicit `base_url` → provider default → $OPENAI_API_BASE (openai only).
    pub fn effective_base_url(&self) -> String {
        if let Some(ref u) = self.base_url {
            return u.clone();
        }
        match self.provider.as_str() {
            "ollama" => "http://localhost:11434/v1".to_string(),
            "lmstudio" => "http://localhost:1234/v1".to_string(),
            "openai" => std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            _ => String::new(),
        }
    }

    /// Context-window size in tokens.  `context_window_tokens` wins; otherwise
    /// a built-in table is consulted.  Local/unknown models default to 32 768.
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
            32_768
        }
    }
}

fn default_models() -> std::collections::HashMap<String, ModelEntry> {
    let mut m = std::collections::HashMap::new();
    m.insert("default".to_string(), ModelEntry::default());
    m
}

/// Global AI settings from the `[ai]` section of `config.toml`.
/// Provider and model configuration has moved to `[models.<name>]` entries.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AiConfig {
    /// Name of a prompt file in `~/.daemoneye/prompts/` (without `.toml`).
    /// Defaults to `"sre"`.
    #[serde(default = "default_prompt")]
    pub prompt: String,
}

fn default_prompt() -> String {
    "sre".to_string()
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            prompt: default_prompt(),
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

/// `~/.daemoneye/lib/` — shared SDK modules (de_sdk, Python helpers, etc.).
pub fn lib_dir() -> PathBuf {
    config_dir().join("lib")
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

/// Path for the structured event log: `~/.daemoneye/var/log/events.jsonl`.
pub fn events_path() -> PathBuf {
    var_log_dir().join("events.jsonl")
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

    /// Return the path to the schedules JSON store: `~/.daemoneye/var/run/schedules.json`.
    pub fn schedules_path() -> PathBuf {
        var_run_dir().join("schedules.json")
    }

    /// Ensure the config directory tree and default files exist.
    pub fn ensure_dirs() -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        // FHS-inspired subtree
        std::fs::create_dir_all(etc_dir())?;
        std::fs::create_dir_all(var_run_dir())?;
        std::fs::create_dir_all(var_log_dir())?;
        std::fs::create_dir_all(pipe_log_dir())?;
        std::fs::create_dir_all(pane_logs_dir())?;
        std::fs::create_dir_all(bin_dir())?;
        std::fs::create_dir_all(lib_dir())?;
        // Daemon-managed persistent data
        let pd = prompts_dir();
        std::fs::create_dir_all(&pd)?;
        std::fs::create_dir_all(sessions_dir())?;
        // User-managed top-level directories
        std::fs::create_dir_all(Self::scripts_dir())?;
        std::fs::create_dir_all(Self::runbooks_dir())?;

        let cfg_path = etc_dir().join("config.toml");
        if !cfg_path.exists() {
            std::fs::write(&cfg_path, include_str!("../assets/etc/config.toml"))?;
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

        // Seed built-in session memories if they don't already exist.
        seed_session_memory(
            "pane-referencing-convention",
            PANE_REFERENCING_CONVENTION_MEMORY,
        )?;
        seed_session_memory("unicode-decoration-pref", UNICODE_DECORATION_PREF_MEMORY)?;

        Ok(())
    }
}

/// Write a knowledge memory file only if it does not already exist.
fn seed_knowledge_memory(key: &str, content: &str) -> Result<()> {
    seed_memory_inner("knowledge", key, content, false)
}

/// Write a session memory file only if it does not already exist.
fn seed_session_memory(key: &str, content: &str) -> Result<()> {
    seed_memory_inner("session", key, content, false)
}

/// Write a memory file into the given subdirectory, optionally overwriting.
fn seed_memory_inner(subdir: &str, key: &str, content: &str, force: bool) -> Result<()> {
    let dir = config_dir().join("memory").join(subdir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.md", key));
    if force || !path.exists() {
        std::fs::write(&path, content)
            .with_context(|| format!("seeding {} memory '{}'", subdir, key))?;
    }
    Ok(())
}

/// Re-seed all built-in memory files (knowledge + session), overwriting existing ones.
/// Called by `daemoneye setup --overwrite-memory`.
pub fn overwrite_knowledge_memories() -> Result<()> {
    seed_memory_inner("knowledge", "webhook-setup", WEBHOOK_SETUP_MEMORY, true)?;
    seed_memory_inner("knowledge", "runbook-format", RUNBOOK_FORMAT_MEMORY, true)?;
    seed_memory_inner(
        "knowledge",
        "runbook-ghost-template",
        RUNBOOK_GHOST_TEMPLATE_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "ghost-shell-guide",
        GHOST_SHELL_GUIDE_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "scheduling-guide",
        SCHEDULING_GUIDE_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "scripts-and-sudoers",
        SCRIPTS_AND_SUDOERS_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "session",
        "pane-referencing-convention",
        PANE_REFERENCING_CONVENTION_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "session",
        "unicode-decoration-pref",
        UNICODE_DECORATION_PREF_MEMORY,
        true,
    )?;
    Ok(())
}

/// Overwrite the built-in SRE prompt regardless of whether it already exists.
/// Called by `daemoneye setup --overwrite-all`.
pub fn overwrite_sre_prompt() -> Result<()> {
    let sre_path = prompts_dir().join("sre.toml");
    std::fs::write(&sre_path, SRE_PROMPT_TOML)
        .with_context(|| format!("overwriting SRE prompt at {}", sre_path.display()))
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

// ---------------------------------------------------------------------------
// Built-in SRE prompt (also written to ~/.daemoneye/prompts/sre.toml on startup)
// ---------------------------------------------------------------------------

const SRE_PROMPT_TOML: &str = include_str!("../assets/prompts/sre.toml");

// ---------------------------------------------------------------------------
// Seeded knowledge memories (written to ~/.daemoneye/memory/knowledge/ on first run)
// ---------------------------------------------------------------------------

const WEBHOOK_SETUP_MEMORY: &str = include_str!("../assets/memory/knowledge/webhook-setup.md");
const RUNBOOK_FORMAT_MEMORY: &str = include_str!("../assets/memory/knowledge/runbook-format.md");
const RUNBOOK_GHOST_TEMPLATE_MEMORY: &str =
    include_str!("../assets/memory/knowledge/runbook-ghost-template.md");
const GHOST_SHELL_GUIDE_MEMORY: &str =
    include_str!("../assets/memory/knowledge/ghost-shell-guide.md");
const SCHEDULING_GUIDE_MEMORY: &str =
    include_str!("../assets/memory/knowledge/scheduling-guide.md");
const SCRIPTS_AND_SUDOERS_MEMORY: &str =
    include_str!("../assets/memory/knowledge/scripts-and-sudoers.md");

// ---------------------------------------------------------------------------
// Seeded session memories (written to ~/.daemoneye/memory/session/ on first run)
// ---------------------------------------------------------------------------

const PANE_REFERENCING_CONVENTION_MEMORY: &str =
    include_str!("../assets/memory/session/pane-referencing-convention.md");
const UNICODE_DECORATION_PREF_MEMORY: &str =
    include_str!("../assets/memory/session/unicode-decoration-pref.md");

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default values ───────────────────────────────────────────────────────

    #[test]
    fn default_config_has_default_model() {
        let cfg = Config::default();
        let entry = cfg.resolve_model(None);
        assert_eq!(entry.provider, "anthropic");
        assert_eq!(entry.model, "claude-sonnet-4-6");
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
    fn parse_models_section() {
        let toml = r#"
            [models.default]
            provider = "openai"
            model    = "gpt-4o"

            [models.big]
            provider = "anthropic"
            model    = "claude-opus-4-6"

            [ai]
            prompt = "custom"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let def = cfg.resolve_model(None);
        assert_eq!(def.provider, "openai");
        assert_eq!(def.model, "gpt-4o");
        let big = cfg.resolve_model(Some("big"));
        assert_eq!(big.model, "claude-opus-4-6");
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
        let entry = cfg.resolve_model(None);
        assert_eq!(entry.provider, "anthropic");
        assert_eq!(cfg.context.environment, "personal");
        assert!(cfg.masking.extra_patterns.is_empty());
    }

    #[test]
    fn resolve_model_unknown_name_falls_back_to_default() {
        let cfg = Config::default();
        let entry = cfg.resolve_model(Some("nonexistent"));
        assert_eq!(entry.provider, "anthropic");
    }

    #[test]
    fn available_models_returns_sorted_keys() {
        let toml = r#"
            [models.default]
            provider = "anthropic"
            model    = "claude-sonnet-4-6"
            [models.opus]
            provider = "anthropic"
            model    = "claude-opus-4-6"
            [models.local]
            provider = "ollama"
            model    = "llama3.2"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let names = cfg.available_models();
        assert_eq!(names, vec!["default", "local", "opus"]);
    }

    // ── ModelEntry methods ───────────────────────────────────────────────────

    #[test]
    fn model_entry_context_window_claude() {
        let entry = ModelEntry {
            model: "claude-sonnet-4-6".to_string(),
            ..ModelEntry::default()
        };
        assert_eq!(entry.context_window(), 200_000);
    }

    #[test]
    fn model_entry_context_window_override() {
        let entry = ModelEntry {
            context_window_tokens: Some(8192),
            ..ModelEntry::default()
        };
        assert_eq!(entry.context_window(), 8192);
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

    // ── ApprovalsConfig ──────────────────────────────────────────────────────

    #[test]
    fn default_approvals_match_current_behavior() {
        let cfg = ApprovalsConfig::default();
        assert!(
            cfg.commands,
            "non-sudo commands must default to auto-approved"
        );
        assert!(!cfg.sudo);
        assert!(!cfg.scripts);
        assert!(!cfg.runbooks);
        assert!(!cfg.file_edits);
        assert!(!cfg.ghost_commands);
    }

    #[test]
    fn approvals_config_parses_all_fields() {
        let toml = r#"
            [approvals]
            commands      = true
            sudo          = true
            scripts       = true
            runbooks      = true
            file_edits    = true
            ghost_commands = true
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.approvals.commands);
        assert!(cfg.approvals.sudo);
        assert!(cfg.approvals.scripts);
        assert!(cfg.approvals.runbooks);
        assert!(cfg.approvals.file_edits);
        assert!(cfg.approvals.ghost_commands);
    }

    #[test]
    fn missing_approvals_section_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.approvals.commands);
        assert!(!cfg.approvals.sudo);
        assert!(!cfg.approvals.scripts);
        assert!(!cfg.approvals.runbooks);
        assert!(!cfg.approvals.file_edits);
        assert!(!cfg.approvals.ghost_commands);
    }

    #[test]
    fn partial_approvals_section_fills_remaining_defaults() {
        let toml = r#"
            [approvals]
            sudo = true
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(
            cfg.approvals.commands,
            "commands must still default to true"
        );
        assert!(cfg.approvals.sudo);
        assert!(!cfg.approvals.scripts);
        assert!(!cfg.approvals.ghost_commands);
    }

    // ── LimitsConfig ─────────────────────────────────────────────────────────

    #[test]
    fn default_limits_match_current_hardcoded_constants() {
        let limits = LimitsConfig::default();
        assert_eq!(
            limits.per_tool_batch, 100,
            "must match MAX_SAME_TOOL_BATCH in server.rs"
        );
        assert_eq!(
            limits.tool_result_chars, 16_000,
            "must match MAX_TOOL_RESULT_CHARS in server.rs"
        );
        assert_eq!(
            limits.max_history, 80,
            "must match MAX_HISTORY in session.rs"
        );
        assert_eq!(
            limits.total_tool_calls_per_turn, 0,
            "new field defaults to uncapped"
        );
        assert_eq!(limits.max_turns, 0, "new field defaults to uncapped");
        assert_eq!(
            limits.max_tool_calls_per_session, 0,
            "new field defaults to uncapped"
        );
        assert!(limits.per_tool.is_empty());
    }

    #[test]
    fn missing_limits_section_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.limits.per_tool_batch, 100);
        assert_eq!(cfg.limits.tool_result_chars, 16_000);
        assert_eq!(cfg.limits.max_history, 80);
        assert_eq!(cfg.limits.total_tool_calls_per_turn, 0);
        assert_eq!(cfg.limits.max_turns, 0);
        assert_eq!(cfg.limits.max_tool_calls_per_session, 0);
    }

    #[test]
    fn limits_section_parses_all_fields() {
        let toml = r#"
            [limits]
            per_tool_batch            = 200
            total_tool_calls_per_turn = 50
            tool_result_chars         = 8000
            max_history               = 40
            max_turns                 = 100
            max_tool_calls_per_session = 500

            [limits.per_tool]
            read_file         = 300
            search_repository = 25
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let l = &cfg.limits;
        assert_eq!(l.per_tool_batch, 200);
        assert_eq!(l.total_tool_calls_per_turn, 50);
        assert_eq!(l.tool_result_chars, 8000);
        assert_eq!(l.max_history, 40);
        assert_eq!(l.max_turns, 100);
        assert_eq!(l.max_tool_calls_per_session, 500);
        assert_eq!(l.per_tool.get("read_file").copied(), Some(300));
        assert_eq!(l.per_tool.get("search_repository").copied(), Some(25));
    }

    #[test]
    fn partial_limits_section_fills_remaining_defaults() {
        let toml = r#"
            [limits]
            max_history = 40
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.limits.max_history, 40);
        assert_eq!(cfg.limits.per_tool_batch, 100, "should still default");
        assert_eq!(cfg.limits.tool_result_chars, 16_000, "should still default");
    }

    #[test]
    fn limits_zero_means_uncapped() {
        let toml = r#"
            [limits]
            per_tool_batch    = 0
            tool_result_chars = 0
            max_history       = 0
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(LimitsConfig::cap_u32(cfg.limits.per_tool_batch).is_none());
        assert!(LimitsConfig::cap_usize(cfg.limits.tool_result_chars).is_none());
        assert!(LimitsConfig::cap_usize(cfg.limits.max_history).is_none());
    }

    #[test]
    fn cap_u32_sentinel() {
        assert_eq!(LimitsConfig::cap_u32(0), None);
        assert_eq!(LimitsConfig::cap_u32(1), Some(1));
        assert_eq!(LimitsConfig::cap_u32(100), Some(100));
    }

    #[test]
    fn cap_usize_sentinel() {
        assert_eq!(LimitsConfig::cap_usize(0), None);
        assert_eq!(LimitsConfig::cap_usize(1), Some(1));
        assert_eq!(LimitsConfig::cap_usize(80), Some(80));
    }

    #[test]
    fn per_tool_cap_uses_override_over_global() {
        let mut limits = LimitsConfig::default(); // per_tool_batch = 100
        limits.per_tool.insert("read_file".to_string(), 200);
        assert_eq!(limits.per_tool_cap("read_file"), Some(200));
        assert_eq!(limits.per_tool_cap("search_repository"), Some(100)); // falls back to global
    }

    #[test]
    fn per_tool_cap_zero_override_means_uncapped() {
        let mut limits = LimitsConfig::default();
        limits.per_tool.insert("read_file".to_string(), 0);
        assert_eq!(limits.per_tool_cap("read_file"), None);
    }

    #[test]
    fn per_tool_cap_zero_global_means_all_uncapped() {
        let mut limits = LimitsConfig::default();
        limits.per_tool_batch = 0;
        assert_eq!(limits.per_tool_cap("read_file"), None);
        assert_eq!(limits.per_tool_cap("get_terminal_context"), None);
    }

    #[test]
    fn validate_approval_gated_per_tool_entry_does_not_panic() {
        // The validate() call should warn (via log::warn!) but never panic.
        // Verify the condition that triggers the warning: an approval-gated tool
        // appearing in per_tool. The warning is observable in daemon.log at runtime.
        let mut limits = LimitsConfig::default();
        limits
            .per_tool
            .insert("run_terminal_command".to_string(), 5);
        assert!(
            limits.per_tool.contains_key("run_terminal_command"),
            "precondition: entry must be present to trigger warning path"
        );
        let digest = DigestConfig::default();
        limits.validate(&digest); // must not panic
    }

    #[test]
    fn validate_unbounded_history_no_narrative_does_not_panic() {
        // The validate() call should warn but never panic when max_history = 0
        // and digest.narrative_enabled = false (the footgun combo).
        let mut limits = LimitsConfig::default();
        limits.max_history = 0;
        let digest = DigestConfig {
            narrative_enabled: false,
            ..DigestConfig::default()
        };
        limits.validate(&digest); // must not panic
    }

    #[test]
    fn validate_narrative_enabled_suppresses_footgun_warning() {
        // No warning should fire (or panic) when narrative_enabled is true, even
        // with max_history = 0, because the narrative step provides compaction.
        let mut limits = LimitsConfig::default();
        limits.max_history = 0;
        let digest = DigestConfig {
            narrative_enabled: true,
            ..DigestConfig::default()
        };
        limits.validate(&digest); // must not panic
    }

    #[test]
    fn config_migration_old_toml_without_limits_section_matches_constants() {
        // A config.toml that predates [limits] must parse cleanly and produce
        // exactly the same numeric constants that were previously hardcoded.
        let old_config = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"
        "#;
        let cfg: Config = toml::from_str(old_config).unwrap();
        assert_eq!(
            cfg.limits.per_tool_batch, 100,
            "must match legacy MAX_SAME_TOOL_BATCH = 100"
        );
        assert_eq!(
            cfg.limits.tool_result_chars, 16_000,
            "must match legacy MAX_TOOL_RESULT_CHARS = 16_000"
        );
        assert_eq!(
            cfg.limits.max_history, 80,
            "must match legacy MAX_HISTORY = 80"
        );
        assert_eq!(
            cfg.limits.total_tool_calls_per_turn, 0,
            "new — default uncapped"
        );
        assert_eq!(cfg.limits.max_turns, 0, "new — default uncapped");
        assert_eq!(
            cfg.limits.max_tool_calls_per_session, 0,
            "new — default uncapped"
        );
        assert!(
            cfg.limits.per_tool.is_empty(),
            "new — no overrides by default"
        );
    }
}
