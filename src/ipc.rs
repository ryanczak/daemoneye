use serde::{Deserialize, Serialize};

/// A snapshot of a single tmux pane, sent in `PaneSelectPrompt` so the client
/// can display a numbered list for the user to choose from.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaneInfo {
    pub id: String,
    pub current_cmd: String,
    pub summary: String,
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
}

/// Effective limit configuration sent in `DaemonStatus` and `LimitsInfo` responses.
/// All values mirror `config.limits.*`; 0 means unlimited.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LimitsSummary {
    pub per_tool_batch: u32,
    pub total_tool_calls_per_turn: u32,
    pub tool_result_chars: usize,
    pub max_history: usize,
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
    /// Notify the daemon of an event (e.g. background pane activity from a tmux hook).
    NotifyActivity {
        pane_id: String,
        hook_index: usize,
        session_name: String,
    },
    /// Notify the daemon that a background command finished.
    /// Carries the exit code directly so no scrollback scan is needed.
    NotifyComplete {
        pane_id: String,
        exit_code: i32,
        session_name: String,
    },
    /// Notify the daemon that a pane received focus (`pane-focus-in` hook, N1).
    /// Allows instant active-pane tracking without waiting for the 2 s poll.
    NotifyFocus {
        pane_id: String,
        session_name: String,
    },
    /// Notify the daemon that the active window changed (`session-window-changed` hook, N2).
    /// Triggers a targeted window-list refresh so `[SESSION TOPOLOGY]` stays current.
    NotifyWindowChanged {
        session_name: String,
    },
    /// Notify the daemon that a new tmux session was created (`after-new-session` hook, N14).
    /// The daemon installs per-session hooks for the new session automatically.
    NotifySessionCreated {
        session_name: String,
    },
    /// Notify the daemon that a tmux session was destroyed (`session-closed` hook, A6).
    /// The daemon cleans up bg windows and pipe-pane logs for that session.
    NotifySessionClosed {
        session_name: String,
    },
    /// Notify the daemon that a tmux client attached to a session (`client-attached` hook, N15).
    /// Clears any pending detach state so the catch-up brief is not shown.
    NotifyClientAttached {
        session_name: String,
    },
    /// Notify the daemon that a tmux client detached from a session (`client-detached` hook, N15).
    /// The daemon records the detach time; the next `Ask` will include a catch-up brief.
    NotifyClientDetached {
        session_name: String,
    },
    /// Notify the daemon that the attached terminal was resized (`client-resized` hook, N8).
    /// Updates the cached client viewport so the AI knows the current terminal dimensions.
    NotifyResize {
        width: u16,
        height: u16,
        session_name: String,
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
    /// Compare two named sessions and return an AI-generated diff summary.
    /// Returns `Response::SessionDiff` on success, `Response::Error` on failure.
    DiffSessions {
        name1: String,
        name2: String,
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

/// Messages sent from the daemon back to the CLI client.
#[derive(Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)] // DaemonStatus is large by design; boxing would add indirection to every IPC response match
pub enum Response {
    /// Acknowledgment of a successful request.
    Ok,
    /// An error occurred on the daemon side.
    Error(String),
    /// Sent once before streaming begins; carries session state so the client
    /// can display a stable turn counter and context-budget indicator.
    SessionInfo {
        message_count: usize,
        /// Ever-increasing turn number for this session (never reset by compaction).
        #[serde(default)]
        turn_count: usize,
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
    },
    /// The approved background command requires a credential (sudo password, etc.).
    /// The client MUST prompt the user with echo disabled and return a `CredentialResponse`.
    CredentialPrompt { id: String, prompt: String },
    /// The output captured after an approved tool call completes.
    /// Sent to the client so it can display a dimmed result block.
    ToolResult(String),
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
    /// Each entry is `(pane_id, current_cmd, window_name, pane_index, is_current_target)`.
    PaneList {
        panes: Vec<(String, String, String, usize, bool)>,
    },
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
    /// AI-generated diff summary between two named sessions (response to `DiffSessions`).
    SessionDiff { summary: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_req(req: &Request) -> Request {
        let json = serde_json::to_string(req).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    fn roundtrip_resp(resp: &Response) -> Response {
        let json = serde_json::to_string(resp).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    // ── Request round-trips ──────────────────────────────────────────────────

    #[test]
    fn request_ping_roundtrip() {
        assert!(matches!(roundtrip_req(&Request::Ping), Request::Ping));
    }

    #[test]
    fn request_shutdown_roundtrip() {
        assert!(matches!(
            roundtrip_req(&Request::Shutdown),
            Request::Shutdown
        ));
    }

    #[test]
    fn request_refresh_roundtrip() {
        assert!(matches!(roundtrip_req(&Request::Refresh), Request::Refresh));
    }

    #[test]
    fn request_ask_roundtrip() {
        let req = Request::Ask {
            query: "what is load avg?".to_string(),
            tmux_pane: Some("%3".to_string()),
            session_id: Some("deadbeef".to_string()),
            chat_pane: Some("%4".to_string()),
            prompt: Some("sre".to_string()),
            chat_width: Some(54),
            tmux_session: Some("mysession".to_string()),
            target_pane: Some("%1".to_string()),
            model: Some("opus".to_string()),
        };
        match roundtrip_req(&req) {
            Request::Ask {
                query,
                tmux_pane,
                session_id,
                chat_pane,
                prompt,
                chat_width,
                tmux_session,
                target_pane,
                model,
            } => {
                assert_eq!(query, "what is load avg?");
                assert_eq!(tmux_pane, Some("%3".to_string()));
                assert_eq!(session_id, Some("deadbeef".to_string()));
                assert_eq!(chat_pane, Some("%4".to_string()));
                assert_eq!(prompt, Some("sre".to_string()));
                assert_eq!(chat_width, Some(54));
                assert_eq!(tmux_session, Some("mysession".to_string()));
                assert_eq!(target_pane, Some("%1".to_string()));
                assert_eq!(model, Some("opus".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_ask_optional_fields_none() {
        let req = Request::Ask {
            query: "hi".to_string(),
            tmux_pane: None,
            session_id: None,
            chat_pane: None,
            prompt: None,
            chat_width: None,
            tmux_session: None,
            target_pane: None,
            model: None,
        };
        match roundtrip_req(&req) {
            Request::Ask {
                tmux_pane,
                session_id,
                chat_pane,
                prompt,
                chat_width,
                ..
            } => {
                assert!(tmux_pane.is_none());
                assert!(session_id.is_none());
                assert!(chat_pane.is_none());
                assert!(prompt.is_none());
                assert!(chat_width.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_ask_backward_compat_no_prompt_field() {
        // Simulate a message from an old client that omits the `prompt` field.
        let json = r#"{"Ask":{"query":"hi","tmux_pane":null,"session_id":null,"chat_pane":null}}"#;
        let parsed: Request = serde_json::from_str(json).expect("backward-compat deserialize");
        match parsed {
            Request::Ask { prompt, .. } => assert!(prompt.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_tool_call_response_roundtrip() {
        let req = Request::ToolCallResponse {
            id: "tc_1".to_string(),
            approved: true,
            user_message: None,
        };
        match roundtrip_req(&req) {
            Request::ToolCallResponse {
                id,
                approved,
                user_message,
            } => {
                assert_eq!(id, "tc_1");
                assert!(approved);
                assert!(user_message.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_tool_call_response_with_user_message_roundtrip() {
        let req = Request::ToolCallResponse {
            id: "tc_2".to_string(),
            approved: false,
            user_message: Some("don't do that, try a safer approach".to_string()),
        };
        match roundtrip_req(&req) {
            Request::ToolCallResponse {
                id,
                approved,
                user_message,
            } => {
                assert_eq!(id, "tc_2");
                assert!(!approved);
                assert_eq!(
                    user_message.as_deref(),
                    Some("don't do that, try a safer approach")
                );
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_tool_call_response_backward_compat_no_user_message() {
        // Old clients omit user_message; default should be None.
        let json = r#"{"ToolCallResponse":{"id":"tc_3","approved":false}}"#;
        let parsed: Request = serde_json::from_str(json).expect("backward-compat deserialize");
        match parsed {
            Request::ToolCallResponse { user_message, .. } => assert!(user_message.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_credential_response_roundtrip() {
        let req = Request::CredentialResponse {
            id: "tc_2".to_string(),
            credential: "hunter2".to_string(),
        };
        match roundtrip_req(&req) {
            Request::CredentialResponse { id, credential } => {
                assert_eq!(id, "tc_2");
                assert_eq!(credential, "hunter2");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_activity_roundtrip() {
        let req = Request::NotifyActivity {
            pane_id: "%3".to_string(),
            hook_index: 42,
            session_name: "test_session".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyActivity { pane_id, .. } => {
                assert_eq!(pane_id, "%3");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_complete_roundtrip() {
        let req = Request::NotifyComplete {
            pane_id: "%5".to_string(),
            exit_code: 42,
            session_name: "test_session".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyComplete {
                pane_id, exit_code, ..
            } => {
                assert_eq!(pane_id, "%5");
                assert_eq!(exit_code, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Response round-trips ─────────────────────────────────────────────────

    #[test]
    fn response_ok_roundtrip() {
        assert!(matches!(roundtrip_resp(&Response::Ok), Response::Ok));
    }

    #[test]
    fn response_error_roundtrip() {
        let resp = Response::Error("something broke".to_string());
        match roundtrip_resp(&resp) {
            Response::Error(msg) => assert_eq!(msg, "something broke"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_session_info_roundtrip() {
        let resp = Response::SessionInfo {
            message_count: 7,
            turn_count: 3,
        };
        match roundtrip_resp(&resp) {
            Response::SessionInfo {
                message_count,
                turn_count,
            } => {
                assert_eq!(message_count, 7);
                assert_eq!(turn_count, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_token_roundtrip() {
        let resp = Response::Token("Hello".to_string());
        match roundtrip_resp(&resp) {
            Response::Token(t) => assert_eq!(t, "Hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_tool_call_prompt_roundtrip() {
        let resp = Response::ToolCallPrompt {
            id: "tc_3".to_string(),
            command: "ls -la".to_string(),
            background: false,
            target_pane: Some("%5".to_string()),
        };
        match roundtrip_resp(&resp) {
            Response::ToolCallPrompt {
                id,
                command,
                background,
                target_pane,
            } => {
                assert_eq!(id, "tc_3");
                assert_eq!(command, "ls -la");
                assert!(!background);
                assert_eq!(target_pane, Some("%5".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_tool_call_prompt_no_target_pane_roundtrip() {
        // Older daemons omit target_pane; default should be None.
        let json = r#"{"ToolCallPrompt":{"id":"tc_3","command":"ls -la","background":false}}"#;
        let parsed: Response = serde_json::from_str(json).expect("backward-compat deserialize");
        match parsed {
            Response::ToolCallPrompt { target_pane, .. } => assert!(target_pane.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_credential_prompt_roundtrip() {
        let resp = Response::CredentialPrompt {
            id: "tc_4".to_string(),
            prompt: "[sudo] password for alice:".to_string(),
        };
        match roundtrip_resp(&resp) {
            Response::CredentialPrompt { id, prompt } => {
                assert_eq!(id, "tc_4");
                assert_eq!(prompt, "[sudo] password for alice:");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_tool_result_roundtrip() {
        let resp = Response::ToolResult("output here".to_string());
        match roundtrip_resp(&resp) {
            Response::ToolResult(s) => assert_eq!(s, "output here"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_pane_select_response_roundtrip() {
        let req = Request::PaneSelectResponse {
            id: "ps_1".to_string(),
            pane_id: "%3".to_string(),
        };
        match roundtrip_req(&req) {
            Request::PaneSelectResponse { id, pane_id } => {
                assert_eq!(id, "ps_1");
                assert_eq!(pane_id, "%3");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_pane_select_prompt_roundtrip() {
        let resp = Response::PaneSelectPrompt {
            id: "ps_2".to_string(),
            panes: vec![
                PaneInfo {
                    id: "%1".to_string(),
                    current_cmd: "bash".to_string(),
                    summary: "idle shell".to_string(),
                },
                PaneInfo {
                    id: "%3".to_string(),
                    current_cmd: "vim".to_string(),
                    summary: "editing file".to_string(),
                },
            ],
        };
        match roundtrip_resp(&resp) {
            Response::PaneSelectPrompt { id, panes } => {
                assert_eq!(id, "ps_2");
                assert_eq!(panes.len(), 2);
                assert_eq!(panes[0].id, "%1");
                assert_eq!(panes[0].current_cmd, "bash");
                assert_eq!(panes[1].id, "%3");
                assert_eq!(panes[1].current_cmd, "vim");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_script_write_response_roundtrip() {
        let req = Request::ScriptWriteResponse {
            id: "sw_1".to_string(),
            approved: true,
        };
        match roundtrip_req(&req) {
            Request::ScriptWriteResponse { id, approved } => {
                assert_eq!(id, "sw_1");
                assert!(approved);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_script_write_prompt_roundtrip() {
        // New file: no existing content
        let resp = Response::ScriptWritePrompt {
            id: "sw_2".to_string(),
            script_name: "check-disk.sh".to_string(),
            content: "#!/bin/bash\ndf -h".to_string(),
            existing_content: None,
        };
        match roundtrip_resp(&resp) {
            Response::ScriptWritePrompt {
                id,
                script_name,
                content,
                existing_content,
            } => {
                assert_eq!(id, "sw_2");
                assert_eq!(script_name, "check-disk.sh");
                assert!(content.contains("df -h"));
                assert!(existing_content.is_none());
            }
            _ => panic!("wrong variant"),
        }

        // Modified file: existing content provided
        let resp2 = Response::ScriptWritePrompt {
            id: "sw_3".to_string(),
            script_name: "check-disk.sh".to_string(),
            content: "#!/bin/bash\ndf -h\necho done".to_string(),
            existing_content: Some("#!/bin/bash\ndf -h".to_string()),
        };
        match roundtrip_resp(&resp2) {
            Response::ScriptWritePrompt {
                existing_content, ..
            } => {
                assert!(existing_content.is_some());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_runbook_write_prompt_roundtrip() {
        // New runbook: no existing content
        let resp = Response::RunbookWritePrompt {
            id: "rw_1".to_string(),
            runbook_name: "disk-alert".to_string(),
            content: "# Runbook: disk-alert\n## Alert Criteria\ndf -h".to_string(),
            existing_content: None,
        };
        match roundtrip_resp(&resp) {
            Response::RunbookWritePrompt {
                id,
                runbook_name,
                content,
                existing_content,
            } => {
                assert_eq!(id, "rw_1");
                assert_eq!(runbook_name, "disk-alert");
                assert!(content.contains("df -h"));
                assert!(existing_content.is_none());
            }
            _ => panic!("wrong variant"),
        }

        // Modified runbook: existing content provided
        let resp2 = Response::RunbookWritePrompt {
            id: "rw_2".to_string(),
            runbook_name: "disk-alert".to_string(),
            content: "# Runbook: disk-alert\n## Alert Criteria\ndf -h\nnew line".to_string(),
            existing_content: Some("# Runbook: disk-alert\n## Alert Criteria\ndf -h".to_string()),
        };
        match roundtrip_resp(&resp2) {
            Response::RunbookWritePrompt {
                existing_content, ..
            } => {
                assert!(existing_content.is_some());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_schedule_write_response_roundtrip() {
        let req = Request::ScheduleWriteResponse {
            id: "sch_1".to_string(),
            approved: true,
        };
        match roundtrip_req(&req) {
            Request::ScheduleWriteResponse { id, approved } => {
                assert_eq!(id, "sch_1");
                assert!(approved);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_schedule_write_prompt_roundtrip() {
        let resp = Response::ScheduleWritePrompt {
            id: "sch_2".to_string(),
            name: "MyJob".to_string(),
            kind: "every 5m".to_string(),
            action: "echo Hello".to_string(),
        };
        match roundtrip_resp(&resp) {
            Response::ScheduleWritePrompt {
                id,
                name,
                kind,
                action,
            } => {
                assert_eq!(id, "sch_2");
                assert_eq!(name, "MyJob");
                assert_eq!(kind, "every 5m");
                assert_eq!(action, "echo Hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_schedule_list_roundtrip() {
        let resp = Response::ScheduleList {
            jobs: vec![ScheduleListItem {
                id: "job-1".to_string(),
                name: "disk-check".to_string(),
                kind: "every 5m".to_string(),
                action: "cmd: df -h".to_string(),
                status: "pending".to_string(),
                last_run: None,
                next_run: Some("2026-03-01 12:00 UTC".to_string()),
            }],
        };
        match roundtrip_resp(&resp) {
            Response::ScheduleList { jobs } => {
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].name, "disk-check");
                assert_eq!(jobs[0].next_run, Some("2026-03-01 12:00 UTC".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_script_list_roundtrip() {
        let resp = Response::ScriptList {
            scripts: vec![
                ScriptListItem {
                    name: "check-disk.sh".to_string(),
                    size: 42,
                },
                ScriptListItem {
                    name: "monitor.sh".to_string(),
                    size: 128,
                },
            ],
        };
        match roundtrip_resp(&resp) {
            Response::ScriptList { scripts } => {
                assert_eq!(scripts.len(), 2);
                assert_eq!(scripts[0].name, "check-disk.sh");
                assert_eq!(scripts[0].size, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_client_attached_roundtrip() {
        let req = Request::NotifyClientAttached {
            session_name: "dev".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyClientAttached { session_name } => assert_eq!(session_name, "dev"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_client_detached_roundtrip() {
        let req = Request::NotifyClientDetached {
            session_name: "staging".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyClientDetached { session_name } => assert_eq!(session_name, "staging"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_session_closed_roundtrip() {
        let req = Request::NotifySessionClosed {
            session_name: "prod".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifySessionClosed { session_name } => assert_eq!(session_name, "prod"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_resize_roundtrip() {
        let req = Request::NotifyResize {
            width: 220,
            height: 50,
            session_name: "main".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyResize {
                width,
                height,
                session_name,
            } => {
                assert_eq!(width, 220);
                assert_eq!(height, 50);
                assert_eq!(session_name, "main");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_status_roundtrip() {
        assert!(matches!(roundtrip_req(&Request::Status), Request::Status));
    }

    #[test]
    fn response_daemon_status_roundtrip() {
        let mut memory_breakdown = std::collections::HashMap::new();
        memory_breakdown.insert("knowledge".to_string(), 3);
        memory_breakdown.insert("incident".to_string(), 1);

        let resp = Response::DaemonStatus {
            uptime_secs: 3661,
            pid: 12345,
            active_sessions: 2,
            total_turns: 42,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            available_models: vec!["default".to_string(), "opus".to_string()],
            socket_path: "/tmp/daemoneye.sock".to_string(),
            schedule_count: 3,
            commands_fg_succeeded: 5,
            commands_fg_failed: 1,
            commands_fg_approved: 6,
            commands_fg_denied: 2,
            commands_bg_succeeded: 3,
            commands_bg_failed: 1,
            commands_bg_approved: 4,
            commands_bg_denied: 1,
            commands_sched_succeeded: 2,
            commands_sched_failed: 0,
            webhooks_received: 5,
            webhooks_rejected: 1,
            webhook_url: "http://127.0.0.1:8000/webhook".to_string(),
            runbook_count: 2,
            runbooks_created: 1,
            runbooks_executed: 4,
            runbooks_deleted: 0,
            script_count: 3,
            scripts_created: 2,
            scripts_executed: 6,
            scripts_deleted: 1,
            memories_created: 3,
            memories_recalled: 7,
            memories_deleted: 1,
            schedules_created: 2,
            schedules_executed: 5,
            schedules_deleted: 0,
            ghosts_launched: 1,
            ghosts_active: 0,
            ghosts_completed: 1,
            ghosts_failed: 0,
            active_prompt_tokens: 1000,
            context_window_tokens: 4000,
            recent_commands: vec![RecentCommand {
                id: 1,
                cmd: "ls".to_string(),
                timestamp: "2026-03-20 12:00:00".to_string(),
                mode: "foreground".to_string(),
                approval: "approved".to_string(),
                status: "succeeded".to_string(),
            }],
            memory_breakdown: memory_breakdown.clone(),
            redaction_counts: {
                let mut m = std::collections::HashMap::new();
                m.insert("JWT".to_string(), 3);
                m.insert("Secret".to_string(), 1);
                m
            },
            compactions: 2,
            compaction_ratio: 3.5,
            scripts_approved: 0,
            scripts_denied: 0,
            runbooks_approved: 0,
            runbooks_denied: 0,
            file_edits_approved: 0,
            file_edits_denied: 0,
            limits: LimitsSummary::default(),
        };
        match roundtrip_resp(&resp) {
            Response::DaemonStatus {
                uptime_secs,
                pid,
                active_sessions,
                provider,
                model,
                schedule_count,
                commands_fg_succeeded,
                commands_fg_failed,
                commands_bg_succeeded,
                commands_bg_failed,
                commands_sched_succeeded,
                commands_sched_failed,
                webhooks_received,
                webhooks_rejected,
                webhook_url,
                runbooks_created,
                runbooks_executed,
                runbooks_deleted,
                scripts_created,
                scripts_executed,
                memories_created,
                memories_recalled,
                memories_deleted,
                schedules_created,
                schedules_executed,
                schedules_deleted,
                active_prompt_tokens,
                context_window_tokens,
                recent_commands,
                memory_breakdown: mb,
                redaction_counts: rc,
                ..
            } => {
                assert_eq!(uptime_secs, 3661);
                assert_eq!(pid, 12345);
                assert_eq!(active_sessions, 2);
                assert_eq!(provider, "anthropic");
                assert_eq!(model, "claude-sonnet-4-6");
                assert_eq!(schedule_count, 3);
                assert_eq!(commands_fg_succeeded, 5);
                assert_eq!(commands_fg_failed, 1);
                assert_eq!(commands_bg_succeeded, 3);
                assert_eq!(commands_bg_failed, 1);
                assert_eq!(commands_sched_succeeded, 2);
                assert_eq!(commands_sched_failed, 0);
                assert_eq!(webhooks_received, 5);
                assert_eq!(webhooks_rejected, 1);
                assert_eq!(webhook_url, "http://127.0.0.1:8000/webhook");
                assert_eq!(runbooks_created, 1);
                assert_eq!(runbooks_executed, 4);
                assert_eq!(runbooks_deleted, 0);
                assert_eq!(scripts_created, 2);
                assert_eq!(scripts_executed, 6);
                assert_eq!(memories_created, 3);
                assert_eq!(memories_recalled, 7);
                assert_eq!(memories_deleted, 1);
                assert_eq!(schedules_created, 2);
                assert_eq!(schedules_executed, 5);
                assert_eq!(schedules_deleted, 0);
                assert_eq!(active_prompt_tokens, 1000);
                assert_eq!(context_window_tokens, 4000);
                assert_eq!(recent_commands.len(), 1);
                assert_eq!(mb.len(), 2);
                assert_eq!(rc.get("JWT").copied().unwrap_or(0), 3);
                assert_eq!(rc.get("Secret").copied().unwrap_or(0), 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn invalid_json_returns_error() {
        let result: Result<Request, _> = serde_json::from_str("not json at all");
        assert!(result.is_err());
    }
}
