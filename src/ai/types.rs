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
    pub tool_name: String,
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
    Background { id: String, thought_signature: Option<String>, cmd: String, _credential: Option<String>, retry_pane: Option<String> },
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
    WatchPane { id: String, thought_signature: Option<String>, pane_id: String, timeout_secs: u64, pattern: Option<String> },
    ReadFile { id: String, thought_signature: Option<String>, path: String, offset: Option<u64>, limit: Option<u64>, pattern: Option<String>, target_pane: Option<String> },
    EditFile { id: String, thought_signature: Option<String>, path: String, old_string: String, new_string: String, target_pane: Option<String> },
    WriteRunbook { id: String, thought_signature: Option<String>, name: String, content: String },
    DeleteRunbook { id: String, thought_signature: Option<String>, name: String },
    ReadRunbook { id: String, thought_signature: Option<String>, name: String },
    ListRunbooks { id: String, thought_signature: Option<String> },
    AddMemory { id: String, thought_signature: Option<String>, key: String, value: String, category: String },
    DeleteMemory { id: String, thought_signature: Option<String>, key: String, category: String },
    ReadMemory { id: String, thought_signature: Option<String>, key: String, category: String },
    ListMemories { id: String, thought_signature: Option<String>, category: Option<String> },
    SearchRepository { id: String, thought_signature: Option<String>, query: String, kind: String },
    GetTerminalContext { id: String, thought_signature: Option<String> },
    ListPanes { id: String, thought_signature: Option<String> },
    CloseBackgroundWindow { id: String, thought_signature: Option<String>, pane_id: String },
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
            PendingCall::Background { id, thought_signature, cmd, retry_pane, .. } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: {
                    let mut a = serde_json::json!({"command": cmd, "background": true});
                    if let Some(rp) = retry_pane {
                        a["retry_in_pane"] = serde_json::json!(rp);
                    }
                    a.to_string()
                },
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
            PendingCall::WatchPane { id, thought_signature, pane_id, timeout_secs, pattern } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "watch_pane".to_string(),
                arguments: serde_json::json!({"pane_id": pane_id, "timeout_secs": timeout_secs, "pattern": pattern}).to_string(),
            },
            PendingCall::ReadFile { id, thought_signature, path, offset, limit, pattern, target_pane } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": path, "offset": offset, "limit": limit, "pattern": pattern, "target_pane": target_pane}).to_string(),
            },
            PendingCall::EditFile { id, thought_signature, path, old_string, new_string, target_pane } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "edit_file".to_string(),
                arguments: serde_json::json!({"path": path, "old_string": old_string, "new_string": new_string, "target_pane": target_pane}).to_string(),
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
            PendingCall::GetTerminalContext { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "get_terminal_context".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::ListPanes { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_panes".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::CloseBackgroundWindow { id, thought_signature, pane_id } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "close_background_window".to_string(),
                arguments: serde_json::json!({"pane_id": pane_id}).to_string(),
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
            PendingCall::ReadFile { id, .. } => id,
            PendingCall::EditFile { id, .. } => id,
            PendingCall::WriteRunbook { id, .. } => id,
            PendingCall::DeleteRunbook { id, .. } => id,
            PendingCall::ReadRunbook { id, .. } => id,
            PendingCall::ListRunbooks { id, .. } => id,
            PendingCall::AddMemory { id, .. } => id,
            PendingCall::DeleteMemory { id, .. } => id,
            PendingCall::ReadMemory { id, .. } => id,
            PendingCall::ListMemories { id, .. } => id,
            PendingCall::SearchRepository { id, .. } => id,
            PendingCall::GetTerminalContext { id, .. } => id,
            PendingCall::ListPanes { id, .. } => id,
            PendingCall::CloseBackgroundWindow { id, .. } => id,
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
            PendingCall::ReadFile { .. } => "read_file",
            PendingCall::EditFile { .. } => "edit_file",
            PendingCall::WriteRunbook { .. } => "write_runbook",
            PendingCall::DeleteRunbook { .. } => "delete_runbook",
            PendingCall::ReadRunbook { .. } => "read_runbook",
            PendingCall::ListRunbooks { .. } => "list_runbooks",
            PendingCall::AddMemory { .. } => "add_memory",
            PendingCall::DeleteMemory { .. } => "delete_memory",
            PendingCall::ReadMemory { .. } => "read_memory",
            PendingCall::ListMemories { .. } => "list_memories",
            PendingCall::SearchRepository { .. } => "search_repository",
            PendingCall::GetTerminalContext { .. } => "get_terminal_context",
            PendingCall::ListPanes { .. } => "list_panes",
            PendingCall::CloseBackgroundWindow { .. } => "close_background_window",
        }
    }
}

#[derive(Debug)]
pub enum AiEvent {
    Token(String),
    /// (id, cmd, background, target_pane, retry_in_pane, thought_signature)
    ToolCall(String, String, bool, Option<String>, Option<String>, Option<String>),
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
    WatchPane { id: String, pane_id: String, timeout_secs: u64, pattern: Option<String>, thought_signature: Option<String> },
    ReadFile { id: String, path: String, offset: Option<u64>, limit: Option<u64>, pattern: Option<String>, target_pane: Option<String>, thought_signature: Option<String> },
    EditFile { id: String, path: String, old_string: String, new_string: String, target_pane: Option<String>, thought_signature: Option<String> },
    WriteRunbook { id: String, name: String, content: String, thought_signature: Option<String> },
    DeleteRunbook { id: String, name: String, thought_signature: Option<String> },
    ReadRunbook { id: String, name: String, thought_signature: Option<String> },
    ListRunbooks { id: String, thought_signature: Option<String> },
    AddMemory { id: String, key: String, value: String, category: String, thought_signature: Option<String> },
    DeleteMemory { id: String, key: String, category: String, thought_signature: Option<String> },
    ReadMemory { id: String, key: String, category: String, thought_signature: Option<String> },
    ListMemories { id: String, category: Option<String>, thought_signature: Option<String> },
    SearchRepository { id: String, query: String, kind: String, thought_signature: Option<String> },
    GetTerminalContext { id: String, thought_signature: Option<String> },
    ListPanes { id: String, thought_signature: Option<String> },
    CloseBackgroundWindow { id: String, pane_id: String, thought_signature: Option<String> },
    Done(AiUsage),
    Error(String),
}
