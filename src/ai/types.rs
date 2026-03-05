use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
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
    WatchPane { id: String, pane_id: String, thought_signature: Option<String> },
    Done(AiUsage),
    Error(String),
}
