use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub sessions: SessionsConfig,
    #[serde(default)]
    pub events: EventsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
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
            compaction: CompactionConfig::default(),
            sessions: SessionsConfig::default(),
            events: EventsConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

/// Named session persistence configuration (`[sessions]` in config.toml).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SessionsConfig {
    /// Automatically propose a session name after this many turns.
    /// Set to 0 to disable auto-naming.  Default: 10.
    #[serde(default = "default_auto_name_turn_threshold")]
    pub auto_name_turn_threshold: usize,
    /// Enable the auto-naming suggestion.  Default: true.
    #[serde(default = "default_true")]
    pub auto_name_enabled: bool,
    /// Number of most-recent messages loaded when resuming a saved session.
    /// Set to 0 to load the complete history (may exceed context window).  Default: 10.
    #[serde(default = "default_load_recent_turns")]
    pub load_recent_turns: usize,
    /// Delete session archive files whose mtime is older than this many
    /// days. 0 = keep forever (default).
    #[serde(default)]
    pub archive_retention_days: u32,
}

fn default_auto_name_turn_threshold() -> usize {
    10
}

fn default_load_recent_turns() -> usize {
    10
}

/// Event log rotation and retention configuration (`[events]` in config.toml).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EventsConfig {
    /// Delete dated event segments older than this many days.
    /// 0 = keep forever. The legacy `var/events.jsonl` is never deleted.
    #[serde(default = "default_events_retention_days")]
    pub retention_days: u32,
}

fn default_events_retention_days() -> u32 {
    90
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            retention_days: default_events_retention_days(),
        }
    }
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            auto_name_turn_threshold: default_auto_name_turn_threshold(),
            auto_name_enabled: true,
            load_recent_turns: default_load_recent_turns(),
            archive_retention_days: 0,
        }
    }
}

/// Daemon log rotation configuration (`[logging]` in config.toml).
///
/// The live `var/log/daemon.log` reached 25.8 MB in ~12 weeks (May–Aug 2025).
/// A 5 MB bound with 5 kept rotations caps total disk usage at ~25 MB while
/// preserving enough history for debugging.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggingConfig {
    /// Rotate `daemon.log` when it exceeds this many bytes.
    /// Default: 5 MB (0x500000).
    #[serde(default = "default_log_max_bytes")]
    pub log_max_bytes: u64,
    /// Number of rotated files to keep (`daemon.log.1` … `daemon.log.N`).
    /// Default: 5.
    #[serde(default = "default_log_keep_count")]
    pub log_keep_count: u32,
}

/// 5 MB — bounds the observed 25.8 MB growth to a single rotation cycle.
fn default_log_max_bytes() -> u64 {
    5 * 1024 * 1024
}

/// Keep 5 rotated copies, giving ~25 MB total on disk at default size.
fn default_log_keep_count() -> u32 {
    5
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_max_bytes: default_log_max_bytes(),
            log_keep_count: default_log_keep_count(),
        }
    }
}

/// Session-compaction digest configuration.
///
/// The structured digest (event tallies + artifact scans) always runs when
/// token pressure crosses the digest threshold.  The optional *narrative*
/// step calls a cheap AI model to turn the about-to-be-dropped turns into a
/// short natural-language summary.  With async compaction (phase 08), the
/// narrative cost is per-epoch and off the interactive path, so it defaults
/// to `true`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DigestConfig {
    /// When true, each digest pass calls the `[models.digest]` entry (falling
    /// back to `[models.default]`) to generate a narrative summary of the
    /// compacted turns; the narrative is prepended to the structured tally.
    /// Default: true (cost is per-epoch and off the interactive path as of
    /// phase 08).
    #[serde(default = "default_narrative_enabled")]
    pub narrative_enabled: bool,
}

fn default_narrative_enabled() -> bool {
    true
}

impl Default for DigestConfig {
    fn default() -> Self {
        Self {
            narrative_enabled: default_narrative_enabled(),
        }
    }
}

/// Compaction budget and threshold configuration.
///
/// Controls token-pressure-driven compaction: when to elide, when to compact,
/// and what the post-compaction target should be.  The `emergency_pct` field
/// is parsed now but consumed in phase 08 (async emergency compaction).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CompactionConfig {
    /// Elide oversized old tool_results at this % of the context window.
    #[serde(default = "default_elide_at_pct")]
    pub elide_at_pct: u32,
    /// Build a digest and cut the working set at this %.
    #[serde(default = "default_compact_at_pct")]
    pub compact_at_pct: u32,
    /// Post-compaction working-set target as % of the context window.
    #[serde(default = "default_target_pct")]
    pub target_pct: u32,
    /// Synchronous emergency compaction threshold (consumed in phase 08;
    /// parsed now so configs written today keep working).
    #[serde(default = "default_emergency_pct")]
    pub emergency_pct: u32,
    /// Number of uncovered epochs that triggers a chapter rollup.
    /// When the count of uncovered epochs exceeds this value, the oldest
    /// ROLLUP_FOLD (5) uncovered epochs are folded into one chapter record.
    /// Default: 10.
    #[serde(default = "default_rollup_after")]
    pub rollup_after: u32,
    /// When true, each (interactive, async) epoch build asks the digest model to
    /// propose 0–3 durable facts, written to persistent memory (category
    /// "knowledge", source "compaction"). Off by default — one small-model call
    /// per epoch, and it writes to shared memory.
    #[serde(default)]
    pub extract_memories: bool,
}

