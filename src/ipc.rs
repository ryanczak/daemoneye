use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long a user has to approve or deny a foreground/background tool call.
///
/// Both sides of the approval exchange bound themselves with this: the daemon
/// gives up on the user after it, and the client's read (F7) gives up on the
/// panel at the same instant so the panel never outlives the daemon's wait.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

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

/// Messages sent from the CLI client to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// A simple ping to check if the daemon is alive.
    Ping,
    /// Every PTY shell the daemon runs, with its state and last command (M23).
    ShellList,
    /// Write raw bytes into one PTY shell (M23) — typing at it, or answering a
    /// program's stdin prompt.
    ShellInput {
        id: String,
        bytes: Vec<u8>,
    },
    /// Open a PTY shell owned by the user (`/shell [host]`, M23), on the
    /// daemon host or on a configured `[hosts.<name>]` host. Answered with
    /// `Response::ShellOpened`.
    ShellOpen {
        host: Option<String>,
    },
    /// Signal one PTY shell's foreground job from attached mode (M23):
    /// `"STOP"` pauses it, `"CONT"` resumes it, `"INT"` interrupts it.
    /// Answered with `Response::ShellSignalled`.
    ShellSignal {
        id: String,
        signal: String,
    },
    /// Resize one PTY shell's terminal (M23).
    ShellResize {
        id: String,
        rows: u16,
        cols: u16,
    },
    /// Watch one PTY shell (M23): its current screen, then — with `follow` —
    /// its live output until the shell exits or the client disconnects.
    ShellTail {
        id: String,
        follow: bool,
    },
    /// The shell recordings on disk, newest first (M23). Each filter is
    /// optional: `since` keeps recordings started at or after that unix time,
    /// `host` a host name (`"local"` for the daemon host), `owner` an owner
    /// kind (`"chat"`, `"ghost"`, `"adopted"`). `limit` caps the rows.
    ShellLogList {
        since: Option<u64>,
        host: Option<String>,
        owner: Option<String>,
        limit: Option<usize>,
    },
    /// One shell recording by its log id (`s3-1788470000`), for `/log` (M23).
    ShellLogRead {
        id: String,
    },
    /// Ask the daemon to shut down cleanly.
    Shutdown,
    /// Send an ask request with the invoking tmux pane (if in tmux).
    /// `session_id` is set by `run_chat` to maintain conversational memory across turns.
    /// `chat_pane` is the pane ID of the AI chat pane itself (i.e. `$TMUX_PANE` inside
    /// `daemoneye chat`) so the daemon can switch focus back to it after a foreground sudo command.
    Ask {
        query: String,
        tmux_pane: Option<String>,
        session_id: Option<String>,
        chat_pane: Option<String>,
        /// Optional prompt override — name of a prompt in ~/.daemoneye/prompts/.
        /// When set, the daemon uses this instead of the configured default.
        prompt: Option<String>,
        /// Width of the chat pane in columns (terminal_width() value from the client).
        /// Passed to the AI so it formats prose for the actual display width.
        #[serde(default)]
        chat_width: Option<usize>,
        /// The tmux session the client is running in, resolved by the client before
        /// connecting. Used by the daemon to adopt (or confirm) the correct session
        /// when started by systemd before any user session existed.
        #[serde(default)]
        tmux_session: Option<String>,
        /// The target pane for foreground commands, resolved client-side by sibling
        /// detection or user prompt. Eliminates mid-conversation pane picker prompts.
        #[serde(default)]
        target_pane: Option<String>,
        /// Optional model override for this session.  When set on the first turn, the
        /// daemon pins this model for the lifetime of the session.  Later turns with a
        /// different or absent value have no effect once the session model is pinned.
        #[serde(default)]
        model: Option<String>,
    },
    /// Approve or deny a tool call.  When `approved` is false and `user_message`
    /// is `Some`, the daemon discards the pending tool chain and injects the
    /// message as a new user turn so the AI can course-correct.
    ToolCallResponse {
        id: String,
        approved: bool,
        /// Optional corrective message typed at the approval prompt.
        /// Present only when the user wants to redirect the agent.
        #[serde(default)]
        user_message: Option<String>,
    },
    /// User-supplied credential (password / passphrase) in response to
    /// `Response::CredentialPrompt`. The daemon injects it into the background tmux window.
    CredentialResponse {
        id: String,
        credential: String,
    },
    /// User's pane selection in response to `Response::PaneSelectPrompt`.
    PaneSelectResponse {
        id: String,
        pane_id: String,
    },
    /// Re-collect the system context (OS info, memory, processes, history).
    /// Daemon responds with Response::Ok when done.
    Refresh,
    /// Approve or deny a script write proposed by the AI.
    ScriptWriteResponse {
        id: String,
        approved: bool,
    },
    /// Approve or deny a job schedule proposed by the AI.
    ScheduleWriteResponse {
        id: String,
        approved: bool,
    },
    /// Approve or deny a runbook write proposed by the AI.
    RunbookWriteResponse {
        id: String,
        approved: bool,
    },
    /// Approve or deny a runbook delete proposed by the AI.
    RunbookDeleteResponse {
        id: String,
        approved: bool,
    },
    /// Approve or deny a file operation proposed by the AI (edit_file tool).
    EditFileResponse {
        id: String,
        approved: bool,
        #[serde(default)]
        user_message: Option<String>,
    },
    ScriptDeleteResponse {
        id: String,
        approved: bool,
    },
    /// Switch the active model for the given session.
    /// The daemon validates the name against configured models and responds with
    /// `Response::ModelChanged` on success or `Response::Error` if unknown.
    SetModel {
        session_id: String,
        model: String,
    },
    /// List all configured model names and the session's current active model.
    /// The daemon responds with `Response::ModelList`.
    ListModels {
        session_id: String,
    },
    /// Query the daemon's current operational status (F1).
    Status,
    /// Query sandbox runtime health and the containers this daemon owns.
    /// Separate from `Status` because collecting it spawns the container
    /// runtime, which `Status` must not wait on.
    ContainerStatus,
    /// Pin the foreground target pane for the given session.
    /// The daemon updates `default_target_pane`, persists to `pane_prefs.json`,
    /// and responds with `Response::PaneChanged`.
    SetPane {
        session_id: String,
        pane_id: String,
    },
    /// List targetable panes known to the daemon for the given session.
    /// The daemon responds with `Response::PaneList`.
    ListPanesForSession {
        session_id: String,
    },
    /// Query the effective limits config and this session's live counters.
    /// The daemon responds with `Response::LimitsInfo`.
    QueryLimits {
        session_id: String,
    },
    /// Reset the per-session cumulative tool-call counter to zero for the given session.
    /// The daemon responds with `Response::Ok`.
    ResetSessionToolCount {
        session_id: String,
    },
    /// Save the current session under `name`.
    /// Returns `Response::SessionSaved` on success, `Response::Error` on failure.
    /// Set `force = true` to overwrite an existing session with the same name.
    SaveSession {
        session_id: String,
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        force: bool,
    },
    /// Load a previously saved session into the current session slot.
    /// Returns `Response::SessionLoaded` on success, `Response::Error` on failure.
    /// Fails if the current session has unread changes (`dirty = true`) unless `force = true`.
    LoadSession {
        session_id: String,
        name: String,
        #[serde(default)]
        force: bool,
    },
    /// List all saved sessions.
    /// Returns `Response::SavedSessionList`.
    ListSavedSessions,
    /// Delete a named saved session from disk.
    /// Returns `Response::Ok` on success, `Response::Error` if the name is not found.
    DeleteSavedSession {
        name: String,
    },
    /// Rename a saved session.
    /// Returns `Response::Ok` on success, `Response::Error` on failure.
    RenameSavedSession {
        old_name: String,
        new_name: String,
    },
    /// Cancel the in-flight turn of a session. Sent on a fresh connection
    /// (out-of-band) by the chat client when the user interrupts, so the
    /// streaming connection's reader is never contended.
    Cancel {
        session_id: String,
    },
    /// Resume a finished ghost run from its briefing plus a fresh directive.
    ResumeRun {
        /// The finished run's id, as `daemoneye runs` prints it.
        run_id: String,
        /// What the resumed run should do differently.
        directive: String,
    },
    /// Register this connection as a live chat presence for `session_id` (M27
    /// phase 03). The daemon replies with zero or one `Response::SystemMsg`
    /// (the catch-up brief) and then `Response::Ok`, then holds the
    /// connection until the client closes it. When the last connection for
    /// the session closes, detach state is stamped so a later re-attach can
    /// be briefed.
    ChatAttach {
        session_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentCommand {
    pub id: usize,
    pub cmd: String,
    pub timestamp: String,
    pub mode: String,
    pub approval: String,
    pub status: String,
}

/// What kind of approval a `ToolCallPrompt` is asking for, so the client can
/// render the stakes rather than showing every command identically.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Classification {
    /// An ordinary command on the daemon host.
    #[default]
    Command,
    /// A command on a configured `[hosts.<name>]` host.
    RemoteShell { host: String },
    /// A command naming a host that `[hosts]` does not configure.
    NewHost { host: String },
    /// A ghost shell reaching outside its sandbox.
    EscapeHatch,
    /// A command or input aimed at a shell the user opened with `/shell`.
    /// Always asked: no session grant covers the user's own shell.
    UserShell { shell: String },
}

