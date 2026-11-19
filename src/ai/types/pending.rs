use super::AiEvent;

/// A tool call collected during AI streaming, to be executed after `Done`.
#[derive(Debug)]
pub enum PendingCall {
    Foreground {
        id: String,
        thought_signature: Option<String>,
        cmd: String,
        shell: Option<String>,
        host: Option<String>,
        timeout_secs: Option<u64>,
        /// `run_terminal_command(interactive: true)` (03e): take the launch
        /// path — type the command, return the settled screen — instead of
        /// waiting on the command. Only `Foreground` carries it.
        interactive: bool,
    },
    Background {
        id: String,
        thought_signature: Option<String>,
        cmd: String,
        _credential: Option<String>,
        host: Option<String>,
    },
    ScheduleCommand {
        id: String,
        thought_signature: Option<String>,
        name: String,
        command: String,
        is_script: bool,
        run_at: Option<String>,
        interval: Option<String>,
        runbook: Option<String>,
        /// When set, schedule a Ghost Shell job using this runbook instead of
        /// running a raw command or script.  Mutually exclusive with `command`/`is_script`.
        ghost_runbook: Option<String>,
        /// 5-field cron expression (e.g. `*/5 * * * *`). Mutually exclusive with `interval`.
        cron: Option<String>,
    },
    /// A call to a known tool whose arguments failed schema validation
    /// (M27 phase-07d, F10). The raw arguments are kept so history shows the
    /// call exactly as the model made it; the executor returns the listed
    /// mistakes as the tool result and runs nothing.
    InvalidArgs {
        id: String,
        name: &'static str,
        arguments: String,
        error: String,
        thought_signature: Option<String>,
    },
    /// Load a deferred group of tools into the active set for this session.
    LoadTools {
        id: String,
        groups: Vec<String>,
        thought_signature: Option<String>,
    },
    ListSchedules {
        id: String,
        thought_signature: Option<String>,
    },
    CancelSchedule {
        id: String,
        thought_signature: Option<String>,
        job_id: String,
    },
    DeleteSchedule {
        id: String,
        thought_signature: Option<String>,
        job_id: String,
    },
    WriteScript {
        id: String,
        thought_signature: Option<String>,
        script_name: String,
        content: String,
    },
    ListScripts {
        id: String,
        thought_signature: Option<String>,
    },
    ReadScript {
        id: String,
        thought_signature: Option<String>,
        script_name: String,
    },
    DeleteScript {
        id: String,
        thought_signature: Option<String>,
        script_name: String,
    },
    ReadFile {
        id: String,
        thought_signature: Option<String>,
        path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pattern: Option<String>,
        host: Option<String>,
    },
    EditFile {
        id: String,
        thought_signature: Option<String>,
        path: String,
        operation: String,
        old_string: Option<String>,
        new_string: Option<String>,
        content: Option<String>,
        dest_path: Option<String>,
        host: Option<String>,
        sudo: Option<bool>,
    },
    WriteRunbook {
        id: String,
        thought_signature: Option<String>,
        name: String,
        content: String,
    },
    DeleteRunbook {
        id: String,
        thought_signature: Option<String>,
        name: String,
    },
    ReadRunbook {
        id: String,
        thought_signature: Option<String>,
        name: String,
    },
    ListRunbooks {
        id: String,
        thought_signature: Option<String>,
    },
    AddMemory {
        id: String,
        thought_signature: Option<String>,
        key: String,
        value: String,
        category: String,
    },
    UpdateMemory {
        id: String,
        thought_signature: Option<String>,
        key: String,
        category: String,
        body: Option<String>,
        append: bool,
        tags: Option<Vec<String>>,
        summary: Option<String>,
        relates_to: Option<Vec<String>>,
        expires: Option<String>,
    },
    DeleteMemory {
        id: String,
        thought_signature: Option<String>,
        key: String,
        category: String,
    },
    ReadMemory {
        id: String,
        thought_signature: Option<String>,
        key: String,
        category: String,
    },
    ListMemories {
        id: String,
        thought_signature: Option<String>,
        category: Option<String>,
    },
    SearchRepository {
        id: String,
        thought_signature: Option<String>,
        query: String,
        kind: String,
    },
    RecallContext {
        id: String,
        thought_signature: Option<String>,
        query: Option<String>,
        turn_start: Option<u32>,
        turn_end: Option<u32>,
        scope: Option<String>,
    },
    GetShellContext {
        id: String,
        thought_signature: Option<String>,
        scope: Option<String>,
    },
    ListShells {
        id: String,
        thought_signature: Option<String>,
        scope: Option<String>,
    },
    /// Spawn an autonomous Ghost Shell session in the background.
    SpawnGhost {
        id: String,
        thought_signature: Option<String>,
        /// Name of the runbook in `~/.daemoneye/runbooks/` that governs the ghost.
        runbook: String,
        /// Human-readable description of the problem to investigate.
        message: String,
        /// Optional named agent to use as the executor identity.
        agent: Option<String>,
        /// Host the specialist's commands run on, overriding the runbook's
        /// `ssh_target`: a configured `[hosts]` key or `"local"`.
        host: Option<String>,
    },
    /// Create or update a named agent config.
    CreateAgent {
        id: String,
        thought_signature: Option<String>,
        name: String,
        description: String,
        prompt: String,
        model: Option<String>,
        memory_namespace: String,
        max_turns: Option<u32>,
        auto_approve_read_only: bool,
        auto_approve_scripts: Vec<String>,
    },
    /// Read a named agent config.
    ReadAgent {
        id: String,
        thought_signature: Option<String>,
        name: String,
    },
    /// List all named agents.
    ListAgents {
        id: String,
        thought_signature: Option<String>,
    },
    /// Delete a named agent.
    DeleteAgent {
        id: String,
        thought_signature: Option<String>,
        name: String,
    },
    /// Wait for a spawned agent ghost shell to complete and return its result.
    AwaitAgentResult {
        id: String,
        thought_signature: Option<String>,
        job_id: String,
        agent_name: String,
        timeout_secs: u64,
    },
    /// Read the last N records of one run's turn log (M24 phase-02b).
    RunLogTail {
        id: String,
        thought_signature: Option<String>,
        run_id: String,
        limit: Option<u64>,
    },
    /// Search one run's turn log for records containing a query substring.
    RunLogSearch {
        id: String,
        thought_signature: Option<String>,
        run_id: String,
        query: String,
        limit: Option<u64>,
    },
    /// Read every record one turn logged — the uncapped escape hatch.
    GetTurn {
        id: String,
        thought_signature: Option<String>,
        run_id: String,
        turn: u32,
    },
    /// Read a shell's recorded command output from its asciicast log (M22
    /// phase-03b). Reads the **log**, not the live screen — there is no
    /// protocol frame for a screen read.
    ReadShell {
        id: String,
        thought_signature: Option<String>,
        shell: String,
        lines: Option<u64>,
        since_command: Option<u32>,
        filter: Option<String>,
    },
    /// Read a shell's live rendered screen (M28 phase 01c): the viewport as
    /// of the call, with a header naming what holds the terminal. Lossy by
    /// design; `ReadShell` reads the record. 03a adds the wait parameters —
    /// block until the screen shows `wait_for` or goes quiet for
    /// `settle_ms`, bounded by `timeout_secs`.
    ReadShellScreen {
        id: String,
        thought_signature: Option<String>,
        shell: String,
        lines: Option<u64>,
        scrollback: Option<u16>,
        pattern: Option<String>,
        annotated: bool,
        wait_for: Option<String>,
        settle_ms: Option<u64>,
        timeout_secs: Option<u64>,
    },
    /// Search every in-scope shell's recorded output for a regex (M22
    /// phase-03c). Reads the **recordings**, like `read_shell` — not live
    /// screens; shells whose recording cannot be read are skipped and named.
    FindInShells {
        id: String,
        thought_signature: Option<String>,
        pattern: String,
        scope: Option<String>,
    },
    /// Open a new PTY shell for this session (M22 phase-04a). An empty `host`
    /// opens the shell on the daemon host; a named one must be a configured
    /// `[hosts.<name>]` key (M21 phase-04b), which `open_shell` resolves
    /// before spawning. M28 phase-03c adds `rows`/`cols`: the size the shell
    /// is opened at, `None` for the configured `[shells]` size.
    OpenShell {
        id: String,
        thought_signature: Option<String>,
        host: Option<String>,
        rows: Option<u64>,
        cols: Option<u64>,
    },
    /// Close one of this session's PTY shells (M22 phase-04a). Sends Term and
    /// reports what it observed; there is no kill frame on this wire.
    CloseShell {
        id: String,
        thought_signature: Option<String>,
        shell: String,
    },
    /// Block on a PTY shell's arriving output (M22 phase-04b): with a
    /// `pattern`, until output written *after the call* matches it; without
    /// one, until the shell's running command finishes (or is found absent,
    /// paused, exited); or until `timeout_secs` elapses. Reads the live
    /// `.cast` recording, never the command index — nothing in production
    /// writes that index.
    WatchShell {
        id: String,
        thought_signature: Option<String>,
        shell: String,
        timeout_secs: u64,
        pattern: Option<String>,
    },
    /// Write text, named keys, or both into a live PTY shell's input (M22
    /// phase-05; M28 phase-02e adds `keys` and `paste`). Approval-gated on
    /// every call; `enter` appends the carriage return (true only by default
    /// for a text-only send).
    SendInput {
        id: String,
        thought_signature: Option<String>,
        shell: String,
        text: String,
        keys: Vec<String>,
        paste: Option<bool>,
        enter: bool,
    },
    /// List the shell recordings on disk (M22 phase-06b) — the deferred
    /// `history` group — newest first, each row carrying the recording's
    /// **log id** (`<shell id>-<started unix>`).
    ListShellLogs {
        id: String,
        thought_signature: Option<String>,
        since: Option<u64>,
        limit: Option<u64>,
    },
    /// Read one shell recording by its **log id** (M22 phase-06b). The log id
    /// is the filename stem — not the bare shell id, which is reused across
    /// daemon restarts and refused here.
    ReadShellLog {
        id: String,
        thought_signature: Option<String>,
        log: String,
        command_index: Option<u32>,
        lines: Option<u64>,
        filter: Option<String>,
    },
}

