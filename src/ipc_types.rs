//! The payload structs `Request` and `Response` carry: container, pane,
//! schedule, script, shell, log, ghost, limits, runbook and session summaries.

use super::*;

/// One sandboxed container this daemon owns (M19 phase-05).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContainerInfo {
    /// Short (12-char) container id, as docker displays it.
    pub id: String,
    /// `running`, `exited`, `created`, …
    pub state: String,
    pub image: String,
    /// Owning session, from the `de.session` label. `None` for a container
    /// started before the label existed.
    pub session: Option<String>,
    /// Carries `de.ghost=1`.
    pub is_ghost: bool,
}

/// Sandbox health plus the containers it owns, returned by
/// `Request::ContainerStatus`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContainerStatusReport {
    /// `[sandbox] enabled`. When false the rest is reported, not enforced.
    pub enabled: bool,
    pub runtime_ok: bool,
    /// Version string when reachable; the operator-facing reason when not.
    pub runtime_detail: String,
    /// Live image id vs the lockfile, already rendered for display.
    pub image_detail: String,
    pub containers: Vec<ContainerInfo>,
}

/// A snapshot of a single tmux pane (M12 D7). A named struct rather than a
/// widening tuple — the tuple had already reached five positional fields.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaneInfo {
    pub id: String,
    /// Window-relative pane index — the number `ctrl+a q` shows.
    pub idx: usize,
    pub window: String,
    /// Owning tmux session. Equal to the user's own session for every row this
    /// phase produces; carried because the cache is multi-session (D1) and the
    /// renderer must not assume otherwise.
    pub session: String,
    pub cmd: String,
    pub cwd: String,
    pub title: String,
    /// `PaneStatus` rendered through its `Display` impl.
    pub status: String,
    /// Seconds since the pane last produced output; `None` when unknown.
    pub activity_age_secs: Option<u64>,
    /// True when this pane is the session's pinned foreground target.
    pub is_target: bool,
    /// One-line preview of the pane's buffer, masked.
    pub preview: String,
}

/// Summary of a scheduled job for the `ScheduleList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScheduleListItem {
    pub id: String,
    pub name: String,
    /// Human-readable schedule kind (e.g. "every 5m", "once at 2026-03-01 12:00 UTC").
    pub kind: String,
    /// Human-readable action description.
    pub action: String,
    /// Human-readable status.
    pub status: String,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
}

/// Summary of a script file for the `ScriptList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScriptListItem {
    pub name: String,
    pub size: u64,
}

/// One PTY shell the daemon runs, as the client sees it (M23).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ShellInfo {
    /// `s3`.
    pub id: String,
    /// `"local"` or the configured host name (`HostId::as_str()`).
    pub host: String,
    /// The owner's key (`Owner::key()`), e.g. `chat:<session>` or `ghost:<job>`.
    pub owner: String,
    pub started_unix: u64,
    pub dedicated: bool,
    /// `Some` once the shell's host has been seen dead.
    pub exited_unix: Option<u64>,
    /// The most recent command recorded in the shell's index, if any.
    pub last_command: Option<String>,
    /// What holds the shell's foreground, as the daemon last observed it —
    /// the `Display` of its `last_foreground`, or `running <cmd>` when an
    /// in-flight command started later. `None` when nothing was observed.
    #[serde(default)]
    pub foreground: Option<String>,
    /// The last readable line of the shell's latest recorded command output,
    /// masked and cut to 120 characters. `None` when there is none.
    #[serde(default)]
    pub last_line: Option<String>,
}

/// One shell recording on disk, as `/logs` lists it (M23).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ShellLogInfo {
    /// The log id, `s3-1788470000`: the shell id plus its start time.
    pub id: String,
    pub started_unix: u64,
    /// The owner's key (`chat:<session>`, `ghost:<job>`), when the recording's
    /// command index carries one.
    pub owner: Option<String>,
    /// `"local"` or the host name, when the command index carries one.
    pub host: Option<String>,
    /// How many commands the index holds; `None` when there is no readable
    /// index (which is not the same as 0).
    pub commands: Option<usize>,
    /// The first recorded command, masked.
    pub first_command: Option<String>,
}

/// One output event of a shell recording: its time since the recording
/// started, in seconds, and the text the shell wrote (M23).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LogEvent {
    pub t: f64,
    pub data: String,
}

/// One recorded command in a shell recording (M23): `event` is the index in
/// `ShellLog.events` where the command begins — the output event right after
/// it was typed, so the prompt and the command line are in view.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LogCommand {
    pub event: usize,
    pub command: String,
    pub exit_code: i32,
}

