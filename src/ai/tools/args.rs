use crate::ai::types::AiEvent;
use serde::Deserialize;
use serde_json::Value;

/// Trait for typed tool-argument deserialization + AiEvent construction.
/// Every tool in `TOOLS` must have a corresponding impl — the
/// `dispatch_roundtrip` test verifies coverage at compile time.
pub(super) trait ToolArgs: Sized {
    fn from_value(value: Value) -> Option<Self>;
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent;
}

// ── Typed arg structs ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct RunTerminalCommandArgs {
    command: String,
    #[serde(default)]
    background: bool,
    target_pane: Option<String>,
    retry_in_pane: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ScheduleCommandArgs {
    #[serde(default = "default_unnamed")]
    name: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    is_script: bool,
    run_at: Option<String>,
    interval: Option<String>,
    runbook: Option<String>,
    ghost_runbook: Option<String>,
    cron: Option<String>,
}

pub(super) struct LoadToolsArgs {
    groups: Value,
}

impl LoadToolsArgs {
    fn from_value(value: Value) -> Option<Self> {
        let groups = value.get("groups")?.clone();
        Some(Self { groups })
    }
}

#[derive(Deserialize)]
pub(super) struct CancelDeleteScheduleArgs {
    id: String,
}

#[derive(Deserialize)]
pub(super) struct CloseBgWindowArgs {
    pane_id: String,
}

#[derive(Deserialize)]
pub(super) struct WriteScriptArgs {
    script_name: String,
    content: String,
}

#[derive(Deserialize)]
pub(super) struct ReadScriptArgs {
    script_name: String,
}

#[derive(Deserialize)]
pub(super) struct DeleteScriptArgs {
    script_name: String,
}