fn default_elide_at_pct() -> u32 {
    50
}
fn default_compact_at_pct() -> u32 {
    60
}
fn default_target_pct() -> u32 {
    40
}
fn default_emergency_pct() -> u32 {
    85
}
fn default_rollup_after() -> u32 {
    10
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            elide_at_pct: default_elide_at_pct(),
            compact_at_pct: default_compact_at_pct(),
            target_pct: default_target_pct(),
            emergency_pct: default_emergency_pct(),
            rollup_after: default_rollup_after(),
            extract_memories: false,
        }
    }
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

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            per_tool_batch: default_per_tool_batch(),
            total_tool_calls_per_turn: 0,
            tool_result_chars: default_tool_result_chars(),
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
    pub fn validate(&self) {
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
    /// Per-1M-token input cost in USD. None = unknown (will log warning).
    /// Local providers (lmstudio, ollama) should set this to Some(0.0).
    #[serde(default)]
    pub input_cost_per_mtok: Option<f64>,
    /// Per-1M-token output cost in USD.
    #[serde(default)]
    pub output_cost_per_mtok: Option<f64>,
    /// Cache-read rate (Anthropic ephemeral cache; Gemini implicit cache).
    /// Typically ~10% of input rate for Anthropic.
    #[serde(default)]
    pub cache_read_cost_per_mtok: Option<f64>,
    /// Cache-write rate (Anthropic cache creation; ~125% of input rate).
    #[serde(default)]
    pub cache_write_cost_per_mtok: Option<f64>,
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
            input_cost_per_mtok: None,
            output_cost_per_mtok: None,
            cache_read_cost_per_mtok: None,
            cache_write_cost_per_mtok: None,
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

    /// Resolve pricing for this model entry.
    /// Returns a `Pricing` struct with rates from the user's config, or `None`
    /// if no cost fields are set (cost accounting will report `$0+` for the model).
    /// Local providers always return zero pricing.
    pub fn pricing(&self) -> Option<Pricing> {
        if self.provider == "ollama" || self.provider == "lmstudio" {
            return Some(Pricing::zero());
        }

        let has_user = self.input_cost_per_mtok.is_some()
            || self.output_cost_per_mtok.is_some()
            || self.cache_read_cost_per_mtok.is_some()
            || self.cache_write_cost_per_mtok.is_some();

        if has_user {
            return Some(Pricing {
                input_per_mtok: self.input_cost_per_mtok.unwrap_or(0.0),
                output_per_mtok: self.output_cost_per_mtok.unwrap_or(0.0),
                cache_read_per_mtok: self.cache_read_cost_per_mtok.unwrap_or(0.0),
                cache_write_per_mtok: self.cache_write_cost_per_mtok.unwrap_or(0.0),
                source: PricingSource::UserConfig,
            });
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Pricing schema
// ---------------------------------------------------------------------------

/// Where a pricing rate originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingSource {
    /// Rate came from the user's `config.toml` `[models.<name>]` section.
    /// Alias covers records written before pricing moved to config-only.
    #[serde(alias = "BuiltinDefault")]
    UserConfig,
    /// Provider is local (ollama, lmstudio) — all rates are 0.0.
    Local,
    /// Model is not recognized and no user pricing was set.
    Unknown,
}

/// Resolved per-model pricing rates (per million tokens, in USD).
///
/// All fields are concrete `f64` values — the resolution from `Option<f64>`
/// config fields to defaults happens in `ModelEntry::pricing()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    /// Input cost per 1M tokens (USD).
    pub input_per_mtok: f64,
    /// Output cost per 1M tokens (USD).
    pub output_per_mtok: f64,
    /// Cache-read cost per 1M tokens (USD).
    pub cache_read_per_mtok: f64,
    /// Cache-write cost per 1M tokens (USD).
    pub cache_write_per_mtok: f64,
    /// Where these rates came from.
    pub source: PricingSource,
}

impl Pricing {
    /// Constructor for local providers — all rates are zero.
    pub fn zero() -> Self {
        Pricing {
            input_per_mtok: 0.0,
            output_per_mtok: 0.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            source: PricingSource::Local,
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
