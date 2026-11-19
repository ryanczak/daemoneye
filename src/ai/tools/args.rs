use crate::ai::types::{AiEvent, PendingCall};
use serde::Deserialize;
use serde_json::Value;

/// Trait for typed tool-argument deserialization + AiEvent construction.
/// Every tool in `TOOLS` must have a corresponding impl — the
/// `dispatch_roundtrip` test verifies coverage at compile time.
pub(super) trait ToolArgs: serde::de::DeserializeOwned {
    fn from_value(value: Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent;
}

/// Lenient boolean deserialization: providers (GLM in rehearsal 1) send
/// `"true"`/`"false"` as JSON strings despite the schema's `boolean` type.
/// A real boolean, a string boolean (case-insensitive, whitespace-trimmed)
/// and null all parse; any other string is an error so the invalid-args
/// error can name the field.
pub(super) fn lenient_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<Value>::deserialize(deserializer)?;
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            other => Err(serde::de::Error::custom(format!(
                "invalid boolean: {other:?}"
            ))),
        },
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected a boolean, got {}",
            json_type_of(&other)
        ))),
    }
}

fn json_type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Lenient `keys`: a real array, a JSON-encoded string of an array, or a
/// single key name as a bare string — the three shapes providers send for a
/// `string`-typed array parameter. Never split on separators: `C-,` and
/// `M- ` are valid key names.
pub(super) fn lenient_keys<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<Value>::deserialize(deserializer)?.unwrap_or(Value::Null);
    if v.as_str().is_some_and(|s| s.trim().is_empty()) {
        return Ok(Vec::new());
    }
    // A string that parses as a JSON array is that array, empty included
    // (F6): GLM sends `"[]"` for no keys, and a bare-string fallback here
    // would hand back the one-name list `["[]"]`.
    if let Some(s) = v.as_str()
        && let Ok(arr) = serde_json::from_str::<serde_json::Value>(s)
        && arr.is_array()
    {
        return Ok(extract_string_vec(&arr).unwrap_or_default());
    }
    Ok(extract_string_vec(&v)
        .filter(|keys| !keys.is_empty())
        .or_else(|| v.as_str().map(|s| vec![s.to_string()]))
        .unwrap_or_default())
}

// ── Typed arg structs ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct RunTerminalCommandArgs {
    command: String,
    #[serde(default, deserialize_with = "lenient_bool")]
    wait: Option<bool>,
    // GLM sends `background` from its priors — as a string, most of the time
    // — so the schema never mentions it; serde must keep it or it silently
    // degrades to a waited call.
    #[serde(default, deserialize_with = "lenient_bool")]
    background: Option<bool>,
    #[serde(default, deserialize_with = "lenient_bool")]
    interactive: Option<bool>,
    shell: Option<String>,
    host: Option<String>,
    timeout_secs: Option<u64>,
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

#[derive(Deserialize)]
pub(super) struct LoadToolsArgs {
    groups: Value,
}

