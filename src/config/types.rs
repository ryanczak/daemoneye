use super::*;
use crate::host::HostId;
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
    /// Named remote execution targets, keyed by host name.  Empty by default —
    /// no `[hosts]` section is required.
    #[serde(default)]
    pub hosts: std::collections::HashMap<String, HostEntry>,
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
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub shells: ShellsConfig,
    /// Daily spend cap. Default: off (`0.0`).
    #[serde(default)]
    pub budget: BudgetConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ai: AiConfig::default(),
            models: default_models(),
            hosts: std::collections::HashMap::new(),
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
            retention: RetentionConfig::default(),
            sandbox: SandboxConfig::default(),
            execution: ExecutionConfig::default(),
            shells: ShellsConfig::default(),
            budget: BudgetConfig::default(),
        }
    }
}

/// Daily spend budget (`[budget]` in config.toml).
///
/// The cap is compared against the ledger's *local*-day total
/// ([`crate::daemon::spend::spent_today`]), not the UTC day the
/// `status` cost breakdowns use.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct BudgetConfig {
    /// Maximum USD spend per local calendar day. `0` (the default) means
    /// off — nothing is ever refused. A negative value is warned about at
    /// load and treated as `0`.
    #[serde(default)]
    pub max_cost_per_day: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            max_cost_per_day: 0.0,
        }
    }
}

/// Whether today's spend sits under a configured daily cap.
///
/// Phase 06 (daily-cap) consults this at every model call to decide
/// whether to refuse; phase 05 only displays it.
#[derive(Debug, Clone, PartialEq)]
pub enum CapStatus {
    /// No cap configured (`cap <= 0`) — nothing is ever refused.
    Off,
    /// Today's spend has reached the cap.
    Over {
        /// Total spend for the local day, USD.
        spent: f64,
        /// The configured cap, USD.
        cap: f64,
    },
    /// Today's spend is below the cap.
    Under {
        /// Total spend for the local day, USD.
        spent: f64,
        /// The configured cap, USD.
        cap: f64,
    },
}

/// Pure cap decision: `Off` at `cap <= 0`, `Over` at `spent >= cap`,
/// `Under` below it.
pub fn cap_status(spent: f64, cap: f64) -> CapStatus {
    if cap <= 0.0 {
        CapStatus::Off
    } else if spent >= cap {
        CapStatus::Over { spent, cap }
    } else {
        CapStatus::Under { spent, cap }
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
    /// Delete an inactive session's chat log (`<id>.jsonl`) and its resume
    /// state (`<id>.meta.json`) once the log has not been written for this
    /// many days. 0 = keep forever (default). Archives and epoch files are
    /// never touched: they have their own lifecycle and feed the index.
    #[serde(default)]
    pub log_retention_days: u32,
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
            auto_name_enabled: default_true(),
            load_recent_turns: default_load_recent_turns(),
            archive_retention_days: 0,
            log_retention_days: 0,
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

/// Retention settings for artifact classes that are swept periodically.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RetentionConfig {
    /// Days to keep pane log files (`var/log/panes/*.log`). 0 = keep forever.
    /// Default: 7.
    #[serde(default = "default_pane_log_retention_days")]
    pub pane_log_retention_days: u32,
    /// Days to keep agent mailbox entries (`agents/*/mailbox/*.json`). 0 = keep forever.
    /// Default: 7.
    #[serde(default = "default_mailbox_retention_days")]
    pub mailbox_retention_days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        RetentionConfig {
            pane_log_retention_days: default_pane_log_retention_days(),
            mailbox_retention_days: default_mailbox_retention_days(),
        }
    }
}

fn default_pane_log_retention_days() -> u32 {
    7
}

fn default_mailbox_retention_days() -> u32 {
    7
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
            commands: default_true(),
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
    /// Name of an existing tmux session the daemon observes when launched
    /// outside tmux. The daemon never creates it.
    ///
    /// When launched inside tmux, the current session is used and this setting
    /// is ignored. `""` means no tmux at all.
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
pub(super) fn default_true() -> bool {
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

impl Config {
    /// Walk `[hosts.*]` and return one human-readable problem per bad entry.
    /// Never panics and never stops the daemon: a typo'd host must not take
    /// the whole thing down, so the caller only logs what comes back.
    pub fn validate_hosts(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, entry) in &self.hosts {
            if entry.ssh.trim().is_empty() {
                problems.push(format!(
                    "[hosts.{name}] ssh is empty or whitespace — every host needs a destination; \
                     set ssh = \"user@hostname\" (or a bare alias from ~/.ssh/config)."
                ));
            }
            if name.eq_ignore_ascii_case("local") {
                problems.push(format!(
                    "[hosts.{name}] `local` is reserved — the local host is implicit and cannot \
                     be a [hosts] entry; remove this block and rely on HostId::Local."
                ));
                continue;
            }
            if let Err(err) = HostId::parse(name) {
                problems.push(format!("[hosts.{name}] invalid host name: {err}"));
            }
        }
        problems
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn budget_defaults_to_no_cap() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(
            cfg.budget.max_cost_per_day, 0.0,
            "a config with no [budget] section has no cap"
        );
    }

    #[test]
    fn budget_max_cost_per_day_parses() {
        let cfg: Config = toml::from_str("[budget]\nmax_cost_per_day = 5.0\n").unwrap();
        assert_eq!(cfg.budget.max_cost_per_day, 5.0);
    }
}