/// Classify a tool-call approval from its destination and origin — pure, so
/// the classifier is unit-testable with no config on disk.
///
/// The rules, in order:
/// 1. a ghost reaching a shell is the escape-hatch case, whatever the host —
///    it outranks the host question;
/// 2. no host, or `"local"` in any case, is an ordinary command on the daemon
///    host (`HostId::parse` maps `"local"` to `HostId::Local`; this matches
///    it rather than diverging);
/// 3. a name `[hosts]` configures is a remote shell;
/// 4. anything else is a new host.
pub fn classify(host: Option<&str>, is_ghost: bool, known_hosts: &[String]) -> Classification {
    if is_ghost {
        return Classification::EscapeHatch;
    }
    let named = match host {
        None => return Classification::Command,
        Some(name) if name.eq_ignore_ascii_case("local") => return Classification::Command,
        Some(name) => name,
    };
    if known_hosts.iter().any(|k| k == named) {
        Classification::RemoteShell {
            host: named.to_string(),
        }
    } else {
        Classification::NewHost {
            host: named.to_string(),
        }
    }
}

/// Messages sent from the daemon back to the CLI client.
#[derive(Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)] // DaemonStatus is large by design; boxing would add indirection to every IPC response match
pub enum Response {
    /// Acknowledgment of a successful request.
    Ok,
    /// Every PTY shell the daemon runs, with its state and last command (M23).
    ShellList { shells: Vec<ShellInfo> },
    /// The recordings `ShellLogList` matched, newest first and capped at its
    /// limit; `total` counts every match before the cap.
    ShellLogList {
        logs: Vec<ShellLogInfo>,
        total: usize,
    },
    /// The recording `ShellLogRead` named: the terminal size it was recorded
    /// at, its output events in order, and its indexed commands. `truncated`
    /// is true when the oldest events were dropped to fit the size cap.
    ShellLog {
        id: String,
        rows: u16,
        cols: u16,
        events: Vec<LogEvent>,
        commands: Vec<LogCommand>,
        truncated: bool,
    },
    /// The first frame of a `ShellTail`: escape codes that redraw the shell's
    /// current screen, and the shell's terminal size, which those codes assume.
    /// `bytes` is empty when the shell's host is too old to provide a screen.
    ShellScreen {
        bytes: Vec<u8>,
        rows: u16,
        cols: u16,
    },
    /// Live output from a followed shell, after its `ShellScreen`.
    ShellChunk { bytes: Vec<u8> },
    /// The shell `ShellOpen` opened, owned by the user.
    ShellOpened { id: String },
    /// The shell's state once a `ShellSignal` took effect: `"Idle"`,
    /// `"Running"`, `"Paused"` or `"Exited"`. After `INT`, `cancelled_run`
    /// names the run that owned the shell and was cancelled with it.
    ShellSignalled {
        state: String,
        #[serde(default)]
        cancelled_run: Option<String>,
    },
    /// The followed shell has exited; no more frames follow.
    ShellExited,
    /// An error occurred on the daemon side.
    Error(String),
    /// Sent once before streaming begins; carries session state so the client
    /// can display a stable turn counter and context-budget indicator.
    SessionInfo {
        message_count: usize,
        /// Ever-increasing turn number for this session (never reset by compaction).
        #[serde(default)]
        turn_count: usize,
        /// Cumulative cost of this session so far.
        #[serde(default)]
        session_cost_usd: f64,
        /// Whether any AI call in this session had Unknown pricing.
        #[serde(default)]
        has_untracked_cost: bool,
    },
    /// A stream of tokens from the AI.
    Token(String),
    /// A system-level notification from the daemon (sudo alerts, pane-switch
    /// notices, etc.).  Displayed with a distinct amber prefix.
    SystemMsg(String),
    /// A prompt for the user to approve a tool call.
    ToolCallPrompt {
        id: String,
        command: String,
        background: bool,
        /// The pane ID (`%N`) that will receive the command (foreground only).
        /// `None` for background commands (which run in a daemon-managed window).
        /// The client uses this to show the window-relative index and to
        /// visually highlight the pane during the approval window.
        #[serde(default)]
        target_pane: Option<String>,
        /// What kind of approval this is — ordinary, remote, a new host, or a
        /// ghost reaching outside its sandbox. Defaulted so an older client
        /// still deserialises a newer daemon's frame.
        #[serde(default)]
        classification: Classification,
    },
    /// The approved background command requires a credential (sudo password, etc.).
    /// The client MUST prompt the user with echo disabled and return a `CredentialResponse`.
    CredentialPrompt { id: String, prompt: String },
    /// The output captured after an approved tool call completes.
    /// Sent to the client so it can display a dimmed result block.
    /// `tool_call_id` joins this output to the AI tool call that produced it,
    /// and to the matching `tool_results` record in the session JSONL.
    ToolResult {
        tool_call_id: String,
        output: String,
    },
    /// Emitted before the daemon executes a silent tool call (one without an
    /// approval prompt or panel response). Allows the client to display a
    /// one-line history entry and animate an elapsed timer while the tool runs.
    ToolStarted {
        id: String,
        tool: String,
        /// Human-readable argument summary, e.g. `"path=src/foo.rs lines=10-50"`.
        summary: String,
    },
    /// Emitted after a silent tool call returns (when no panel response will
    /// follow). The client replaces the spinner line with a final `⎿` result line.
    ToolFinished {
        id: String,
        ok: bool,
        elapsed_ms: u64,
        #[serde(default)]
        detail: Option<String>,
    },
    /// Daemon cannot determine the target pane and needs the user to choose.
    /// Client displays the list and returns a `Request::PaneSelectResponse`.
    PaneSelectPrompt { id: String, panes: Vec<PaneInfo> },
    /// The AI wants to delete a script; the client MUST confirm with the user,
    /// then return `Request::ScriptDeleteResponse`.
    ScriptDeletePrompt { id: String, script_name: String },
    /// The AI wants to write a script; the client MUST show the content and
    /// prompt the user for approval, then return `Request::ScriptWriteResponse`.
    /// `existing_content` is `Some` when the script already exists on disk so
    /// the client can render a diff instead of the raw new content.
    ScriptWritePrompt {
        id: String,
        script_name: String,
        content: String,
        #[serde(default)]
        existing_content: Option<String>,
    },
    /// The AI wants to schedule a job; the client MUST show the details and
    /// prompt the user for approval, then return `Request::ScheduleWriteResponse`.
    ScheduleWritePrompt {
        id: String,
        name: String,
        kind: String,
        action: String,
    },
    /// The current list of scheduled jobs.
    ScheduleList { jobs: Vec<ScheduleListItem> },
    /// The current list of scripts in `~/.daemoneye/scripts/`.
    ScriptList { scripts: Vec<ScriptListItem> },
    /// The AI wants to write a runbook; the client MUST show the content and
    /// prompt the user for approval, then return `Request::RunbookWriteResponse`.
    /// `existing_content` is `Some` when the runbook already exists on disk so
    /// the client can render a diff instead of the raw new content.
    RunbookWritePrompt {
        id: String,
        runbook_name: String,
        content: String,
        #[serde(default)]
        existing_content: Option<String>,
    },
    /// The AI wants to delete a runbook; the client MUST show affected jobs and
    /// prompt the user for approval, then return `Request::RunbookDeleteResponse`.
    RunbookDeletePrompt {
        id: String,
        runbook_name: String,
        /// Names of scheduled jobs that reference this runbook.
        active_jobs: Vec<String>,
    },
    /// The current list of runbooks in `~/.daemoneye/runbooks/`.
    RunbookList { runbooks: Vec<RunbookListItem> },
    /// The AI wants to perform a file operation; the client MUST show a colored
    /// diff and prompt the user for approval, then return `Request::EditFileResponse`.
    ///
    /// `operation` is one of `"edit"` | `"create"` | `"delete"` | `"copy"`.
    /// For `"edit"`: `existing_content` = original file, `new_content` = result after replacement.
    /// For `"create"`: `existing_content` = None, `new_content` = content to write.
    /// For `"delete"`: `existing_content` = current file content, `new_content` = None.
    /// For `"copy"`:  `existing_content` = None, `new_content` = source content,
    ///                `dest_path` = destination path.
    EditFilePrompt {
        id: String,
        path: String,
        operation: String,
        #[serde(default)]
        existing_content: Option<String>,
        #[serde(default)]
        new_content: Option<String>,
        #[serde(default)]
        dest_path: Option<String>,
        /// true = the edit runs as root via the privilege tool; the client
        /// never auto-approves such an edit and says so in the panel.
        #[serde(default)]
        sudo: bool,
    },

