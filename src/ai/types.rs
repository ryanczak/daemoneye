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
    /// Turn number this message belongs to.  Set when the message is pushed
    /// onto the session history so compaction and `/pin` can identify the
    /// turn boundary even after the message vec has been reindexed.
    /// Legacy messages loaded from pre-upgrade session files have `None` and
    /// cannot be pinned, but remain valid for playback.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub turn: Option<usize>,
}

/// Four-bucket token breakdown for a single AI call.
///
/// Providers report `prompt_tokens` as the *total* tokens sent (including cache
/// reads and writes); the `input_tokens` field here is the **uncached** remainder.
/// Backends must subtract cache totals before populating.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TokenBreakdown {
    /// Billed at input rate (prompt_tokens − cache_read − cache_write).
    #[serde(default)]
    pub input_tokens: u32,
    /// Billed at output rate.
    #[serde(default)]
    pub output_tokens: u32,
    /// Billed at cache-read rate (Anthropic ephemeral cache; Gemini implicit cache).
    #[serde(default)]
    pub cache_read_tokens: u32,
    /// Billed at cache-write rate (Anthropic cache creation).
    #[serde(default)]
    pub cache_write_tokens: u32,
}

impl TokenBreakdown {
    /// Sum of all four buckets.
    pub fn total(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// Uncached input tokens that are billed at the full input rate.
    /// `input_tokens` already excludes cache_read and cache_write after backend
    /// subtraction, so this is simply the field value.
    ///
    /// Note: Phase 3 cost computation must bill all four fields independently
    /// (`input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`)
    /// at their respective rates. This method returns only the non-cached portion
    /// of the input — not the total billing basis.
    pub fn uncached_input_tokens(&self) -> u32 {
        self.input_tokens
    }
}

// Custom deserializer: accepts both `TokenBreakdown` (new) and `AiUsage` (legacy) shapes.
impl<'de> Deserialize<'de> for TokenBreakdown {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};

        struct TokenBreakdownVisitor;

        impl<'de> Visitor<'de> for TokenBreakdownVisitor {
            type Value = TokenBreakdown;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a TokenBreakdown or legacy AiUsage object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<TokenBreakdown, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut input_tokens: Option<u32> = None;
                let mut output_tokens: Option<u32> = None;
                let mut cache_read_tokens: Option<u32> = None;
                let mut cache_write_tokens: Option<u32> = None;
                let mut prompt_tokens: Option<u32> = None;
                let mut completion_tokens: Option<u32> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "input_tokens" => input_tokens = Some(map.next_value()?),
                        "output_tokens" => output_tokens = Some(map.next_value()?),
                        "cache_read_tokens" => cache_read_tokens = Some(map.next_value()?),
                        "cache_write_tokens" => cache_write_tokens = Some(map.next_value()?),
                        "prompt_tokens" => prompt_tokens = Some(map.next_value()?),
                        "completion_tokens" => completion_tokens = Some(map.next_value()?),
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_read_tokens.is_some()
                    || cache_write_tokens.is_some()
                {
                    Ok(TokenBreakdown {
                        input_tokens: input_tokens.unwrap_or(0),
                        output_tokens: output_tokens.unwrap_or(0),
                        cache_read_tokens: cache_read_tokens.unwrap_or(0),
                        cache_write_tokens: cache_write_tokens.unwrap_or(0),
                    })
                } else if let (Some(pt), Some(ct)) = (prompt_tokens, completion_tokens) {
                    Ok(TokenBreakdown {
                        input_tokens: pt,
                        output_tokens: ct,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    })
                } else {
                    Ok(TokenBreakdown::default())
                }
            }
        }

        deserializer.deserialize_map(TokenBreakdownVisitor)
    }
}