macro_rules! pending_field {
    ($self:expr, $field:ident) => {
        match $self {
            PendingCall::Foreground { $field, .. }
            | PendingCall::Background { $field, .. }
            | PendingCall::ScheduleCommand { $field, .. }
            | PendingCall::InvalidArgs { $field, .. }
            | PendingCall::ListSchedules { $field, .. }
            | PendingCall::LoadTools { $field, .. }
            | PendingCall::CancelSchedule { $field, .. }
            | PendingCall::DeleteSchedule { $field, .. }
            | PendingCall::WriteScript { $field, .. }
            | PendingCall::ListScripts { $field, .. }
            | PendingCall::ReadScript { $field, .. }
            | PendingCall::DeleteScript { $field, .. }
            | PendingCall::WatchShell { $field, .. }
            | PendingCall::SendInput { $field, .. }
            | PendingCall::ListShellLogs { $field, .. }
            | PendingCall::ReadShellLog { $field, .. }
            | PendingCall::ReadFile { $field, .. }
            | PendingCall::EditFile { $field, .. }
            | PendingCall::WriteRunbook { $field, .. }
            | PendingCall::DeleteRunbook { $field, .. }
            | PendingCall::ReadRunbook { $field, .. }
            | PendingCall::ListRunbooks { $field, .. }
            | PendingCall::AddMemory { $field, .. }
            | PendingCall::UpdateMemory { $field, .. }
            | PendingCall::DeleteMemory { $field, .. }
            | PendingCall::ReadMemory { $field, .. }
            | PendingCall::ListMemories { $field, .. }
            | PendingCall::SearchRepository { $field, .. }
            | PendingCall::RecallContext { $field, .. }
            | PendingCall::GetShellContext { $field, .. }
            | PendingCall::ListShells { $field, .. }
            | PendingCall::OpenShell { $field, .. }
            | PendingCall::CloseShell { $field, .. }
            | PendingCall::SpawnGhost { $field, .. }
            | PendingCall::CreateAgent { $field, .. }
            | PendingCall::ReadAgent { $field, .. }
            | PendingCall::ListAgents { $field, .. }
            | PendingCall::DeleteAgent { $field, .. }
            | PendingCall::AwaitAgentResult { $field, .. }
            | PendingCall::ReadShell { $field, .. }
            | PendingCall::ReadShellScreen { $field, .. }
            | PendingCall::FindInShells { $field, .. }
            | PendingCall::RunLogTail { $field, .. }
            | PendingCall::RunLogSearch { $field, .. }
            | PendingCall::GetTurn { $field, .. } => $field,
        }
    };
}