    /// Sent after each AI turn completes, carrying the prompt token count from
    /// that turn. The client uses this to update the context-budget display.
    UsageUpdate { prompt_tokens: u32 },
    /// Sent periodically while the daemon is waiting for a slow LLM to produce
    /// the next token. The client treats this as a no-op; receiving it resets
    /// the per-token deadline so slow local models don't trigger a timeout.
    KeepAlive,
    /// Confirmation that the session's active model was changed (response to `SetModel`).
    ModelChanged { model: String },
    /// All configured model names and the session's current active model
    /// (response to `ListModels`).
    /// Each entry is `(key_name, model_id)` — e.g. `("opus", "claude-opus-4-6")`.
    ModelList {
        models: Vec<(String, String)>,
        active: String,
    },
    /// Confirmation that the session's foreground target pane was changed (response to `SetPane`).
    PaneChanged {
        pane_id: String,
        description: String,
    },
    /// List of targetable panes (response to `ListPanesForSession`).
    PaneList { panes: Vec<PaneInfo> },
    /// Sandbox status snapshot (response to `Request::ContainerStatus`).
    ContainerStatus { report: ContainerStatusReport },
    /// Daemon status snapshot returned in response to `Request::Status` (F1).
    DaemonStatus {
        uptime_secs: u64,
        pid: u32,
        active_sessions: usize,
        /// Sum of turn_count across all active sessions.
        #[serde(default)]
        total_turns: usize,
        provider: String,
        model: String,
        /// All model names configured in `[models.*]` sections, sorted.
        #[serde(default)]
        available_models: Vec<String>,
        socket_path: String,
        schedule_count: usize,
        commands_fg_succeeded: usize,
        commands_fg_failed: usize,
        #[serde(default)]
        commands_fg_approved: usize,
        #[serde(default)]
        commands_fg_denied: usize,
        commands_bg_succeeded: usize,
        commands_bg_failed: usize,
        #[serde(default)]
        commands_bg_approved: usize,
        #[serde(default)]
        commands_bg_denied: usize,
        commands_sched_succeeded: usize,
        commands_sched_failed: usize,
        #[serde(default)]
        ghosts_launched: usize,
        #[serde(default)]
        ghosts_active: usize,
        #[serde(default)]
        ghosts_completed: usize,
        #[serde(default)]
        ghosts_failed: usize,
        webhooks_received: usize,
        webhooks_rejected: usize,
        webhook_url: String,
        runbook_count: usize,
        runbooks_created: usize,
        runbooks_executed: usize,
        runbooks_deleted: usize,
        script_count: usize,
        scripts_created: usize,
        scripts_executed: usize,
        scripts_deleted: usize,
        memories_created: usize,
        memories_recalled: usize,
        memories_deleted: usize,
        schedules_created: usize,
        schedules_executed: usize,
        schedules_deleted: usize,
        active_prompt_tokens: u32,
        context_window_tokens: u32,
        recent_commands: Vec<RecentCommand>,
        memory_breakdown: std::collections::HashMap<String, usize>,
        /// Redaction counts by type since daemon start (all built-in types included, even if zero).
        #[serde(default)]
        redaction_counts: std::collections::HashMap<String, usize>,
        /// Number of session history compaction events since daemon start.
        #[serde(default)]
        compactions: usize,
        /// Cumulative compression ratio (msgs_in / msgs_out) across all compactions.  0.0 if none.
        #[serde(default)]
        compaction_ratio: f64,
        /// Script write approvals/denials since daemon start.
        #[serde(default)]
        scripts_approved: usize,
        #[serde(default)]
        scripts_denied: usize,
        /// Runbook write approvals/denials since daemon start.
        #[serde(default)]
        runbooks_approved: usize,
        #[serde(default)]
        runbooks_denied: usize,
        /// File edit approvals/denials since daemon start.
        #[serde(default)]
        file_edits_approved: usize,
        #[serde(default)]
        file_edits_denied: usize,
        /// Effective limit configuration (from `config.limits`).
        #[serde(default)]
        limits: LimitsSummary,
        /// Active agent sessions: `(agent_name, task_or_unknown)`. The second element is
        /// the first 40 chars of the task message passed to `spawn_ghost_shell`, or
        /// `"unknown"` if none was recorded.
        #[serde(default)]
        active_agents: Vec<(String, String)>,
        /// Per-session cost totals: `(session_id, cost_usd)`.
        #[serde(default)]
        daemon_session_costs: Vec<(String, f64)>,
        /// Aggregated daemon-wide cost for today from events.jsonl.
        #[serde(default)]
        daemon_total_cost_today_usd: f64,
        /// Cost breakdown by provider for today: `(provider, cost_usd)`.
        #[serde(default)]
        daemon_cost_by_provider: Vec<(String, f64)>,
        /// Cost breakdown by agent for today: `(agent_name, cost_usd)`.
        #[serde(default)]
        daemon_cost_by_agent: Vec<(String, f64)>,
    },
    /// Confirmation that a session was saved (response to `SaveSession`).
    SessionSaved { name: String },
    /// Confirmation that a session was loaded (response to `LoadSession`).
    /// `banner` is shown to the user as a styled announcement; it also describes
    /// the stale-reference warning the AI should heed.
    SessionLoaded {
        name: String,
        message_count: usize,
        turn_count: usize,
        banner: String,
    },
    /// All saved sessions (response to `ListSavedSessions`).
    SavedSessionList { sessions: Vec<SessionSummary> },