#[derive(Deserialize)]
pub(super) struct WatchPaneArgs {
    pane_id: String,
    #[serde(default = "default_300")]
    timeout_secs: u64,
    pattern: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ReadFileArgs {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<String>,
    target_pane: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ReadPaneArgs {
    pane_id: String,
    lines: Option<u64>,
    grep: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct EditFileArgs {
    path: String,
    #[serde(default = "default_edit")]
    operation: String,
    old_string: Option<String>,
    new_string: Option<String>,
    content: Option<String>,
    dest_path: Option<String>,
    target_pane: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct WriteRunbookArgs {
    name: String,
    content: String,
}

#[derive(Deserialize)]
pub(super) struct ReadRunbookArgs {
    name: String,
}

#[derive(Deserialize)]
pub(super) struct AddMemoryArgs {
    key: String,
    value: String,
    #[serde(default = "default_knowledge")]
    category: String,
}

#[derive(Deserialize)]
pub(super) struct UpdateMemoryArgs {
    key: String,
    #[serde(default = "default_knowledge")]
    category: String,
    body: Option<String>,
    #[serde(default)]
    append: bool,
    tags: Option<serde_json::Value>,
    summary: Option<String>,
    relates_to: Option<serde_json::Value>,
    expires: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct DeleteMemoryArgs {
    key: String,
    #[serde(default = "default_knowledge")]
    category: String,
}

#[derive(Deserialize)]
pub(super) struct ReadMemoryArgs {
    key: String,
    #[serde(default = "default_knowledge")]
    category: String,
}

#[derive(Deserialize)]
pub(super) struct ListMemoriesArgs {
    category: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SearchRepositoryArgs {
    query: String,
    #[serde(default = "default_all")]
    kind: String,
}

#[derive(Deserialize)]
pub(super) struct SpawnGhostArgs {
    runbook: String,
    message: String,
    agent: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CreateAgentArgs {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    prompt: String,
    model: Option<String>,
    #[serde(default)]
    memory_namespace: String,
    max_turns: Option<u32>,
    #[serde(default)]
    auto_approve_read_only: bool,
    auto_approve_scripts: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(super) struct ReadAgentArgs {
    name: String,
}

#[derive(Deserialize)]
pub(super) struct AwaitAgentResultArgs {
    job_id: String,
    agent_name: String,
    #[serde(default = "default_300")]
    timeout_secs: u64,
}

// ── Default helpers ────────────────────────────────────────────────────────

pub(super) fn default_unnamed() -> String {
    "unnamed".to_string()
}

pub(super) fn default_300() -> u64 {
    300
}

pub(super) fn default_edit() -> String {
    "edit".to_string()
}

pub(super) fn default_knowledge() -> String {
    "knowledge".to_string()
}

pub(super) fn default_all() -> String {
    "all".to_string()
}

// ── ToolArgs impls ────────────────────────────────────────────────────────

impl ToolArgs for RunTerminalCommandArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ToolCall(
            id.to_string(),
            self.command,
            self.background,
            self.target_pane,
            self.retry_in_pane,
            ts,
        )
    }
}

impl ToolArgs for ScheduleCommandArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ScheduleCommand {
            id: id.to_string(),
            name: self.name,
            command: self.command,
            is_script: self.is_script,
            run_at: self.run_at,
            interval: self.interval,
            runbook: self.runbook,
            ghost_runbook: self.ghost_runbook,
            cron: self.cron,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for LoadToolsArgs {
    fn from_value(value: Value) -> Option<Self> {
        LoadToolsArgs::from_value(value)
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        let groups = extract_string_vec(&self.groups).unwrap_or_default();
        AiEvent::LoadTools {
            id: id.to_string(),
            groups,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for CancelDeleteScheduleArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, _id: &str, _ts: Option<String>) -> AiEvent {
        // Never called directly — cancel/delete schedule use schedule_id_event()
        unreachable!("use schedule_id_event instead")
    }
}

impl ToolArgs for CloseBgWindowArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::CloseBackgroundWindow {
            id: id.to_string(),
            pane_id: self.pane_id,
            thought_signature: ts,
        }
    }
}

/// Build CancelSchedule or DeleteSchedule from the shared arg shape.
pub(super) fn schedule_id_event<T>(args: Value, ts: Option<String>, mk: T) -> Option<AiEvent>
where
    T: FnOnce(String, Option<String>) -> AiEvent,
{
    CancelDeleteScheduleArgs::from_value(args).map(|a| mk(a.id, ts))
}

impl ToolArgs for WriteScriptArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::WriteScript {
            id: id.to_string(),
            script_name: self.script_name,
            content: self.content,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ReadScriptArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ReadScript {
            id: id.to_string(),
            script_name: self.script_name,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for DeleteScriptArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::DeleteScript {
            id: id.to_string(),
            script_name: self.script_name,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for WatchPaneArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::WatchPane {
            id: id.to_string(),
            pane_id: self.pane_id,
            timeout_secs: self.timeout_secs,
            pattern: self.pattern,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ReadFileArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ReadFile {
            id: id.to_string(),
            path: self.path,
            offset: self.offset,
            limit: self.limit,
            pattern: self.pattern,
            target_pane: self.target_pane,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ReadPaneArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ReadPane {
            id: id.to_string(),
            pane_id: self.pane_id,
            lines: self.lines,
            grep: self.grep,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for EditFileArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::EditFile {
            id: id.to_string(),
            path: self.path,
            operation: self.operation,
            old_string: self.old_string,
            new_string: self.new_string,
            content: self.content,
            dest_path: self.dest_path,
            target_pane: self.target_pane,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for WriteRunbookArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::WriteRunbook {
            id: id.to_string(),
            name: self.name,
            content: self.content,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ReadRunbookArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ReadRunbook {
            id: id.to_string(),
            name: self.name,
            thought_signature: ts,
        }
    }
}

/// Shared helper for read/delete runbook — same arg shape, different event.
pub(super) fn runbook_name_event<T>(args: Value, ts: Option<String>, mk: T) -> Option<AiEvent>
where
    T: FnOnce(String, Option<String>) -> AiEvent,
{
    ReadRunbookArgs::from_value(args).map(|a| mk(a.name, ts))
}

impl ToolArgs for AddMemoryArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::AddMemory {
            id: id.to_string(),
            key: self.key,
            value: self.value,
            category: self.category,
            thought_signature: ts,
        }
    }
}

/// Helpers for the dual-format tags/relates_to fields (JSON string or array).
pub(super) fn extract_string_vec(v: &Value) -> Option<Vec<String>> {
    v.as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .or_else(|| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
        })
}

impl ToolArgs for UpdateMemoryArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::UpdateMemory {
            id: id.to_string(),
            key: self.key,
            category: self.category,
            body: self.body,
            append: self.append,
            tags: self.tags.as_ref().and_then(extract_string_vec),
            summary: self.summary,
            relates_to: self.relates_to.as_ref().and_then(extract_string_vec),
            expires: self.expires,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for DeleteMemoryArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::DeleteMemory {
            id: id.to_string(),
            key: self.key,
            category: self.category,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ReadMemoryArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ReadMemory {
            id: id.to_string(),
            key: self.key,
            category: self.category,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ListMemoriesArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ListMemories {
            id: id.to_string(),
            category: self.category,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for SearchRepositoryArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::SearchRepository {
            id: id.to_string(),
            query: self.query,
            kind: self.kind,
            thought_signature: ts,
        }
    }
}
pub(super) struct RecallContextArgs {
    pub query: Option<String>,
    pub turn_start: Option<u32>,
    pub turn_end: Option<u32>,
    pub scope: Option<String>,
}

#[derive(Deserialize)]
struct RecallContextDeserialize {
    pub query: Option<String>,
    pub turn_start: Option<u32>,
    pub turn_end: Option<u32>,
    pub scope: Option<String>,
}

impl ToolArgs for RecallContextArgs {
    fn from_value(value: Value) -> Option<Self> {
        let deserialized: RecallContextDeserialize = serde_json::from_value(value).ok()?;
        Some(Self {
            query: deserialized.query,
            turn_start: deserialized.turn_start,
            turn_end: deserialized.turn_end,
            scope: deserialized.scope,
        })
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::RecallContext {
            id: id.to_string(),
            query: self.query,
            turn_start: self.turn_start,
            turn_end: self.turn_end,
            scope: self.scope,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for SpawnGhostArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::SpawnGhost {
            id: id.to_string(),
            runbook: self.runbook,
            message: self.message,
            agent: self.agent,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for CreateAgentArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::CreateAgent {
            id: id.to_string(),
            name: self.name,
            description: self.description,
            prompt: self.prompt,
            model: self.model,
            memory_namespace: self.memory_namespace,
            max_turns: self.max_turns,
            auto_approve_read_only: self.auto_approve_read_only,
            auto_approve_scripts: self
                .auto_approve_scripts
                .as_ref()
                .and_then(extract_string_vec)
                .unwrap_or_default(),
            thought_signature: ts,
        }
    }
}

impl ToolArgs for ReadAgentArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ReadAgent {
            id: id.to_string(),
            name: self.name,
            thought_signature: ts,
        }
    }
}

impl ToolArgs for AwaitAgentResultArgs {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::AwaitAgentResult {
            id: id.to_string(),
            job_id: self.job_id,
            agent_name: self.agent_name,
            timeout_secs: self.timeout_secs,
            thought_signature: ts,
        }
    }
}