impl PendingCall {
    pub fn id(&self) -> &str {
        pending_field!(self, id)
    }

    pub fn thought_signature(&self) -> Option<&str> {
        pending_field!(self, thought_signature).as_deref()
    }

    /// Returns true for silent tools that have no approval prompt or list-panel
    /// response — the executor should emit `ToolStarted`/`ToolFinished` for these.
    pub fn should_emit_tool_feedback(&self) -> bool {
        matches!(
            self,
            PendingCall::CancelSchedule { .. }
                | PendingCall::DeleteSchedule { .. }
                | PendingCall::ReadScript { .. }
                | PendingCall::WatchShell { .. }
                | PendingCall::ReadFile { .. }
                | PendingCall::ReadRunbook { .. }
                | PendingCall::AddMemory { .. }
                | PendingCall::UpdateMemory { .. }
                | PendingCall::DeleteMemory { .. }
                | PendingCall::ReadMemory { .. }
                | PendingCall::ListMemories { .. }
                | PendingCall::SearchRepository { .. }
                | PendingCall::RecallContext { .. }
                | PendingCall::GetShellContext { .. }
                | PendingCall::ListShells { .. }
                | PendingCall::SpawnGhost { .. }
                | PendingCall::ReadAgent { .. }
                | PendingCall::ListAgents { .. }
                | PendingCall::AwaitAgentResult { .. }
                | PendingCall::ReadShell { .. }
                | PendingCall::ReadShellScreen { .. }
                | PendingCall::FindInShells { .. }
                | PendingCall::OpenShell { .. }
                | PendingCall::CloseShell { .. }
                | PendingCall::LoadTools { .. }
                | PendingCall::RunLogTail { .. }
                | PendingCall::RunLogSearch { .. }
                | PendingCall::GetTurn { .. }
                | PendingCall::ListShellLogs { .. }
                | PendingCall::ReadShellLog { .. }
        )
    }