/// Configuration for autonomous Ghost Shells triggered by a runbook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GhostConfig {
    /// Whether the AI can operate autonomously in a Ghost Shell.
    pub enabled: bool,
    /// List of script names (in `~/.daemoneye/scripts/`) pre-approved for sudo execution.
    /// Non-sudo commands are always allowed; sudo commands must be listed here and have a
    /// corresponding `/etc/sudoers.d/` `NOPASSWD` entry (via `daemoneye install-sudoers`).
    pub auto_approve_scripts: Vec<String>,
    /// Maximum number of AI turns before the session is forcibly stopped.
    /// `0` means use the daemon default (20).
    pub max_ghost_turns: usize,
    /// Whether to prepend `sudo` when executing pre-approved scripts.
    /// Intended for use with `/etc/sudoers.d/` `NOPASSWD` rules.
    pub run_with_sudo: bool,
    /// Optional SSH destination (e.g. `user@host` or `host`) for remote execution.
    /// When set, approved commands are automatically wrapped in `ssh <target> <cmd>`.
    /// Scripts are resolved to `~/.daemoneye/scripts/<name>` on the remote host.
    /// The AI is instructed not to SSH manually — the policy handles it transparently.
    #[serde(default)]
    pub ssh_target: Option<String>,
    /// Optional model name override (a key from `[models.<name>]` in config).
    /// When set, this ghost shell uses the named model instead of the daemon default.
    /// Falls back to the default model if the name is not found in config.
    #[serde(default)]
    pub model: Option<String>,
    /// Allow the ghost to run non-sudo commands freely without listing them in
    /// `auto_approve_scripts`.  Non-sudo commands are already permitted by the OS
    /// permission boundary; this flag makes that permission explicit in the ghost
    /// shell system prompt and in `/approvals` status output.
    /// Set per-runbook via `auto_approve_commands: true` in frontmatter, or
    /// daemon-wide via `[approvals] ghost_commands = true` in `config.toml`.
    #[serde(default)]
    pub auto_approve_commands: bool,
    /// Whether a ghost may type into a shell whose foreground is a running
    /// program (line mode or TUI), rather than only at its prompt. Set
    /// per-runbook via `allow_tui_input: true` in frontmatter. It widens
    /// **where** a permitted ghost may type, never **whether** it may: the
    /// agent `ToolPolicy` must still explicitly allow `send_input` first.
    /// A paused, exited or unknown shell stays refused either way.
    #[serde(default)]
    pub allow_tui_input: bool,
    /// Name of the agent this ghost shell was spawned from.
    /// Set when a runbook specifies `agent: <name>` or the AI passes an agent
    /// to `spawn_ghost_shell`. Used for logging and audit purposes.
    #[serde(default)]
    pub agent: Option<String>,
    /// Agent-level tool access policy merged from `AgentConfig.tools`.
    /// Enforced in `execute_tool_call()` before the runbook `GhostPolicy` check.
    #[serde(default)]
    pub tool_policy: Option<crate::agents::ToolPolicy>,
    /// Delegation depth: 0 = top-level coordinator, 1 = specialist, 2+ = blocked.
    #[serde(default)]
    pub spawn_depth: u8,
    /// Job ID of the parent ghost that spawned this one (if any).
    #[serde(default)]
    pub parent_job_id: Option<String>,
    /// Hosts this ghost may open shells on, by `[hosts.<name>]` key.
    /// Empty = unrestricted, preserving every existing runbook's behaviour
    /// (a `hosts:` line is absent from all of them). Enforced by
    /// `GhostPolicy::may_use_host` at both host-taking tool calls.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// The `[hosts]` key this runbook's `ssh_target` resolved to at ghost
    /// start; `None` when it named no configured host (the 1.x path) or was
    /// ambiguous.
    #[serde(default)]
    pub ssh_target_host: Option<String>,
    /// That host's `sudo_scripts`, copied at ghost start.
    #[serde(default)]
    pub ssh_target_sudo_scripts: Vec<String>,
}

/// Effective limit configuration sent in `DaemonStatus` and `LimitsInfo` responses.
/// All values mirror `config.limits.*`; 0 means unlimited.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LimitsSummary {
    pub per_tool_batch: u32,
    pub total_tool_calls_per_turn: u32,
    pub tool_result_chars: usize,
    pub max_turns: usize,
    pub max_tool_calls_per_session: usize,
    /// Per-tool overrides sorted by tool name.  Each entry is `(tool_name, cap)`; 0 = uncapped.
    #[serde(default)]
    pub per_tool_overrides: Vec<(String, u32)>,
}

/// Summary of a runbook for the `RunbookList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RunbookListItem {
    pub name: String,
    pub tags: Vec<String>,
    pub ghost_config: GhostConfig,
}

/// Summary of a saved named session for `/session list`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionSummary {
    pub name: String,
    pub description: String,
    /// RFC 3339 — when the session was first saved.
    pub created_at: String,
    /// RFC 3339 — when the session was last saved or resumed.
    pub last_updated: String,
    pub turn_count: usize,
    pub message_count: usize,
    pub artifact_count: usize,
}
