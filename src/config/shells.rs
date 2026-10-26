//! Config sections: [shells] and [limits].

use serde::{Deserialize, Serialize};

/// Shell-engine settings (`[shells]`).
/// These values control the PTY-backed shell engine shipped by M20; `rows` and
/// `cols` are the geometry every production spawn reads (`ShellsConfig::size`,
/// via `spawn_size`), and the screen model reads `scrollback_lines`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ShellsConfig {
    /// Max concurrent shells one owner (a chat session, a ghost) may hold.
    /// 0 = unlimited. Default: 5.
    #[serde(default = "default_shells_max_per_owner")]
    pub max_per_owner: u32,
    /// How long an exited shell stays listed before it is reaped.
    /// Default: 300 seconds.
    #[serde(default = "default_shells_exited_retention_secs")]
    pub exited_retention_secs: u64,
    /// Days to keep `var/log/shells/*.cast`. 0 = keep forever. Default: 7.
    #[serde(default = "default_shells_log_retention_days")]
    pub log_retention_days: u32,
    /// Rows of scrollback the screen model retains per shell.
    /// Default: 5000.
    #[serde(default = "default_shells_scrollback_lines")]
    pub scrollback_lines: u32,
    /// Rows of the PTY every production spawn opens. Default: 40.
    #[serde(default = "default_shells_rows")]
    pub rows: u16,
    /// Columns of the PTY every production spawn opens. Default: 120.
    #[serde(default = "default_shells_cols")]
    pub cols: u16,
}

/// Default per-owner concurrent shell cap (5).
fn default_shells_max_per_owner() -> u32 {
    5
}

/// Default exited-shell retention (300 s = 5 min).
fn default_shells_exited_retention_secs() -> u64 {
    300
}

/// Default shell recording retention (7 days).
fn default_shells_log_retention_days() -> u32 {
    7
}

/// Default scrollback depth (5000 rows per shell).
fn default_shells_scrollback_lines() -> u32 {
    5000
}

/// Default shell PTY height (40 rows).
fn default_shells_rows() -> u16 {
    40
}

/// Default shell PTY width (120 columns).
fn default_shells_cols() -> u16 {
    120
}

impl Default for ShellsConfig {
    fn default() -> Self {
        Self {
            max_per_owner: default_shells_max_per_owner(),
            exited_retention_secs: default_shells_exited_retention_secs(),
            log_retention_days: default_shells_log_retention_days(),
            scrollback_lines: default_shells_scrollback_lines(),
            rows: default_shells_rows(),
            cols: default_shells_cols(),
        }
    }
}

impl ShellsConfig {
    /// The `(rows, cols)` a shell spawns with: `rows` clamped to `10..=200`
    /// and `cols` to `40..=500`, so `rows = 0` cannot open an unusable
    /// terminal and a nonsense value cannot exceed any screen.
    pub fn size(&self) -> (u16, u16) {
        (self.rows.clamp(10, 200), self.cols.clamp(40, 500))
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

    /// Maximum wall-clock seconds for a single interactive assistant turn,
    /// including tool execution. On expiry the turn ends with a visible
    /// error. Ghost shells have their own fixed 300 s per-turn budget.
    /// Default: 0 (no limit). Recommended when enabled: 3600.
    #[serde(default)]
    pub turn_timeout_secs: u64,
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
            turn_timeout_secs: 0,
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
        // ignored at runtime, so surface the misconfiguration early. Shared with
        // the runtime guard in `daemon::stream` — one list, so the warning can
        // no longer disagree with the behaviour it describes.
        for tool in crate::ai::tools::APPROVAL_GATED_TOOLS {
            if self.per_tool.contains_key(*tool) {
                log::warn!(
                    "[limits] per_tool.{tool} is set but {tool} is approval-gated and \
                     exempt from per-tool caps — this entry has no effect"
                );
            }
        }
    }
}
