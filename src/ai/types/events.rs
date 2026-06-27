use super::wire::TokenBreakdown;

#[derive(Debug)]
pub enum AiEvent {
    Token(String),
    /// (id, cmd, background, target_pane, retry_in_pane, thought_signature)
    ToolCall(
        String,
        String,
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    ScheduleCommand {
        id: String,
        name: String,
        command: String,
        is_script: bool,
        run_at: Option<String>,
        interval: Option<String>,
        runbook: Option<String>,
        ghost_runbook: Option<String>,
        cron: Option<String>,
        thought_signature: Option<String>,
    },
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
        job_id: String,
        thought_signature: Option<String>,
    },
    DeleteSchedule {
        id: String,
        job_id: String,
        thought_signature: Option<String>,
    },
    WriteScript {
        id: String,
        script_name: String,
        content: String,
        thought_signature: Option<String>,
    },
    ListScripts {
        id: String,
        thought_signature: Option<String>,
    },
    ReadScript {
        id: String,
        script_name: String,
        thought_signature: Option<String>,
    },
    DeleteScript {
        id: String,
        script_name: String,
        thought_signature: Option<String>,
    },
    WatchPane {
        id: String,
        pane_id: String,
        timeout_secs: u64,
        pattern: Option<String>,
        thought_signature: Option<String>,
    },
    ReadFile {
        id: String,
        path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pattern: Option<String>,
        target_pane: Option<String>,
        thought_signature: Option<String>,
    },
    EditFile {
        id: String,
        path: String,
        operation: String,
        old_string: Option<String>,
        new_string: Option<String>,
        content: Option<String>,
        dest_path: Option<String>,
        target_pane: Option<String>,
        thought_signature: Option<String>,
    },
    WriteRunbook {
        id: String,
        name: String,
        content: String,
        thought_signature: Option<String>,
    },
    DeleteRunbook {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    ReadRunbook {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    ListRunbooks {
        id: String,
        thought_signature: Option<String>,
    },
    AddMemory {
        id: String,
        key: String,
        value: String,
        category: String,
        thought_signature: Option<String>,
    },
    UpdateMemory {
        id: String,
        key: String,
        category: String,
        body: Option<String>,
        append: bool,
        tags: Option<Vec<String>>,
        summary: Option<String>,
        relates_to: Option<Vec<String>>,
        expires: Option<String>,
        thought_signature: Option<String>,
    },
    DeleteMemory {
        id: String,
        key: String,
        category: String,
        thought_signature: Option<String>,
    },
    ReadMemory {
        id: String,
        key: String,
        category: String,
        thought_signature: Option<String>,
    },
    ListMemories {
        id: String,
        category: Option<String>,
        thought_signature: Option<String>,
    },
    SearchRepository {
        id: String,
        query: String,
        kind: String,
        thought_signature: Option<String>,
    },
    GetTerminalContext {
        id: String,
        thought_signature: Option<String>,
    },
    ListPanes {
        id: String,
        thought_signature: Option<String>,
    },
    CloseBackgroundWindow {
        id: String,
        pane_id: String,
        thought_signature: Option<String>,
    },
    /// Spawn an autonomous Ghost Shell session in the background.
    SpawnGhost {
        id: String,
        runbook: String,
        message: String,
        agent: Option<String>,
        thought_signature: Option<String>,
    },
    /// Create or update a named agent config.
    CreateAgent {
        id: String,
        name: String,
        description: String,
        prompt: String,
        model: Option<String>,
        memory_namespace: String,
        max_turns: Option<u32>,
        auto_approve_read_only: bool,
        auto_approve_scripts: Vec<String>,
        thought_signature: Option<String>,
    },
    /// Read a named agent config.
    ReadAgent {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    /// List all named agents.
    ListAgents {
        id: String,
        thought_signature: Option<String>,
    },
    /// Delete a named agent.
    DeleteAgent {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    /// Wait for a spawned agent ghost shell to complete and return its result.
    AwaitAgentResult {
        id: String,
        job_id: String,
        agent_name: String,
        timeout_secs: u64,
        thought_signature: Option<String>,
    },
    Done(TokenBreakdown),
    Error(String),
}