    /// Effective limits config + live session counters (response to `QueryLimits`).
    LimitsInfo {
        /// Effective limits from `config.limits`.
        limits: LimitsSummary,
        /// Number of turns completed so far in this session.
        turn_count: usize,
        /// Cumulative non-approval-gated tool calls executed in this session.
        tool_calls_this_session: usize,
        /// Current number of messages in this session's history.
        history_len: usize,
    },
}

impl Response {
    /// A stable, human-readable label for this variant — its PascalCase
    /// name. Used in user-facing "unexpected reply" messages instead of a
    /// `{:?}` debug dump, which would leak internal field values.
    pub fn kind(&self) -> &'static str {
        match self {
            Response::Ok => "Ok",
            Response::ShellList { .. } => "ShellList",
            Response::ShellLogList { .. } => "ShellLogList",
            Response::ShellLog { .. } => "ShellLog",
            Response::ShellScreen { .. } => "ShellScreen",
            Response::ShellChunk { .. } => "ShellChunk",
            Response::ShellSignalled { .. } => "ShellSignalled",
            Response::ShellOpened { .. } => "ShellOpened",
            Response::ShellExited => "ShellExited",
            Response::Error(_) => "Error",
            Response::SessionInfo { .. } => "SessionInfo",
            Response::Token(_) => "Token",
            Response::SystemMsg(_) => "SystemMsg",
            Response::ToolCallPrompt { .. } => "ToolCallPrompt",
            Response::CredentialPrompt { .. } => "CredentialPrompt",
            Response::ToolResult { .. } => "ToolResult",
            Response::ToolStarted { .. } => "ToolStarted",
            Response::ToolFinished { .. } => "ToolFinished",
            Response::PaneSelectPrompt { .. } => "PaneSelectPrompt",
            Response::ScriptDeletePrompt { .. } => "ScriptDeletePrompt",
            Response::ScriptWritePrompt { .. } => "ScriptWritePrompt",
            Response::ScheduleWritePrompt { .. } => "ScheduleWritePrompt",
            Response::ScheduleList { .. } => "ScheduleList",
            Response::ScriptList { .. } => "ScriptList",
            Response::RunbookWritePrompt { .. } => "RunbookWritePrompt",
            Response::RunbookDeletePrompt { .. } => "RunbookDeletePrompt",
            Response::RunbookList { .. } => "RunbookList",
            Response::EditFilePrompt { .. } => "EditFilePrompt",
            Response::UsageUpdate { .. } => "UsageUpdate",
            Response::KeepAlive => "KeepAlive",
            Response::ModelChanged { .. } => "ModelChanged",
            Response::ModelList { .. } => "ModelList",
            Response::PaneChanged { .. } => "PaneChanged",
            Response::PaneList { .. } => "PaneList",
            Response::ContainerStatus { .. } => "ContainerStatus",
            Response::DaemonStatus { .. } => "DaemonStatus",
            Response::SessionSaved { .. } => "SessionSaved",
            Response::SessionLoaded { .. } => "SessionLoaded",
            Response::SavedSessionList { .. } => "SavedSessionList",
            Response::LimitsInfo { .. } => "LimitsInfo",
        }
    }
}

#[cfg(test)]
#[path = "ipc_tests.rs"]
mod tests;