#[derive(Deserialize)]
pub(super) struct CancelDeleteScheduleArgs {
    id: String,
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
pub(super) struct ReadFileArgs {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<String>,
    host: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ReadShellArgs {
    shell: String,
    #[serde(default)]
    lines: Option<u64>,
    #[serde(default)]
    since_command: Option<u32>,
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ReadShellScreenArgs {
    shell: String,
    #[serde(default)]
    lines: Option<u64>,
    #[serde(default)]
    scrollback: Option<u16>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    annotated: bool,
    #[serde(default)]
    wait_for: Option<String>,
    #[serde(default)]
    settle_ms: Option<u64>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct FindInShellsArgs {
    pattern: String,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct GetShellContextArgs {
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ListShellsArgs {
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct OpenShellArgs {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    rows: Option<u64>,
    #[serde(default)]
    cols: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct CloseShellArgs {
    shell: String,
}

#[derive(Deserialize)]
pub(super) struct WatchShellArgs {
    shell: String,
    #[serde(default = "default_300")]
    timeout_secs: u64,
    pattern: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SendInputArgs {
    shell: String,
    #[serde(default)]
    text: String,
    #[serde(default, deserialize_with = "lenient_keys")]
    keys: Vec<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    paste: Option<bool>,
    // `Option` rather than a bool default: `enter` defaults to true only
    // when `keys` is empty, and the default fn cannot see the other fields —
    // `to_event` resolves it.
    #[serde(default, deserialize_with = "lenient_bool")]
    enter: Option<bool>,
}

#[derive(Deserialize)]
pub(super) struct ListShellLogsArgs {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct ReadShellLogArgs {
    log: String,
    #[serde(default)]
    command_index: Option<u32>,
    #[serde(default)]
    lines: Option<u64>,
    #[serde(default)]
    filter: Option<String>,
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
    host: Option<String>,
    #[serde(default)]
    sudo: Option<bool>,
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
pub(super) struct RunLogTailArgs {
    run_id: String,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct RunLogSearchArgs {
    run_id: String,
    query: String,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct GetTurnArgs {
    run_id: String,
    turn: u32,
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
    #[serde(default)]
    host: Option<String>,
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

macro_rules! impl_tool_args {
    ($args:ty => $variant:ident { $($field:ident),* $(,)? }) => {
        impl ToolArgs for $args {
            fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
                AiEvent::Tool(PendingCall::$variant {
                    id: id.to_string(),
                    thought_signature: ts,
                    $($field: self.$field,)*
                })
            }
        }
    };
}

impl ToolArgs for RunTerminalCommandArgs {
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::ToolCall {
            id: id.to_string(),
            cmd: self.command,
            wait: self.wait.or(self.background.map(|b| !b)),
            interactive: self.interactive.unwrap_or(false),
            thought_signature: ts,
            shell: self.shell,
            host: self.host,
            timeout_secs: self.timeout_secs,
        }
    }
}

impl ToolArgs for LoadToolsArgs {
    fn from_value(value: Value) -> Option<Self> {
        let groups = value.get("groups")?.clone();
        Some(Self { groups })
    }
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        let groups = extract_string_vec(&self.groups).unwrap_or_default();
        AiEvent::Tool(PendingCall::LoadTools {
            id: id.to_string(),
            groups,
            thought_signature: ts,
        })
    }
}

/// Build CancelSchedule or DeleteSchedule from the shared arg shape.
pub(super) fn schedule_id_event<T>(args: Value, ts: Option<String>, mk: T) -> Option<AiEvent>
where
    T: FnOnce(String, Option<String>) -> PendingCall,
{
    serde_json::from_value::<CancelDeleteScheduleArgs>(args)
        .ok()
        .map(|a| AiEvent::Tool(mk(a.id, ts)))
}

/// Shared helper for read/delete runbook — same arg shape, different event.
pub(super) fn runbook_name_event<T>(args: Value, ts: Option<String>, mk: T) -> Option<AiEvent>
where
    T: FnOnce(String, Option<String>) -> PendingCall,
{
    ReadRunbookArgs::from_value(args).map(|a| AiEvent::Tool(mk(a.name, ts)))
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

impl ToolArgs for SendInputArgs {
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        // `enter` defaults to true only for a text-only send: `keys:
        // ["Escape"]` must not also press Enter. Resolved here, at the tool
        // boundary, so the event and everything downstream carries the
        // final answer.
        let enter = self.enter.unwrap_or(self.keys.is_empty());
        AiEvent::Tool(PendingCall::SendInput {
            id: id.to_string(),
            shell: self.shell,
            text: self.text,
            keys: self.keys,
            paste: self.paste,
            enter,
            thought_signature: ts,
        })
    }
}

impl ToolArgs for UpdateMemoryArgs {
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::Tool(PendingCall::UpdateMemory {
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
        })
    }
}

impl ToolArgs for CreateAgentArgs {
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent {
        AiEvent::Tool(PendingCall::CreateAgent {
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
        })
    }
}

#[derive(Deserialize)]
pub(super) struct RecallContextArgs {
    pub query: Option<String>,
    pub turn_start: Option<u32>,
    pub turn_end: Option<u32>,
    pub scope: Option<String>,
}

impl_tool_args!(ScheduleCommandArgs => ScheduleCommand {
    name,
    command,
    is_script,
    run_at,
    interval,
    runbook,
    ghost_runbook,
    cron,
});
impl_tool_args!(WriteScriptArgs => WriteScript { script_name, content });
impl_tool_args!(ReadScriptArgs => ReadScript { script_name });
impl_tool_args!(DeleteScriptArgs => DeleteScript { script_name });
impl_tool_args!(ReadFileArgs => ReadFile { path, offset, limit, pattern, host });
impl_tool_args!(ReadShellArgs => ReadShell { shell, lines, since_command, filter });
impl_tool_args!(ReadShellScreenArgs => ReadShellScreen {
    shell,
    lines,
    scrollback,
    pattern,
    annotated,
    wait_for,
    settle_ms,
    timeout_secs,
});
impl_tool_args!(GetShellContextArgs => GetShellContext { scope });
impl_tool_args!(ListShellsArgs => ListShells { scope });
impl_tool_args!(FindInShellsArgs => FindInShells { pattern, scope });
impl_tool_args!(OpenShellArgs => OpenShell { host, rows, cols });
impl_tool_args!(CloseShellArgs => CloseShell { shell });
impl_tool_args!(WatchShellArgs => WatchShell { shell, timeout_secs, pattern });
impl_tool_args!(ListShellLogsArgs => ListShellLogs { since, limit });
impl_tool_args!(ReadShellLogArgs => ReadShellLog { log, command_index, lines, filter });
impl_tool_args!(EditFileArgs => EditFile {
    path,
    operation,
    old_string,
    new_string,
    content,
    dest_path,
    host,
    sudo,
});
impl_tool_args!(WriteRunbookArgs => WriteRunbook { name, content });
impl_tool_args!(ReadRunbookArgs => ReadRunbook { name });
impl_tool_args!(RunLogTailArgs => RunLogTail { run_id, limit });
impl_tool_args!(RunLogSearchArgs => RunLogSearch { run_id, query, limit });
impl_tool_args!(GetTurnArgs => GetTurn { run_id, turn });
impl_tool_args!(AddMemoryArgs => AddMemory { key, value, category });
impl_tool_args!(DeleteMemoryArgs => DeleteMemory { key, category });
impl_tool_args!(ReadMemoryArgs => ReadMemory { key, category });
impl_tool_args!(ListMemoriesArgs => ListMemories { category });
impl_tool_args!(SearchRepositoryArgs => SearchRepository { query, kind });
impl_tool_args!(RecallContextArgs => RecallContext { query, turn_start, turn_end, scope });
impl_tool_args!(SpawnGhostArgs => SpawnGhost { runbook, message, agent, host });
impl_tool_args!(ReadAgentArgs => ReadAgent { name });
impl_tool_args!(AwaitAgentResultArgs => AwaitAgentResult { job_id, agent_name, timeout_secs });

#[cfg(test)]
mod args_tests {
    use super::*;

    #[test]
    fn lenient_bool_rejects_prose_but_takes_string_and_real_booleans() {
        let prose =
            serde_json::json!({"command": "x", "background": "then hostname with host zoidberg"});
        let r = serde_json::from_value::<RunTerminalCommandArgs>(prose);
        assert!(r.is_err(), "lenient_bool must reject prose");

        let strings = serde_json::json!({"command": "x", "wait": "TRUE ", "background": "False"});
        let parsed = serde_json::from_value::<RunTerminalCommandArgs>(strings)
            .expect("trimmed, case-insensitive string booleans parse");
        assert_eq!(parsed.wait, Some(true), "'TRUE ' is true");
        assert_eq!(parsed.background, Some(false), "'False' is false");

        let real = serde_json::json!({"command": "x", "wait": true});
        let parsed = serde_json::from_value::<RunTerminalCommandArgs>(real)
            .expect("a real boolean still parses");
        assert_eq!(parsed.wait, Some(true));

        let absent = serde_json::json!({"command": "x"});
        let parsed = serde_json::from_value::<RunTerminalCommandArgs>(absent)
            .expect("an absent boolean is None");
        assert_eq!(parsed.background, None);
    }

    #[test]
    fn lenient_keys_never_splits_on_separators() {
        let args = serde_json::json!({"shell": "s1", "keys": "C-,"});
        let parsed = serde_json::from_value::<SendInputArgs>(args)
            .expect("a single key name parses verbatim");
        assert_eq!(
            parsed.keys,
            vec!["C-,"],
            "no comma split — C-, is a key name"
        );

        let args = serde_json::json!({"shell": "s1", "keys": ["M- "]});
        let parsed = serde_json::from_value::<SendInputArgs>(args).expect("an array parses");
        assert_eq!(parsed.keys, vec!["M- "], "the space in M- is kept");

        let args = serde_json::json!({"shell": "s1", "keys": "  "});
        let parsed = serde_json::from_value::<SendInputArgs>(args)
            .expect("a whitespace-only string is an empty list");
        assert!(
            parsed.keys.is_empty(),
            "whitespace-only is empty, not one key"
        );

        let args = serde_json::json!({"shell": "s1"});
        let parsed =
            serde_json::from_value::<SendInputArgs>(args).expect("an absent keys is an empty list");
        assert!(parsed.keys.is_empty(), "absent keys defaults to empty");
    }
}