/// A tool call collected during AI streaming, to be executed after `Done`.
pub enum PendingCall {
    Foreground {
        id: String,
        thought_signature: Option<String>,
        cmd: String,
        target: Option<String>,
    },
    Background {
        id: String,
        thought_signature: Option<String>,
        cmd: String,
        _credential: Option<String>,
        retry_pane: Option<String>,
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
    WatchPane {
        id: String,
        thought_signature: Option<String>,
        pane_id: String,
        timeout_secs: u64,
        pattern: Option<String>,
    },
    ReadFile {
        id: String,
        thought_signature: Option<String>,
        path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pattern: Option<String>,
        target_pane: Option<String>,
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
        target_pane: Option<String>,
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
        thought_signature: Option<String>,
        pane_id: String,
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
            PendingCall::ScheduleCommand { id, thought_signature, name, command, is_script, run_at, interval, runbook, ghost_runbook, cron } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "schedule_command".to_string(),
                arguments: serde_json::json!({
                    "name": name, "command": command,
                    "is_script": is_script,
                    "run_at": run_at, "interval": interval, "runbook": runbook,
                    "ghost_runbook": ghost_runbook,
                    "cron": cron
                }).to_string(),
            },
            PendingCall::ListSchedules { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_schedules".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::LoadTools { id, thought_signature, groups } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "load_tools".to_string(),
                arguments: serde_json::json!({"groups": groups}).to_string(),
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
            PendingCall::DeleteScript { id, thought_signature, script_name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_script".to_string(),
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
            PendingCall::EditFile { id, thought_signature, path, operation, old_string, new_string, content, dest_path, target_pane } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "edit_file".to_string(),
                arguments: serde_json::json!({"path": path, "operation": operation, "old_string": old_string, "new_string": new_string, "content": content, "dest_path": dest_path, "target_pane": target_pane}).to_string(),
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
            PendingCall::UpdateMemory { id, thought_signature, key, category, body, append, tags, summary, relates_to, expires } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "update_memory".to_string(),
                arguments: serde_json::json!({
                    "key": key,
                    "category": category,
                    "body": body,
                    "append": append,
                    "tags": tags,
                    "summary": summary,
                    "relates_to": relates_to,
                    "expires": expires,
                }).to_string(),
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
            PendingCall::SpawnGhost { id, thought_signature, runbook, message, agent } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "spawn_ghost_shell".to_string(),
                arguments: serde_json::json!({"runbook": runbook, "message": message, "agent": agent}).to_string(),
            },
            PendingCall::CreateAgent { id, thought_signature, name, description, prompt, model, memory_namespace, max_turns, auto_approve_read_only, auto_approve_scripts } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "create_agent".to_string(),
                arguments: serde_json::json!({
                    "name": name, "description": description, "prompt": prompt,
                    "model": model, "memory_namespace": memory_namespace,
                    "max_turns": max_turns, "auto_approve_read_only": auto_approve_read_only,
                    "auto_approve_scripts": auto_approve_scripts
                }).to_string(),
            },
            PendingCall::ReadAgent { id, thought_signature, name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_agent".to_string(),
                arguments: serde_json::json!({"name": name}).to_string(),
            },
            PendingCall::ListAgents { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_agents".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::DeleteAgent { id, thought_signature, name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_agent".to_string(),
                arguments: serde_json::json!({"name": name}).to_string(),
            },
            PendingCall::AwaitAgentResult { id, thought_signature, job_id, agent_name, timeout_secs } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "await_agent_result".to_string(),
                arguments: serde_json::json!({"job_id": job_id, "agent_name": agent_name, "timeout_secs": timeout_secs}).to_string(),
            },
        }
    }

    pub fn id(&self) -> &str {
        match self {
            PendingCall::Foreground { id, .. } => id,
            PendingCall::Background { id, .. } => id,
            PendingCall::ScheduleCommand { id, .. } => id,
            PendingCall::ListSchedules { id, .. } => id,
            PendingCall::LoadTools { id, .. } => id,
            PendingCall::CancelSchedule { id, .. } => id,
            PendingCall::DeleteSchedule { id, .. } => id,
            PendingCall::WriteScript { id, .. } => id,
            PendingCall::ListScripts { id, .. } => id,
            PendingCall::ReadScript { id, .. } => id,
            PendingCall::DeleteScript { id, .. } => id,
            PendingCall::WatchPane { id, .. } => id,
            PendingCall::ReadFile { id, .. } => id,
            PendingCall::EditFile { id, .. } => id,
            PendingCall::WriteRunbook { id, .. } => id,
            PendingCall::DeleteRunbook { id, .. } => id,
            PendingCall::ReadRunbook { id, .. } => id,
            PendingCall::ListRunbooks { id, .. } => id,
            PendingCall::AddMemory { id, .. } => id,
            PendingCall::UpdateMemory { id, .. } => id,
            PendingCall::DeleteMemory { id, .. } => id,
            PendingCall::ReadMemory { id, .. } => id,
            PendingCall::ListMemories { id, .. } => id,
            PendingCall::SearchRepository { id, .. } => id,
            PendingCall::GetTerminalContext { id, .. } => id,
            PendingCall::ListPanes { id, .. } => id,
            PendingCall::CloseBackgroundWindow { id, .. } => id,
            PendingCall::SpawnGhost { id, .. } => id,
            PendingCall::CreateAgent { id, .. } => id,
            PendingCall::ReadAgent { id, .. } => id,
            PendingCall::ListAgents { id, .. } => id,
            PendingCall::DeleteAgent { id, .. } => id,
            PendingCall::AwaitAgentResult { id, .. } => id,
        }
    }

    /// Returns true for silent tools that have no approval prompt or list-panel
    /// response — the executor should emit `ToolStarted`/`ToolFinished` for these.
    pub fn should_emit_tool_feedback(&self) -> bool {
        matches!(
            self,
            PendingCall::CancelSchedule { .. }
                | PendingCall::DeleteSchedule { .. }
                | PendingCall::ReadScript { .. }
                | PendingCall::WatchPane { .. }
                | PendingCall::ReadFile { .. }
                | PendingCall::ReadRunbook { .. }
                | PendingCall::AddMemory { .. }
                | PendingCall::UpdateMemory { .. }
                | PendingCall::DeleteMemory { .. }
                | PendingCall::ReadMemory { .. }
                | PendingCall::ListMemories { .. }
                | PendingCall::SearchRepository { .. }
                | PendingCall::GetTerminalContext { .. }
                | PendingCall::ListPanes { .. }
                | PendingCall::CloseBackgroundWindow { .. }
                | PendingCall::SpawnGhost { .. }
                | PendingCall::ReadAgent { .. }
                | PendingCall::ListAgents { .. }
                | PendingCall::AwaitAgentResult { .. }
                | PendingCall::LoadTools { .. }
        )
    }

    /// Returns a short human-readable argument summary for the `ToolStarted` event.
    pub fn summary(&self) -> String {
        match self {
            PendingCall::CancelSchedule { job_id, .. }
            | PendingCall::DeleteSchedule { job_id, .. } => job_id.clone(),
            PendingCall::ReadScript { script_name, .. }
            | PendingCall::DeleteScript { script_name, .. } => script_name.clone(),
            PendingCall::WatchPane {
                pane_id,
                timeout_secs,
                pattern,
                ..
            } => {
                if let Some(pat) = pattern {
                    format!("{pane_id} pattern=\"{pat}\"")
                } else {
                    format!("{pane_id} {timeout_secs}s")
                }
            }
            PendingCall::ReadFile {
                path,
                offset,
                limit,
                pattern,
                ..
            } => {
                let mut s = path.clone();
                match (offset, limit) {
                    (Some(o), Some(l)) if *o > 0 || *l > 0 => {
                        s.push_str(&format!(" lines={}-{}", o, o + l));
                    }
                    (Some(o), None) if *o > 0 => {
                        s.push_str(&format!(" offset={o}"));
                    }
                    _ => {}
                }
                if let Some(pat) = pattern {
                    s.push_str(&format!(" grep=\"{pat}\""));
                }
                s
            }
            PendingCall::ReadRunbook { name, .. } => name.clone(),
            PendingCall::AddMemory { key, category, .. }
            | PendingCall::UpdateMemory { key, category, .. }
            | PendingCall::DeleteMemory { key, category, .. }
            | PendingCall::ReadMemory { key, category, .. } => {
                format!("{category}/{key}")
            }
            PendingCall::ListMemories { category, .. } => {
                category.as_deref().unwrap_or("all").to_string()
            }
            PendingCall::SearchRepository { query, .. } => {
                let q = if query.len() > 40 {
                    &query[..40]
                } else {
                    query.as_str()
                };
                format!("\"{q}\"")
            }
            PendingCall::GetTerminalContext { .. } => String::new(),
            PendingCall::ListPanes { .. } => String::new(),
            PendingCall::CloseBackgroundWindow { pane_id, .. } => pane_id.clone(),
            PendingCall::SpawnGhost { runbook, .. } => runbook.clone(),
            PendingCall::CreateAgent { name, .. } => name.clone(),
            PendingCall::ReadAgent { name, .. } => name.clone(),
            PendingCall::ListAgents { .. } => String::new(),
            PendingCall::DeleteAgent { name, .. } => name.clone(),
            PendingCall::AwaitAgentResult { job_id, .. } => job_id.clone(),
            PendingCall::LoadTools { groups, .. } => {
                format!("load_tools: {}", groups.join(", "))
            }
            _ => String::new(),
        }
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
            PendingCall::WatchPane { .. } => "watch_pane",
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
            PendingCall::GetTerminalContext { .. } => "get_terminal_context",
            PendingCall::ListPanes { .. } => "list_panes",
            PendingCall::CloseBackgroundWindow { .. } => "close_background_window",
            PendingCall::SpawnGhost { .. } => "spawn_ghost_shell",
            PendingCall::CreateAgent { .. } => "create_agent",
            PendingCall::ReadAgent { .. } => "read_agent",
            PendingCall::ListAgents { .. } => "list_agents",
            PendingCall::DeleteAgent { .. } => "delete_agent",
            PendingCall::AwaitAgentResult { .. } => "await_agent_result",
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    #[test]
    fn message_roundtrip_plain() {
        let msg = user_msg("test content");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "user");
        assert_eq!(back.content, "test content");
        assert!(back.tool_calls.is_none());
        assert!(back.tool_results.is_none());
    }

    #[test]
    fn message_tool_calls_skipped_when_none() {
        let msg = user_msg("hi");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_results"));
    }

    #[test]
    fn tool_call_roundtrip() {
        let tc = ToolCall {
            id: "tc_99".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"echo hi","background":true}"#.to_string(),
            thought_signature: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "tc_99");
        assert_eq!(back.name, "run_terminal_command");
    }

    // ── should_emit_tool_feedback ────────────────────────────────────────

    fn mk_read_file(path: &str) -> PendingCall {
        PendingCall::ReadFile {
            id: "x".to_string(),
            thought_signature: None,
            path: path.to_string(),
            offset: None,
            limit: None,
            pattern: None,
            target_pane: None,
        }
    }

    fn mk_foreground(cmd: &str) -> PendingCall {
        PendingCall::Foreground {
            id: "x".to_string(),
            thought_signature: None,
            cmd: cmd.to_string(),
            target: None,
        }
    }

    #[test]
    fn should_emit_tool_feedback_silent_tools_true() {
        // All 16 silent tools should return true.
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
            PendingCall::WatchPane {
                id: "x".to_string(),
                thought_signature: None,
                pane_id: "%1".to_string(),
                timeout_secs: 30,
                pattern: None,
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
            PendingCall::GetTerminalContext {
                id: "x".to_string(),
                thought_signature: None,
            },
            PendingCall::ListPanes {
                id: "x".to_string(),
                thought_signature: None,
            },
            PendingCall::CloseBackgroundWindow {
                id: "x".to_string(),
                thought_signature: None,
                pane_id: "%5".to_string(),
            },
            PendingCall::SpawnGhost {
                id: "x".to_string(),
                thought_signature: None,
                runbook: "disk-alert".to_string(),
                message: "investigate".to_string(),
                agent: None,
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
                retry_pane: None,
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
                target_pane: None,
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
            target_pane: None,
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
            target_pane: None,
        };
        assert_eq!(call.summary(), "/etc/hosts grep=\"nameserver\"");
    }

    #[test]
    fn summary_watch_pane_with_pattern() {
        let call = PendingCall::WatchPane {
            id: "x".to_string(),
            thought_signature: None,
            pane_id: "%3".to_string(),
            timeout_secs: 60,
            pattern: Some("DONE".to_string()),
        };
        assert_eq!(call.summary(), "%3 pattern=\"DONE\"");
    }

    #[test]
    fn summary_watch_pane_no_pattern() {
        let call = PendingCall::WatchPane {
            id: "x".to_string(),
            thought_signature: None,
            pane_id: "%3".to_string(),
            timeout_secs: 30,
            pattern: None,
        };
        assert_eq!(call.summary(), "%3 30s");
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
        };
        assert_eq!(call.summary(), "disk-alert");
    }

    #[test]
    fn summary_get_terminal_context_empty() {
        let call = PendingCall::GetTerminalContext {
            id: "x".to_string(),
            thought_signature: None,
        };
        assert!(call.summary().is_empty());
    }

    // ── TokenBreakdown ────────────────────────────────────────────────────

    #[test]
    fn token_breakdown_total_sums_all_buckets() {
        let tb = TokenBreakdown {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 800,
            cache_write_tokens: 200,
        };
        assert_eq!(tb.total(), 2500);
    }

    #[test]
    fn token_breakdown_zero_tokens_is_zero() {
        let tb = TokenBreakdown::default();
        assert_eq!(tb.total(), 0);
        assert_eq!(tb.uncached_input_tokens(), 0);
    }

    #[test]
    fn token_breakdown_uncached_input_tokens_returns_input_field() {
        let tb = TokenBreakdown {
            input_tokens: 300,
            output_tokens: 100,
            cache_read_tokens: 500,
            cache_write_tokens: 100,
        };
        assert_eq!(tb.uncached_input_tokens(), 300);
    }

    #[test]
    fn token_breakdown_serializes_all_fields() {
        let tb = TokenBreakdown {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
        };
        let json = serde_json::to_string(&tb).unwrap();
        assert!(json.contains("\"input_tokens\":10"));
        assert!(json.contains("\"output_tokens\":20"));
        assert!(json.contains("\"cache_read_tokens\":30"));
        assert!(json.contains("\"cache_write_tokens\":40"));
    }

    #[test]
    fn legacy_ai_usage_jsonl_deserializes_into_token_breakdown() {
        let legacy_json = r#"{"prompt_tokens":1500,"completion_tokens":800}"#;
        let tb: TokenBreakdown = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(tb.input_tokens, 1500);
        assert_eq!(tb.output_tokens, 800);
        assert_eq!(tb.cache_read_tokens, 0);
        assert_eq!(tb.cache_write_tokens, 0);
        assert_eq!(tb.total(), 2300);
    }

    #[test]
    fn token_breakdown_new_format_deserializes_directly() {
        let new_json = r#"{
            "input_tokens": 200,
            "output_tokens": 100,
            "cache_read_tokens": 500,
            "cache_write_tokens": 50
        }"#;
        let tb: TokenBreakdown = serde_json::from_str(new_json).unwrap();
        assert_eq!(tb.input_tokens, 200);
        assert_eq!(tb.output_tokens, 100);
        assert_eq!(tb.cache_read_tokens, 500);
        assert_eq!(tb.cache_write_tokens, 50);
    }

    #[test]
    fn token_breakdown_zero_cache_when_provider_omits_field() {
        let partial_json = r#"{"input_tokens":100,"output_tokens":50}"#;
        let tb: TokenBreakdown = serde_json::from_str(partial_json).unwrap();
        assert_eq!(tb.cache_read_tokens, 0);
        assert_eq!(tb.cache_write_tokens, 0);
        assert_eq!(tb.input_tokens, 100);
        assert_eq!(tb.output_tokens, 50);
    }
}