    /// Returns the canonical tool name for this call, used for per-turn rate limiting.
    pub fn tool_name(&self) -> &'static str {
        match self {
            PendingCall::Foreground { .. } | PendingCall::Background { .. } => {
                "run_terminal_command"
            }
            PendingCall::ScheduleCommand { .. } => "schedule_command",
            PendingCall::ListSchedules { .. } => "list_schedules",
            PendingCall::LoadTools { .. } => "load_tools",
            PendingCall::CancelSchedule { .. } => "cancel_schedule",
            PendingCall::DeleteSchedule { .. } => "delete_schedule",
            PendingCall::WriteScript { .. } => "write_script",
            PendingCall::ListScripts { .. } => "list_scripts",
            PendingCall::ReadScript { .. } => "read_script",
            PendingCall::DeleteScript { .. } => "delete_script",
            PendingCall::WatchShell { .. } => "watch_shell",
            PendingCall::SendInput { .. } => "send_input",
            PendingCall::ListShellLogs { .. } => "list_shell_logs",
            PendingCall::ReadShellLog { .. } => "read_shell_log",
            PendingCall::ReadFile { .. } => "read_file",
            PendingCall::EditFile { .. } => "edit_file",
            PendingCall::WriteRunbook { .. } => "write_runbook",
            PendingCall::DeleteRunbook { .. } => "delete_runbook",
            PendingCall::ReadRunbook { .. } => "read_runbook",
            PendingCall::ListRunbooks { .. } => "list_runbooks",
            PendingCall::AddMemory { .. } => "add_memory",
            PendingCall::UpdateMemory { .. } => "update_memory",
            PendingCall::DeleteMemory { .. } => "delete_memory",
            PendingCall::ReadMemory { .. } => "read_memory",
            PendingCall::ListMemories { .. } => "list_memories",
            PendingCall::SearchRepository { .. } => "search_repository",
            PendingCall::RecallContext { .. } => "recall_context",
            PendingCall::GetShellContext { .. } => "get_shell_context",
            PendingCall::ListShells { .. } => "list_shells",
            PendingCall::OpenShell { .. } => "open_shell",
            PendingCall::CloseShell { .. } => "close_shell",
            PendingCall::SpawnGhost { .. } => "spawn_ghost_shell",
            PendingCall::CreateAgent { .. } => "create_agent",
            PendingCall::ReadAgent { .. } => "read_agent",
            PendingCall::ListAgents { .. } => "list_agents",
            PendingCall::DeleteAgent { .. } => "delete_agent",
            PendingCall::AwaitAgentResult { .. } => "await_agent_result",
            PendingCall::ReadShell { .. } => "read_shell",
            PendingCall::ReadShellScreen { .. } => "read_shell_screen",
            PendingCall::FindInShells { .. } => "find_in_shells",
            PendingCall::RunLogTail { .. } => "run_log_tail",
            PendingCall::RunLogSearch { .. } => "run_log_search",
            PendingCall::GetTurn { .. } => "get_turn",
            PendingCall::InvalidArgs { name, .. } => name,
        }
    }
}

