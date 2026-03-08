use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    #[serde(skip)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<ToolResult>>,
}

#[derive(Debug, Clone, Default)]
pub struct AiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// A tool call collected during AI streaming, to be executed after `Done`.
pub enum PendingCall {
    Foreground { id: String, thought_signature: Option<String>, cmd: String, target: Option<String> },
    Background { id: String, thought_signature: Option<String>, cmd: String, _credential: Option<String> },
    ScheduleCommand {
        id: String,
        thought_signature: Option<String>,
        name: String,
        command: String,
        is_script: bool,
        run_at: Option<String>,
        interval: Option<String>,
        runbook: Option<String>,
    },
    ListSchedules { id: String, thought_signature: Option<String> },
    CancelSchedule { id: String, thought_signature: Option<String>, job_id: String },
    DeleteSchedule { id: String, thought_signature: Option<String>, job_id: String },
    WriteScript { id: String, thought_signature: Option<String>, script_name: String, content: String },
    ListScripts { id: String, thought_signature: Option<String> },
    ReadScript { id: String, thought_signature: Option<String>, script_name: String },
    WatchPane { id: String, thought_signature: Option<String>, pane_id: String, timeout_secs: u64 },
    WriteRunbook { id: String, thought_signature: Option<String>, name: String, content: String },
    DeleteRunbook { id: String, thought_signature: Option<String>, name: String },
    ReadRunbook { id: String, thought_signature: Option<String>, name: String },
    ListRunbooks { id: String, thought_signature: Option<String> },
    AddMemory { id: String, thought_signature: Option<String>, key: String, value: String, category: String },
    DeleteMemory { id: String, thought_signature: Option<String>, key: String, category: String },
    ReadMemory { id: String, thought_signature: Option<String>, key: String, category: String },
    ListMemories { id: String, thought_signature: Option<String>, category: Option<String> },
    SearchRepository { id: String, thought_signature: Option<String>, query: String, kind: String },
}

impl PendingCall {
    pub fn to_tool_call(&self) -> ToolCall {
        match self {
            PendingCall::Foreground { id, thought_signature, cmd, target } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: serde_json::json!({
                    "command": cmd,
                    "background": false,
                    "target_pane": target
                }).to_string(),
            },
            PendingCall::Background { id, thought_signature, cmd, .. } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: serde_json::json!({"command": cmd, "background": true}).to_string(),
            },
            PendingCall::ScheduleCommand { id, thought_signature, name, command, is_script, run_at, interval, runbook } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "schedule_command".to_string(),
                arguments: serde_json::json!({
                    "name": name, "command": command,
                    "is_script": is_script,
                    "run_at": run_at, "interval": interval, "runbook": runbook
                }).to_string(),
            },
            PendingCall::ListSchedules { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_schedules".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::CancelSchedule { id, thought_signature, job_id } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "cancel_schedule".to_string(),
                arguments: serde_json::json!({"id": job_id}).to_string(),
            },
            PendingCall::DeleteSchedule { id, thought_signature, job_id } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_schedule".to_string(),
                arguments: serde_json::json!({"id": job_id}).to_string(),
            },
            PendingCall::WriteScript { id, thought_signature, script_name, content } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "write_script".to_string(),
                arguments: serde_json::json!({"script_name": script_name, "content": content}).to_string(),
            },
            PendingCall::ListScripts { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_scripts".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::ReadScript { id, thought_signature, script_name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_script".to_string(),
                arguments: serde_json::json!({"script_name": script_name}).to_string(),
            },
            PendingCall::WatchPane { id, thought_signature, pane_id, timeout_secs } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "watch_pane".to_string(),
                arguments: serde_json::json!({"pane_id": pane_id, "timeout_secs": timeout_secs}).to_string(),
            },
            PendingCall::WriteRunbook { id, thought_signature, name, content } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "write_runbook".to_string(),
                arguments: serde_json::json!({"name": name, "content": content}).to_string(),
            },
            PendingCall::DeleteRunbook { id, thought_signature, name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_runbook".to_string(),
                arguments: serde_json::json!({"name": name}).to_string(),
            },
            PendingCall::ReadRunbook { id, thought_signature, name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_runbook".to_string(),
                arguments: serde_json::json!({"name": name}).to_string(),
            },
            PendingCall::ListRunbooks { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_runbooks".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::AddMemory { id, thought_signature, key, value, category } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "add_memory".to_string(),
                arguments: serde_json::json!({"key": key, "value": value, "category": category}).to_string(),
            },
            PendingCall::DeleteMemory { id, thought_signature, key, category } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_memory".to_string(),
                arguments: serde_json::json!({"key": key, "category": category}).to_string(),
            },
            PendingCall::ReadMemory { id, thought_signature, key, category } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_memory".to_string(),
                arguments: serde_json::json!({"key": key, "category": category}).to_string(),
            },
            PendingCall::ListMemories { id, thought_signature, category } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_memories".to_string(),
                arguments: serde_json::json!({"category": category}).to_string(),
            },
            PendingCall::SearchRepository { id, thought_signature, query, kind } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "search_repository".to_string(),
                arguments: serde_json::json!({"query": query, "kind": kind}).to_string(),
            },
        }
    }

    pub fn id(&self) -> &str {
        match self {
            PendingCall::Foreground { id, .. } => id,
            PendingCall::Background { id, .. } => id,
            PendingCall::ScheduleCommand { id, .. } => id,
            PendingCall::ListSchedules { id, .. } => id,
            PendingCall::CancelSchedule { id, .. } => id,
            PendingCall::DeleteSchedule { id, .. } => id,
            PendingCall::WriteScript { id, .. } => id,
            PendingCall::ListScripts { id, .. } => id,
            PendingCall::ReadScript { id, .. } => id,
            PendingCall::WatchPane { id, .. } => id,
            PendingCall::WriteRunbook { id, .. } => id,
            PendingCall::DeleteRunbook { id, .. } => id,
            PendingCall::ReadRunbook { id, .. } => id,
            PendingCall::ListRunbooks { id, .. } => id,
            PendingCall::AddMemory { id, .. } => id,
            PendingCall::DeleteMemory { id, .. } => id,
            PendingCall::ReadMemory { id, .. } => id,
            PendingCall::ListMemories { id, .. } => id,
            PendingCall::SearchRepository { id, .. } => id,
        }
    }

    /// Returns the canonical tool name for this call, used for per-turn rate limiting.
    pub fn tool_name(&self) -> &'static str {
        match self {
            PendingCall::Foreground { .. } | PendingCall::Background { .. } => "run_terminal_command",
            PendingCall::ScheduleCommand { .. } => "schedule_command",
            PendingCall::ListSchedules { .. } => "list_schedules",
            PendingCall::CancelSchedule { .. } => "cancel_schedule",
            PendingCall::DeleteSchedule { .. } => "delete_schedule",
            PendingCall::WriteScript { .. } => "write_script",
            PendingCall::ListScripts { .. } => "list_scripts",
            PendingCall::ReadScript { .. } => "read_script",
            PendingCall::WatchPane { .. } => "watch_pane",
            PendingCall::WriteRunbook { .. } => "write_runbook",
            PendingCall::DeleteRunbook { .. } => "delete_runbook",
            PendingCall::ReadRunbook { .. } => "read_runbook",
            PendingCall::ListRunbooks { .. } => "list_runbooks",
            PendingCall::AddMemory { .. } => "add_memory",
            PendingCall::DeleteMemory { .. } => "delete_memory",
            PendingCall::ReadMemory { .. } => "read_memory",
            PendingCall::ListMemories { .. } => "list_memories",
            PendingCall::SearchRepository { .. } => "search_repository",
        }
    }
}

#[derive(Debug)]
pub enum AiEvent {
    Token(String),
    ToolCall(String, String, bool, Option<String>, Option<String>),
    ScheduleCommand {
        id: String,
        name: String,
        command: String,
        is_script: bool,
        run_at: Option<String>,
        interval: Option<String>,
        runbook: Option<String>,
        thought_signature: Option<String>,
    },
    ListSchedules { id: String, thought_signature: Option<String> },
    CancelSchedule { id: String, job_id: String, thought_signature: Option<String> },
    DeleteSchedule { id: String, job_id: String, thought_signature: Option<String> },
    WriteScript { id: String, script_name: String, content: String, thought_signature: Option<String> },
    ListScripts { id: String, thought_signature: Option<String> },
    ReadScript { id: String, script_name: String, thought_signature: Option<String> },
    WatchPane { id: String, pane_id: String, timeout_secs: u64, thought_signature: Option<String> },
    WriteRunbook { id: String, name: String, content: String, thought_signature: Option<String> },
    DeleteRunbook { id: String, name: String, thought_signature: Option<String> },
    ReadRunbook { id: String, name: String, thought_signature: Option<String> },
    ListRunbooks { id: String, thought_signature: Option<String> },
    AddMemory { id: String, key: String, value: String, category: String, thought_signature: Option<String> },
    DeleteMemory { id: String, key: String, category: String, thought_signature: Option<String> },
    ReadMemory { id: String, key: String, category: String, thought_signature: Option<String> },
    ListMemories { id: String, category: Option<String>, thought_signature: Option<String> },
    SearchRepository { id: String, query: String, kind: String, thought_signature: Option<String> },
    Done(AiUsage),
    Error(String),
}