impl PendingCall {
    /// Extract the [`PendingCall`] from a tool [`AiEvent`]. Returns `None` for
    /// `run_terminal_command` (`AiEvent::ToolCall`, whose `wait` and
    /// `interactive` arguments are routed by the chat and ghost loops) and for
    /// non-tool stream events.
    pub(crate) fn from_tool_event(event: AiEvent) -> Option<PendingCall> {
        match event {
            AiEvent::Tool(call) => Some(call),
            AiEvent::ToolCall { .. }
            | AiEvent::Token(_)
            | AiEvent::Error(_)
            | AiEvent::Notice(_)
            | AiEvent::Done(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_read_file(path: &str) -> PendingCall {
        PendingCall::ReadFile {
            id: "x".to_string(),
            thought_signature: None,
            path: path.to_string(),
            offset: None,
            limit: None,
            pattern: None,
            host: None,
        }
    }

    fn mk_foreground(cmd: &str) -> PendingCall {
        PendingCall::Foreground {
            id: "x".to_string(),
            thought_signature: None,
            cmd: cmd.to_string(),
            shell: None,
            host: None,
            timeout_secs: None,
            interactive: false,
        }
    }

    #[test]
    fn should_emit_tool_feedback_silent_tools_true() {
        // All 17 silent tools should return true.
        let cases: &[PendingCall] = &[
            PendingCall::CancelSchedule {
                id: "x".to_string(),
                thought_signature: None,
                job_id: "j1".to_string(),
            },
            PendingCall::DeleteSchedule {
                id: "x".to_string(),
                thought_signature: None,
                job_id: "j2".to_string(),
            },
            PendingCall::ReadScript {
                id: "x".to_string(),
                thought_signature: None,
                script_name: "s.sh".to_string(),
            },
            mk_read_file("/etc/hosts"),
            PendingCall::ReadRunbook {
                id: "x".to_string(),
                thought_signature: None,
                name: "rb".to_string(),
            },
            PendingCall::AddMemory {
                id: "x".to_string(),
                thought_signature: None,
                key: "k".to_string(),
                value: "v".to_string(),
                category: "knowledge".to_string(),
            },
            PendingCall::DeleteMemory {
                id: "x".to_string(),
                thought_signature: None,
                key: "k".to_string(),
                category: "knowledge".to_string(),
            },
            PendingCall::ReadMemory {
                id: "x".to_string(),
                thought_signature: None,
                key: "k".to_string(),
                category: "knowledge".to_string(),
            },
            PendingCall::ListMemories {
                id: "x".to_string(),
                thought_signature: None,
                category: None,
            },
            PendingCall::SearchRepository {
                id: "x".to_string(),
                thought_signature: None,
                query: "alert".to_string(),
                kind: "all".to_string(),
            },
            PendingCall::RecallContext {
                id: "x".to_string(),
                thought_signature: None,
                query: None,
                turn_start: None,
                turn_end: None,
                scope: None,
            },
            PendingCall::GetShellContext {
                id: "x".to_string(),
                thought_signature: None,
                scope: None,
            },
            PendingCall::ListShells {
                id: "x".to_string(),
                thought_signature: None,
                scope: None,
            },
            PendingCall::OpenShell {
                id: "x".to_string(),
                thought_signature: None,
                host: None,
                rows: None,
                cols: None,
            },
            PendingCall::CloseShell {
                id: "x".to_string(),
                thought_signature: None,
                shell: "s9".to_string(),
            },
            PendingCall::SpawnGhost {
                id: "x".to_string(),
                thought_signature: None,
                runbook: "disk-alert".to_string(),
                message: "investigate".to_string(),
                agent: None,
                host: None,
            },
        ];
        for call in cases {
            assert!(
                call.should_emit_tool_feedback(),
                "{} should emit tool feedback",
                call.tool_name()
            );
        }
    }

    #[test]
    fn should_emit_tool_feedback_approval_gated_tools_false() {
        // Approval-gated tools must NOT emit ToolStarted/ToolFinished — they have
        // their own richer ToolCallPrompt UI.
        let cases: &[PendingCall] = &[
            mk_foreground("ls -la"),
            PendingCall::Background {
                id: "x".to_string(),
                thought_signature: None,
                cmd: "cargo build".to_string(),
                _credential: None,
                host: None,
            },
            PendingCall::WriteScript {
                id: "x".to_string(),
                thought_signature: None,
                script_name: "check.sh".to_string(),
                content: "#!/bin/bash".to_string(),
            },
            PendingCall::EditFile {
                id: "x".to_string(),
                thought_signature: None,
                path: "/tmp/f".to_string(),
                operation: "edit".to_string(),
                old_string: None,
                new_string: None,
                content: None,
                dest_path: None,
                host: None,
                sudo: None,
            },
        ];
        for call in cases {
            assert!(
                !call.should_emit_tool_feedback(),
                "{} should NOT emit tool feedback",
                call.tool_name()
            );
        }
    }

    // ── summary() ────────────────────────────────────────────────────────

    #[test]
    fn summary_read_file_path_only() {
        assert_eq!(mk_read_file("/var/log/syslog").summary(), "/var/log/syslog");
    }

    #[test]
    fn summary_read_file_with_offset_and_limit() {
        let call = PendingCall::ReadFile {
            id: "x".to_string(),
            thought_signature: None,
            path: "/var/log/syslog".to_string(),
            offset: Some(100),
            limit: Some(50),
            pattern: None,
            host: None,
        };
        assert_eq!(call.summary(), "/var/log/syslog lines=100-150");
    }

    #[test]
    fn summary_read_file_with_grep() {
        let call = PendingCall::ReadFile {
            id: "x".to_string(),
            thought_signature: None,
            path: "/etc/hosts".to_string(),
            offset: None,
            limit: None,
            pattern: Some("nameserver".to_string()),
            host: None,
        };
        assert_eq!(call.summary(), "/etc/hosts grep=\"nameserver\"");
    }

    #[test]
    fn summary_search_repository_truncated() {
        let long_query = "a".repeat(60);
        let call = PendingCall::SearchRepository {
            id: "x".to_string(),
            thought_signature: None,
            query: long_query,
            kind: "all".to_string(),
        };
        let s = call.summary();
        // Should be truncated to 40 chars of query inside quotes.
        assert!(s.starts_with('"'));
        assert!(s.ends_with('"'));
        assert_eq!(s.len(), 42); // 40 chars + 2 quotes
    }

    #[test]
    fn summary_memory_key_category() {
        let call = PendingCall::ReadMemory {
            id: "x".to_string(),
            thought_signature: None,
            key: "api-key".to_string(),
            category: "knowledge".to_string(),
        };
        assert_eq!(call.summary(), "knowledge/api-key");
    }

    #[test]
    fn char_boundary_search_summary() {
        // B4: a query of 20 CJK chars is 60 bytes; the old byte slice at 40
        // panicked the client handler mid-turn.
        let query = "日".repeat(20);
        let call = PendingCall::SearchRepository {
            id: "x".to_string(),
            thought_signature: None,
            query,
            kind: "all".to_string(),
        };
        let s = call.summary();
        let inner = s
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .expect("quoted summary");
        assert!(
            call_query_prefix_of(inner),
            "summary must be a prefix of the query"
        );
        assert!(
            inner.chars().count() <= 20,
            "cut is by char, never past the query"
        );

        // ASCII still summarizes to its first 40 chars inside quotes.
        let ascii = "a".repeat(50);
        let call = PendingCall::SearchRepository {
            id: "x".to_string(),
            thought_signature: None,
            query: ascii,
            kind: "all".to_string(),
        };
        let s = call.summary();
        assert_eq!(s, format!("\"{}\"", "a".repeat(40)));
    }

    #[test]
    fn char_boundary_foreground_summary() {
        // B4: a 20-CJK-char cmd is 60 bytes; the old byte slice at 40 panicked.
        let cmd = "語".repeat(20);
        let prefix_of = cmd.clone();
        let call = PendingCall::Foreground {
            id: "x".to_string(),
            thought_signature: None,
            cmd,
            shell: None,
            host: None,
            timeout_secs: None,
            interactive: false,
        };
        let s = call.summary();
        let inner = s
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .expect("quoted summary");
        assert!(!inner.is_empty(), "cut keeps at least one char");
        assert!(
            prefix_of.starts_with(inner),
            "summary must be a prefix of the cmd"
        );
        assert!(
            inner.chars().count() <= 20,
            "cut is by char, never past the cmd"
        );

        // ASCII keeps its current shape.
        let ascii = "u".repeat(50);
        let call = mk_foreground(&ascii);
        assert_eq!(call.summary(), format!("\"{}\"", "u".repeat(40)));
    }

    /// A prefix of the CJK query: every char of the summary appears in order
    /// at the front of the query (a plain `starts_with` on the rebuilt
    /// String is what this checks directly).
    fn call_query_prefix_of(inner: &str) -> bool {
        let query = "日".repeat(20);
        query.starts_with(inner)
    }

    #[test]
    fn summary_list_memories_all() {
        let call = PendingCall::ListMemories {
            id: "x".to_string(),
            thought_signature: None,
            category: None,
        };
        assert_eq!(call.summary(), "all");
    }

    #[test]
    fn summary_list_memories_category() {
        let call = PendingCall::ListMemories {
            id: "x".to_string(),
            thought_signature: None,
            category: Some("incident".to_string()),
        };
        assert_eq!(call.summary(), "incident");
    }

    #[test]
    fn summary_spawn_ghost() {
        let call = PendingCall::SpawnGhost {
            id: "x".to_string(),
            thought_signature: None,
            runbook: "disk-alert".to_string(),
            message: "check disk usage".to_string(),
            agent: None,
            host: None,
        };
        assert_eq!(call.summary(), "disk-alert");
    }

    #[test]
    fn summary_foreground_names_the_shell_and_host_it_targets() {
        let mk = |shell: Option<&str>, host: Option<&str>| PendingCall::Foreground {
            id: "x".to_string(),
            thought_signature: None,
            cmd: "uptime".to_string(),
            shell: shell.map(|s| s.to_string()),
            host: host.map(|h| h.to_string()),
            timeout_secs: None,
            interactive: false,
        };
        assert_eq!(mk(None, None).summary(), "\"uptime\"");
        assert_eq!(mk(Some("s3"), None).summary(), "\"uptime\" in s3");
        assert_eq!(mk(None, Some("pinky")).summary(), "\"uptime\" on pinky");
        assert_eq!(
            mk(Some("s3"), Some("pinky")).summary(),
            "\"uptime\" in s3 on pinky"
        );
    }

    #[test]
    fn summary_get_shell_context_empty() {
        let call = PendingCall::GetShellContext {
            id: "x".to_string(),
            thought_signature: None,
            scope: None,
        };
        assert!(call.summary().is_empty());
    }

    #[test]
    fn summary_read_shell_screen_names_the_wait() {
        let mk = |wait_for: Option<&str>, settle_ms: Option<u64>| PendingCall::ReadShellScreen {
            id: "x".to_string(),
            thought_signature: None,
            shell: "s3".to_string(),
            lines: None,
            scrollback: None,
            pattern: None,
            annotated: false,
            wait_for: wait_for.map(|s| s.to_string()),
            settle_ms,
            timeout_secs: None,
        };
        assert_eq!(mk(None, None).summary(), "s3");
        assert_eq!(mk(Some("done"), None).summary(), "s3 wait_for=\"done\"");
        assert_eq!(mk(None, Some(1500)).summary(), "s3 settle_ms=1500");
    }

    fn mk_read_shell(filter: Option<String>, lines: Option<u64>) -> PendingCall {
        PendingCall::ReadShell {
            id: "x".to_string(),
            thought_signature: None,
            shell: "s3".to_string(),
            lines,
            since_command: None,
            filter,
        }
    }

    fn parsed_args(call: &PendingCall) -> serde_json::Value {
        serde_json::from_str(&call.to_tool_call().arguments).unwrap()
    }

    #[test]
    fn to_tool_call_read_shell_omits_none_options() {
        let args = parsed_args(&mk_read_shell(None, None));
        let obj = args.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj["shell"], "s3");
        assert!(!obj.contains_key("filter"));
        assert!(!obj.contains_key("lines"));
    }

    #[test]
    fn to_tool_call_read_shell_preserves_string_null_filter() {
        // A user may legitimately filter for the word "null" — the helper
        // strips JSON nulls, never strings.
        let args = parsed_args(&mk_read_shell(Some("null".to_string()), None));
        assert_eq!(args["filter"], "null");
    }

    #[test]
    fn to_tool_call_read_file_omits_none_options() {
        let args = parsed_args(&mk_read_file("/etc/hosts"));
        let obj = args.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj["path"], "/etc/hosts");
    }

    #[test]
    fn to_tool_call_preserves_present_options() {
        let find = PendingCall::FindInShells {
            id: "x".to_string(),
            thought_signature: None,
            pattern: "error".to_string(),
            scope: Some("all".to_string()),
        };
        assert_eq!(parsed_args(&find)["scope"], "all");

        let args = parsed_args(&mk_read_shell(None, Some(200)));
        assert_eq!(args["lines"], 200);

        let screen = PendingCall::ReadShellScreen {
            id: "x".to_string(),
            thought_signature: None,
            shell: "s3".to_string(),
            lines: Some(10),
            scrollback: None,
            pattern: None,
            annotated: false,
            wait_for: Some("done".to_string()),
            settle_ms: None,
            timeout_secs: Some(120),
        };
        let args = parsed_args(&screen);
        assert_eq!(args["lines"], 10);
        assert_eq!(args["wait_for"], "done");
        assert_eq!(args["timeout_secs"], 120);
        assert!(!args.as_object().unwrap().contains_key("settle_ms"));
    }

    /// `to_tool_call` round-trips `interactive` into the arguments JSON, so
    /// the echoed history shows the call the model actually made (03e).
    #[test]
    fn to_tool_call_carries_interactive_into_the_arguments() {
        let interactive = PendingCall::Foreground {
            id: "x".to_string(),
            thought_signature: None,
            cmd: "vim".to_string(),
            shell: None,
            host: None,
            timeout_secs: None,
            interactive: true,
        };
        assert_eq!(parsed_args(&interactive)["interactive"], true);
        assert_eq!(parsed_args(&mk_foreground("ls"))["interactive"], false);
    }

    #[test]
    fn to_tool_call_background_omits_credential_when_absent() {
        let mk = || PendingCall::Background {
            id: "x".to_string(),
            thought_signature: None,
            cmd: "ls".to_string(),
            _credential: None,
            host: None,
        };
        let args = parsed_args(&mk());
        assert!(!args.as_object().unwrap().contains_key("credential"));
    }

    #[test]
    fn to_tool_call_never_serializes_null() {
        // One instance of every variant, all Option fields None: no arm may
        // serialize a JSON null argument value — models imitate echoed nulls
        // as the string "null" on later calls.
        let x = || "x".to_string();
        let all: Vec<PendingCall> = vec![
            PendingCall::Foreground {
                id: x(),
                thought_signature: None,
                cmd: "ls".to_string(),
                shell: None,
                host: None,
                timeout_secs: None,
                interactive: false,
            },
            PendingCall::Background {
                id: x(),
                thought_signature: None,
                cmd: "ls".to_string(),
                _credential: None,
                host: None,
            },
            PendingCall::ScheduleCommand {
                id: x(),
                thought_signature: None,
                name: "n".to_string(),
                command: "c".to_string(),
                is_script: false,
                run_at: None,
                interval: None,
                runbook: None,
                ghost_runbook: None,
                cron: None,
            },
            PendingCall::InvalidArgs {
                id: x(),
                name: "read_file",
                arguments: "{}".to_string(),
                error: "missing required parameter".to_string(),
                thought_signature: None,
            },
            PendingCall::LoadTools {
                id: x(),
                groups: vec!["memory".to_string()],
                thought_signature: None,
            },
            PendingCall::ListSchedules {
                id: x(),
                thought_signature: None,
            },
            PendingCall::CancelSchedule {
                id: x(),
                thought_signature: None,
                job_id: "j".to_string(),
            },
            PendingCall::DeleteSchedule {
                id: x(),
                thought_signature: None,
                job_id: "j".to_string(),
            },
            PendingCall::WriteScript {
                id: x(),
                thought_signature: None,
                script_name: "s.sh".to_string(),
                content: "#!/bin/sh".to_string(),
            },
            PendingCall::ListScripts {
                id: x(),
                thought_signature: None,
            },
            PendingCall::ReadScript {
                id: x(),
                thought_signature: None,
                script_name: "s.sh".to_string(),
            },
            PendingCall::DeleteScript {
                id: x(),
                thought_signature: None,
                script_name: "s.sh".to_string(),
            },
            mk_read_file("/etc/hosts"),
            PendingCall::EditFile {
                id: x(),
                thought_signature: None,
                path: "/tmp/f".to_string(),
                operation: "edit".to_string(),
                old_string: None,
                new_string: None,
                content: None,
                dest_path: None,
                host: None,
                sudo: None,
            },
            PendingCall::WriteRunbook {
                id: x(),
                thought_signature: None,
                name: "rb".to_string(),
                content: "# RB".to_string(),
            },
            PendingCall::DeleteRunbook {
                id: x(),
                thought_signature: None,
                name: "rb".to_string(),
            },
            PendingCall::ReadRunbook {
                id: x(),
                thought_signature: None,
                name: "rb".to_string(),
            },
            PendingCall::ListRunbooks {
                id: x(),
                thought_signature: None,
            },
            PendingCall::AddMemory {
                id: x(),
                thought_signature: None,
                key: "k".to_string(),
                value: "v".to_string(),
                category: "knowledge".to_string(),
            },
            PendingCall::UpdateMemory {
                id: x(),
                thought_signature: None,
                key: "k".to_string(),
                category: "knowledge".to_string(),
                body: None,
                append: false,
                tags: None,
                summary: None,
                relates_to: None,
                expires: None,
            },
            PendingCall::DeleteMemory {
                id: x(),
                thought_signature: None,
                key: "k".to_string(),
                category: "knowledge".to_string(),
            },
            PendingCall::ReadMemory {
                id: x(),
                thought_signature: None,
                key: "k".to_string(),
                category: "knowledge".to_string(),
            },
            PendingCall::ListMemories {
                id: x(),
                thought_signature: None,
                category: None,
            },
            PendingCall::SearchRepository {
                id: x(),
                thought_signature: None,
                query: "q".to_string(),
                kind: "all".to_string(),
            },
            PendingCall::RecallContext {
                id: x(),
                thought_signature: None,
                query: None,
                turn_start: None,
                turn_end: None,
                scope: None,
            },
            PendingCall::GetShellContext {
                id: x(),
                thought_signature: None,
                scope: None,
            },
            PendingCall::ListShells {
                id: x(),
                thought_signature: None,
                scope: None,
            },
            PendingCall::SpawnGhost {
                id: x(),
                thought_signature: None,
                runbook: "rb".to_string(),
                message: "m".to_string(),
                agent: None,
                host: None,
            },
            PendingCall::CreateAgent {
                id: x(),
                thought_signature: None,
                name: "a".to_string(),
                description: "d".to_string(),
                prompt: "p".to_string(),
                model: None,
                memory_namespace: "ns".to_string(),
                max_turns: None,
                auto_approve_read_only: false,
                auto_approve_scripts: vec![],
            },
            PendingCall::ReadAgent {
                id: x(),
                thought_signature: None,
                name: "a".to_string(),
            },
            PendingCall::ListAgents {
                id: x(),
                thought_signature: None,
            },
            PendingCall::DeleteAgent {
                id: x(),
                thought_signature: None,
                name: "a".to_string(),
            },
            PendingCall::AwaitAgentResult {
                id: x(),
                thought_signature: None,
                job_id: "j".to_string(),
                agent_name: "a".to_string(),
                timeout_secs: 30,
            },
            mk_read_shell(None, None),
            PendingCall::ReadShellScreen {
                id: x(),
                thought_signature: None,
                shell: "s3".to_string(),
                lines: None,
                scrollback: None,
                pattern: None,
                annotated: false,
                wait_for: None,
                settle_ms: None,
                timeout_secs: None,
            },
            PendingCall::FindInShells {
                id: x(),
                thought_signature: None,
                pattern: "p".to_string(),
                scope: None,
            },
        ];
        for call in &all {
            let args: serde_json::Value =
                serde_json::from_str(&call.to_tool_call().arguments).unwrap();
            for (key, val) in args.as_object().unwrap() {
                assert!(
                    !val.is_null(),
                    "{} serializes null for key '{}'",
                    call.tool_name(),
                    key
                );
            }
        }
    }

    #[test]
    fn invalid_args_pending_call_keeps_the_raw_call() {
        let call = PendingCall::InvalidArgs {
            id: "tc_9".to_string(),
            name: "run_terminal_command",
            arguments: r#"{"command":"x","wait":"true"}"#.to_string(),
            error: "parameter 'wait' is string, expected boolean".to_string(),
            thought_signature: Some("sig".to_string()),
        };
        assert_eq!(call.tool_name(), "run_terminal_command");
        assert_eq!(call.id(), "tc_9");
        let tool_call = call.to_tool_call();
        assert_eq!(tool_call.name, "run_terminal_command", "the name is kept");
        assert_eq!(
            tool_call.arguments, r#"{"command":"x","wait":"true"}"#,
            "history shows the call exactly as the model made it"
        );
        assert_eq!(tool_call.id, "tc_9");
        assert_eq!(tool_call.thought_signature.as_deref(), Some("sig"));
    }
}
