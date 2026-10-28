use super::AiEvent;
use super::wire::ToolCall;

/// A tool call collected during AI streaming, to be executed after `Done`.
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

/// Serialize a tool-call argument object, omitting entries whose value is
/// JSON `null`. Optional parameters the model did not pass must not be
/// echoed back into conversation history: models imitate `"grep": null`
/// as the string `"null"` on later calls, which then filters pane reads
/// to lines containing "null".
fn args_to_string(mut v: serde_json::Value) -> String {
    if let serde_json::Value::Object(map) = &mut v {
        map.retain(|_, val| !val.is_null());
    }
    v.to_string()
}

impl PendingCall {
    pub fn to_tool_call(&self) -> ToolCall {
        match self {
            PendingCall::Foreground {
                id,
                thought_signature,
                cmd,
                shell,
                host,
                timeout_secs,
                interactive,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "command": cmd,
                    "background": false,
                    "interactive": interactive,
                    "shell": shell,
                    "host": host,
                    "timeout_secs": timeout_secs
                })),
            },
            PendingCall::Background {
                id,
                thought_signature,
                cmd,
                host,
                ..
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "command": cmd,
                    "background": true,
                    "host": host
                })),
            },
            PendingCall::ScheduleCommand {
                id,
                thought_signature,
                name,
                command,
                is_script,
                run_at,
                interval,
                runbook,
                ghost_runbook,
                cron,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "schedule_command".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "name": name, "command": command,
                    "is_script": is_script,
                    "run_at": run_at, "interval": interval, "runbook": runbook,
                    "ghost_runbook": ghost_runbook,
                    "cron": cron
                })),
            },
            PendingCall::ListSchedules {
                id,
                thought_signature,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_schedules".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::InvalidArgs {
                id,
                name,
                arguments,
                thought_signature,
                ..
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: name.to_string(),
                arguments: arguments.clone(),
            },
            PendingCall::LoadTools {
                id,
                thought_signature,
                groups,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "load_tools".to_string(),
                arguments: args_to_string(serde_json::json!({"groups": groups})),
            },
            PendingCall::CancelSchedule {
                id,
                thought_signature,
                job_id,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "cancel_schedule".to_string(),
                arguments: args_to_string(serde_json::json!({"id": job_id})),
            },
            PendingCall::DeleteSchedule {
                id,
                thought_signature,
                job_id,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_schedule".to_string(),
                arguments: args_to_string(serde_json::json!({"id": job_id})),
            },
            PendingCall::WriteScript {
                id,
                thought_signature,
                script_name,
                content,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "write_script".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"script_name": script_name, "content": content}),
                ),
            },
            PendingCall::ListScripts {
                id,
                thought_signature,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_scripts".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::ReadScript {
                id,
                thought_signature,
                script_name,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_script".to_string(),
                arguments: args_to_string(serde_json::json!({"script_name": script_name})),
            },
            PendingCall::DeleteScript {
                id,
                thought_signature,
                script_name,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_script".to_string(),
                arguments: args_to_string(serde_json::json!({"script_name": script_name})),
            },
            PendingCall::WatchShell {
                id,
                thought_signature,
                shell,
                timeout_secs,
                pattern,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "watch_shell".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"shell": shell, "timeout_secs": timeout_secs, "pattern": pattern}),
                ),
            },
            PendingCall::SendInput {
                id,
                thought_signature,
                shell,
                text,
                keys,
                paste,
                enter,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "send_input".to_string(),
                arguments: args_to_string(
                    // Every field goes back in: a resumed or replayed history
                    // that dropped `keys` or `paste` would re-run a different
                    // call from the one the user approved.
                    serde_json::json!({"shell": shell, "text": text, "keys": keys, "paste": paste, "enter": enter}),
                ),
            },
            PendingCall::ListShellLogs {
                id,
                thought_signature,
                since,
                limit,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_shell_logs".to_string(),
                arguments: args_to_string(serde_json::json!({"since": since, "limit": limit})),
            },
            PendingCall::ReadShellLog {
                id,
                thought_signature,
                log,
                command_index,
                lines,
                filter,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_shell_log".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "log": log,
                    "command_index": command_index,
                    "lines": lines,
                    "filter": filter,
                })),
            },
            PendingCall::ReadFile {
                id,
                thought_signature,
                path,
                offset,
                limit,
                pattern,
                host,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_file".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"path": path, "offset": offset, "limit": limit, "pattern": pattern, "host": host}),
                ),
            },
            PendingCall::EditFile {
                id,
                thought_signature,
                path,
                operation,
                old_string,
                new_string,
                content,
                dest_path,
                host,
                sudo,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "edit_file".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"path": path, "operation": operation, "old_string": old_string, "new_string": new_string, "content": content, "dest_path": dest_path, "host": host, "sudo": sudo}),
                ),
            },
            PendingCall::WriteRunbook {
                id,
                thought_signature,
                name,
                content,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "write_runbook".to_string(),
                arguments: args_to_string(serde_json::json!({"name": name, "content": content})),
            },
            PendingCall::DeleteRunbook {
                id,
                thought_signature,
                name,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_runbook".to_string(),
                arguments: args_to_string(serde_json::json!({"name": name})),
            },
            PendingCall::ReadRunbook {
                id,
                thought_signature,
                name,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_runbook".to_string(),
                arguments: args_to_string(serde_json::json!({"name": name})),
            },
            PendingCall::ListRunbooks {
                id,
                thought_signature,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_runbooks".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::RunLogTail {
                id,
                thought_signature,
                run_id,
                limit,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_log_tail".to_string(),
                arguments: args_to_string(serde_json::json!({"run_id": run_id, "limit": limit})),
            },
            PendingCall::RunLogSearch {
                id,
                thought_signature,
                run_id,
                query,
                limit,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_log_search".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"run_id": run_id, "query": query, "limit": limit}),
                ),
            },
            PendingCall::GetTurn {
                id,
                thought_signature,
                run_id,
                turn,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "get_turn".to_string(),
                arguments: args_to_string(serde_json::json!({"run_id": run_id, "turn": turn})),
            },
            PendingCall::AddMemory {
                id,
                thought_signature,
                key,
                value,
                category,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "add_memory".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"key": key, "value": value, "category": category}),
                ),
            },
            PendingCall::UpdateMemory {
                id,
                thought_signature,
                key,
                category,
                body,
                append,
                tags,
                summary,
                relates_to,
                expires,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "update_memory".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "key": key,
                    "category": category,
                    "body": body,
                    "append": append,
                    "tags": tags,
                    "summary": summary,
                    "relates_to": relates_to,
                    "expires": expires,
                })),
            },
            PendingCall::DeleteMemory {
                id,
                thought_signature,
                key,
                category,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_memory".to_string(),
                arguments: args_to_string(serde_json::json!({"key": key, "category": category})),
            },
            PendingCall::ReadMemory {
                id,
                thought_signature,
                key,
                category,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_memory".to_string(),
                arguments: args_to_string(serde_json::json!({"key": key, "category": category})),
            },
            PendingCall::ListMemories {
                id,
                thought_signature,
                category,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_memories".to_string(),
                arguments: args_to_string(serde_json::json!({"category": category})),
            },
            PendingCall::SearchRepository {
                id,
                thought_signature,
                query,
                kind,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "search_repository".to_string(),
                arguments: args_to_string(serde_json::json!({"query": query, "kind": kind})),
            },
            PendingCall::RecallContext {
                id,
                thought_signature,
                query,
                turn_start,
                turn_end,
                scope,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "recall_context".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"query": query, "turn_start": turn_start, "turn_end": turn_end, "scope": scope}),
                ),
            },
            PendingCall::GetShellContext {
                id,
                thought_signature,
                scope,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "get_shell_context".to_string(),
                arguments: args_to_string(serde_json::json!({"scope": scope})),
            },
            PendingCall::ListShells {
                id,
                thought_signature,
                scope,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_shells".to_string(),
                arguments: args_to_string(serde_json::json!({"scope": scope})),
            },
            PendingCall::SpawnGhost {
                id,
                thought_signature,
                runbook,
                message,
                agent,
                host,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "spawn_ghost_shell".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "runbook": runbook,
                    "message": message,
                    "agent": agent,
                    "host": host
                })),
            },
            PendingCall::CreateAgent {
                id,
                thought_signature,
                name,
                description,
                prompt,
                model,
                memory_namespace,
                max_turns,
                auto_approve_read_only,
                auto_approve_scripts,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "create_agent".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "name": name, "description": description, "prompt": prompt,
                    "model": model, "memory_namespace": memory_namespace,
                    "max_turns": max_turns, "auto_approve_read_only": auto_approve_read_only,
                    "auto_approve_scripts": auto_approve_scripts
                })),
            },
            PendingCall::ReadAgent {
                id,
                thought_signature,
                name,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_agent".to_string(),
                arguments: args_to_string(serde_json::json!({"name": name})),
            },
            PendingCall::ListAgents {
                id,
                thought_signature,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_agents".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::DeleteAgent {
                id,
                thought_signature,
                name,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_agent".to_string(),
                arguments: args_to_string(serde_json::json!({"name": name})),
            },
            PendingCall::AwaitAgentResult {
                id,
                thought_signature,
                job_id,
                agent_name,
                timeout_secs,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "await_agent_result".to_string(),
                arguments: args_to_string(
                    serde_json::json!({"job_id": job_id, "agent_name": agent_name, "timeout_secs": timeout_secs}),
                ),
            },
            PendingCall::ReadShell {
                id,
                thought_signature,
                shell,
                lines,
                since_command,
                filter,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_shell".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "shell": shell,
                    "lines": lines,
                    "since_command": since_command,
                    "filter": filter,
                })),
            },
            PendingCall::ReadShellScreen {
                id,
                thought_signature,
                shell,
                lines,
                scrollback,
                pattern,
                annotated,
                wait_for,
                settle_ms,
                timeout_secs,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_shell_screen".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "shell": shell,
                    "lines": lines,
                    "scrollback": scrollback,
                    "pattern": pattern,
                    "annotated": annotated,
                    "wait_for": wait_for,
                    "settle_ms": settle_ms,
                    "timeout_secs": timeout_secs,
                })),
            },
            PendingCall::FindInShells {
                id,
                thought_signature,
                pattern,
                scope,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "find_in_shells".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "pattern": pattern,
                    "scope": scope,
                })),
            },
            PendingCall::OpenShell {
                id,
                thought_signature,
                host,
                rows,
                cols,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "open_shell".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "host": host,
                    "rows": rows,
                    "cols": cols,
                })),
            },
            PendingCall::CloseShell {
                id,
                thought_signature,
                shell,
            } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "close_shell".to_string(),
                arguments: args_to_string(serde_json::json!({
                    "shell": shell,
                })),
            },
        }
    }

    pub fn id(&self) -> &str {
        match self {
            PendingCall::Foreground { id, .. } => id,
            PendingCall::Background { id, .. } => id,
            PendingCall::ScheduleCommand { id, .. } => id,
            PendingCall::InvalidArgs { id, .. } => id,
            PendingCall::ListSchedules { id, .. } => id,
            PendingCall::LoadTools { id, .. } => id,
            PendingCall::CancelSchedule { id, .. } => id,
            PendingCall::DeleteSchedule { id, .. } => id,
            PendingCall::WriteScript { id, .. } => id,
            PendingCall::ListScripts { id, .. } => id,
            PendingCall::ReadScript { id, .. } => id,
            PendingCall::DeleteScript { id, .. } => id,
            PendingCall::WatchShell { id, .. } => id,
            PendingCall::SendInput { id, .. } => id,
            PendingCall::ListShellLogs { id, .. } => id,
            PendingCall::ReadShellLog { id, .. } => id,
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
            PendingCall::RecallContext { id, .. } => id,
            PendingCall::GetShellContext { id, .. } => id,
            PendingCall::ListShells { id, .. } => id,
            PendingCall::OpenShell { id, .. } => id,
            PendingCall::CloseShell { id, .. } => id,
            PendingCall::SpawnGhost { id, .. } => id,
            PendingCall::CreateAgent { id, .. } => id,
            PendingCall::ReadAgent { id, .. } => id,
            PendingCall::ListAgents { id, .. } => id,
            PendingCall::DeleteAgent { id, .. } => id,
            PendingCall::AwaitAgentResult { id, .. } => id,
            PendingCall::ReadShell { id, .. } => id,
            PendingCall::ReadShellScreen { id, .. } => id,
            PendingCall::FindInShells { id, .. } => id,
            PendingCall::RunLogTail { id, .. } => id,
            PendingCall::RunLogSearch { id, .. } => id,
            PendingCall::GetTurn { id, .. } => id,
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

    /// Returns a short human-readable argument summary for the `ToolStarted` event.
    pub fn summary(&self) -> String {
        match self {
            PendingCall::CancelSchedule { job_id, .. }
            | PendingCall::DeleteSchedule { job_id, .. } => job_id.clone(),
            PendingCall::ReadScript { script_name, .. }
            | PendingCall::DeleteScript { script_name, .. } => script_name.clone(),
            PendingCall::WatchShell {
                shell,
                timeout_secs,
                pattern,
                ..
            } => {
                if let Some(pat) = pattern {
                    format!("{shell} pattern=\"{pat}\"")
                } else {
                    format!("{shell} {timeout_secs}s")
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
            // A foreground run may name the shell it targets and the host it
            // runs on; both are what the user watching the line needs to know
            // — a panel that says only "run a command" for a remote command is
            // the wrong summary.
            PendingCall::Foreground {
                cmd, shell, host, ..
            } => {
                let q = if cmd.len() > 40 {
                    &cmd[..40]
                } else {
                    cmd.as_str()
                };
                match (shell, host) {
                    (Some(s), Some(h)) => format!("\"{q}\" in {s} on {h}"),
                    (Some(s), None) => format!("\"{q}\" in {s}"),
                    (None, Some(h)) => format!("\"{q}\" on {h}"),
                    (None, None) => format!("\"{q}\""),
                }
            }
            PendingCall::GetShellContext { scope, .. } => scope.clone().unwrap_or_default(),
            PendingCall::ListShells { scope, .. } => scope.clone().unwrap_or_default(),
            // A close shows which shell; an open shows which host, when one
            // was named — the ordinary case is local, i.e. nothing to show.
            PendingCall::OpenShell {
                host,
                rows: Some(r),
                cols: Some(c),
                ..
            } => format!("{host:?} rows={r} cols={c}"),
            PendingCall::OpenShell { host, .. } => host.clone().unwrap_or_default(),
            PendingCall::CloseShell { shell, .. } => shell.clone(),
            PendingCall::SendInput { shell, .. } => shell.clone(),
            PendingCall::SpawnGhost { runbook, .. } => runbook.clone(),
            PendingCall::CreateAgent { name, .. } => name.clone(),
            PendingCall::ReadAgent { name, .. } => name.clone(),
            PendingCall::ListAgents { .. } => String::new(),
            PendingCall::DeleteAgent { name, .. } => name.clone(),
            PendingCall::AwaitAgentResult { job_id, .. } => job_id.clone(),
            PendingCall::ReadShell {
                shell,
                lines,
                since_command,
                filter,
                ..
            } => {
                let mut s = shell.clone();
                if let Some(n) = lines {
                    s.push_str(&format!(" lines={n}"));
                }
                if let Some(i) = since_command {
                    s.push_str(&format!(" since_command={i}"));
                }
                if let Some(f) = filter {
                    s.push_str(&format!(" filter=\"{f}\""));
                }
                s
            }
            PendingCall::ReadShellScreen {
                shell,
                lines,
                scrollback,
                pattern,
                wait_for,
                settle_ms,
                ..
            } => {
                let mut s = shell.clone();
                if let Some(n) = lines {
                    s.push_str(&format!(" lines={n}"));
                }
                if let Some(n) = scrollback {
                    s.push_str(&format!(" scrollback={n}"));
                }
                if let Some(p) = pattern {
                    s.push_str(&format!(" pattern=\"{p}\""));
                }
                if let Some(p) = wait_for {
                    s.push_str(&format!(" wait_for=\"{p}\""));
                }
                if let Some(n) = settle_ms {
                    s.push_str(&format!(" settle_ms={n}"));
                }
                s
            }
            PendingCall::FindInShells { pattern, scope, .. } => match scope {
                Some(s) => format!("/{pattern}/ scope={s}"),
                None => format!("/{pattern}/"),
            },
            PendingCall::ListShellLogs { since, limit, .. } => match (since, limit) {
                (Some(s), Some(l)) => format!("since {s} limit={l}"),
                (Some(s), None) => format!("since {s}"),
                (None, Some(l)) => format!("limit={l}"),
                (None, None) => String::new(),
            },
            PendingCall::ReadShellLog {
                log,
                command_index,
                lines,
                filter,
                ..
            } => {
                let mut s = log.clone();
                if let Some(i) = command_index {
                    s.push_str(&format!(" command_index={i}"));
                }
                if let Some(n) = lines {
                    s.push_str(&format!(" lines={n}"));
                }
                if let Some(f) = filter {
                    s.push_str(&format!(" filter=\"{f}\""));
                }
                s
            }
            PendingCall::LoadTools { groups, .. } => {
                format!("load_tools: {}", groups.join(", "))
            }
            PendingCall::RunLogTail { run_id, limit, .. } => match limit {
                Some(n) => format!("tail {run_id} limit={n}"),
                None => format!("tail {run_id} (default limit)"),
            },
            PendingCall::RunLogSearch { run_id, query, .. } => {
                format!("search {run_id} \"{query}\"")
            }
            PendingCall::GetTurn { run_id, turn, .. } => {
                format!("turn {turn} of {run_id}")
            }
            PendingCall::InvalidArgs { name, .. } => format!("invalid arguments for {name}"),
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
    /// The `PendingCall` a tool event becomes, field for field. `None` for
    /// `run_terminal_command` (`AiEvent::ToolCall`, whose wait and
    /// interactive arguments pick `Foreground` or `Background` in
    /// `run_conversation_loop`) and for the four events that are not tool
    /// calls. Exhaustive by design: a new `AiEvent` variant lands here.
    pub(crate) fn from_tool_event(event: AiEvent) -> Option<PendingCall> {
        match event {
            AiEvent::InvalidToolArgs {
                id,
                name,
                arguments,
                error,
                thought_signature,
            } => Some(Self::InvalidArgs {
                id,
                name,
                arguments,
                error,
                thought_signature,
            }),
            AiEvent::ScheduleCommand {
                id,
                name,
                command,
                is_script,
                run_at,
                interval,
                runbook,
                ghost_runbook,
                cron,
                thought_signature,
            } => Some(Self::ScheduleCommand {
                id,
                name,
                command,
                is_script,
                run_at,
                interval,
                runbook,
                ghost_runbook,
                cron,
                thought_signature,
            }),
            AiEvent::LoadTools {
                id,
                groups,
                thought_signature,
            } => Some(Self::LoadTools {
                id,
                groups,
                thought_signature,
            }),
            AiEvent::ListSchedules {
                id,
                thought_signature,
            } => Some(Self::ListSchedules {
                id,
                thought_signature,
            }),
            AiEvent::CancelSchedule {
                id,
                job_id,
                thought_signature,
            } => Some(Self::CancelSchedule {
                id,
                job_id,
                thought_signature,
            }),
            AiEvent::DeleteSchedule {
                id,
                job_id,
                thought_signature,
            } => Some(Self::DeleteSchedule {
                id,
                job_id,
                thought_signature,
            }),
            AiEvent::WriteScript {
                id,
                script_name,
                content,
                thought_signature,
            } => Some(Self::WriteScript {
                id,
                script_name,
                content,
                thought_signature,
            }),
            AiEvent::ListScripts {
                id,
                thought_signature,
            } => Some(Self::ListScripts {
                id,
                thought_signature,
            }),
            AiEvent::ReadScript {
                id,
                script_name,
                thought_signature,
            } => Some(Self::ReadScript {
                id,
                script_name,
                thought_signature,
            }),
            AiEvent::DeleteScript {
                id,
                script_name,
                thought_signature,
            } => Some(Self::DeleteScript {
                id,
                script_name,
                thought_signature,
            }),
            AiEvent::ReadFile {
                id,
                path,
                offset,
                limit,
                pattern,
                host,
                thought_signature,
            } => Some(Self::ReadFile {
                id,
                thought_signature,
                path,
                offset,
                limit,
                pattern,
                host,
            }),
            AiEvent::EditFile {
                id,
                path,
                operation,
                old_string,
                new_string,
                content,
                dest_path,
                host,
                sudo,
                thought_signature,
            } => Some(Self::EditFile {
                id,
                thought_signature,
                path,
                operation,
                old_string,
                new_string,
                content,
                dest_path,
                host,
                sudo,
            }),
            AiEvent::WriteRunbook {
                id,
                name,
                content,
                thought_signature,
            } => Some(Self::WriteRunbook {
                id,
                thought_signature,
                name,
                content,
            }),
            AiEvent::DeleteRunbook {
                id,
                name,
                thought_signature,
            } => Some(Self::DeleteRunbook {
                id,
                thought_signature,
                name,
            }),
            AiEvent::ReadRunbook {
                id,
                name,
                thought_signature,
            } => Some(Self::ReadRunbook {
                id,
                thought_signature,
                name,
            }),
            AiEvent::ListRunbooks {
                id,
                thought_signature,
            } => Some(Self::ListRunbooks {
                id,
                thought_signature,
            }),
            AiEvent::AddMemory {
                id,
                key,
                value,
                category,
                thought_signature,
            } => Some(Self::AddMemory {
                id,
                thought_signature,
                key,
                value,
                category,
            }),
            AiEvent::UpdateMemory {
                id,
                key,
                category,
                body,
                append,
                tags,
                summary,
                relates_to,
                expires,
                thought_signature,
            } => Some(Self::UpdateMemory {
                id,
                thought_signature,
                key,
                category,
                body,
                append,
                tags,
                summary,
                relates_to,
                expires,
            }),
            AiEvent::DeleteMemory {
                id,
                key,
                category,
                thought_signature,
            } => Some(Self::DeleteMemory {
                id,
                thought_signature,
                key,
                category,
            }),
            AiEvent::ReadMemory {
                id,
                key,
                category,
                thought_signature,
            } => Some(Self::ReadMemory {
                id,
                thought_signature,
                key,
                category,
            }),
            AiEvent::ListMemories {
                id,
                category,
                thought_signature,
            } => Some(Self::ListMemories {
                id,
                thought_signature,
                category,
            }),
            AiEvent::SearchRepository {
                id,
                query,
                kind,
                thought_signature,
            } => Some(Self::SearchRepository {
                id,
                thought_signature,
                query,
                kind,
            }),
            AiEvent::RecallContext {
                id,
                query,
                turn_start,
                turn_end,
                scope,
                thought_signature,
            } => Some(Self::RecallContext {
                id,
                thought_signature,
                query,
                turn_start,
                turn_end,
                scope,
            }),
            AiEvent::GetShellContext {
                id,
                thought_signature,
                scope,
            } => Some(Self::GetShellContext {
                id,
                thought_signature,
                scope,
            }),
            AiEvent::ListShells {
                id,
                thought_signature,
                scope,
            } => Some(Self::ListShells {
                id,
                thought_signature,
                scope,
            }),
            AiEvent::ReadShell {
                id,
                shell,
                lines,
                since_command,
                filter,
                thought_signature,
            } => Some(Self::ReadShell {
                id,
                thought_signature,
                shell,
                lines,
                since_command,
                filter,
            }),
            AiEvent::ReadShellScreen {
                id,
                shell,
                lines,
                scrollback,
                pattern,
                annotated,
                wait_for,
                settle_ms,
                timeout_secs,
                thought_signature,
            } => Some(Self::ReadShellScreen {
                id,
                thought_signature,
                shell,
                lines,
                scrollback,
                pattern,
                annotated,
                wait_for,
                settle_ms,
                timeout_secs,
            }),
            AiEvent::FindInShells {
                id,
                pattern,
                scope,
                thought_signature,
            } => Some(Self::FindInShells {
                id,
                thought_signature,
                pattern,
                scope,
            }),
            AiEvent::OpenShell {
                id,
                host,
                rows,
                cols,
                thought_signature,
            } => Some(Self::OpenShell {
                id,
                host,
                rows,
                cols,
                thought_signature,
            }),
            AiEvent::CloseShell {
                id,
                shell,
                thought_signature,
            } => Some(Self::CloseShell {
                id,
                thought_signature,
                shell,
            }),
            AiEvent::WatchShell {
                id,
                shell,
                timeout_secs,
                pattern,
                thought_signature,
            } => Some(Self::WatchShell {
                id,
                thought_signature,
                shell,
                timeout_secs,
                pattern,
            }),
            AiEvent::SendInput {
                id,
                shell,
                text,
                keys,
                paste,
                enter,
                thought_signature,
            } => Some(Self::SendInput {
                id,
                thought_signature,
                shell,
                text,
                keys,
                paste,
                enter,
            }),
            AiEvent::ListShellLogs {
                id,
                since,
                limit,
                thought_signature,
            } => Some(Self::ListShellLogs {
                id,
                thought_signature,
                since,
                limit,
            }),
            AiEvent::ReadShellLog {
                id,
                log,
                command_index,
                lines,
                filter,
                thought_signature,
            } => Some(Self::ReadShellLog {
                id,
                thought_signature,
                log,
                command_index,
                lines,
                filter,
            }),
            AiEvent::SpawnGhost {
                id,
                runbook,
                message,
                agent,
                host,
                thought_signature,
            } => Some(Self::SpawnGhost {
                id,
                thought_signature,
                runbook,
                message,
                agent,
                host,
            }),
            AiEvent::CreateAgent {
                id,
                name,
                description,
                prompt,
                model,
                memory_namespace,
                max_turns,
                auto_approve_read_only,
                auto_approve_scripts,
                thought_signature,
            } => Some(Self::CreateAgent {
                id,
                thought_signature,
                name,
                description,
                prompt,
                model,
                memory_namespace,
                max_turns,
                auto_approve_read_only,
                auto_approve_scripts,
            }),
            AiEvent::ReadAgent {
                id,
                name,
                thought_signature,
            } => Some(Self::ReadAgent {
                id,
                thought_signature,
                name,
            }),
            AiEvent::ListAgents {
                id,
                thought_signature,
            } => Some(Self::ListAgents {
                id,
                thought_signature,
            }),
            AiEvent::DeleteAgent {
                id,
                name,
                thought_signature,
            } => Some(Self::DeleteAgent {
                id,
                thought_signature,
                name,
            }),
            AiEvent::AwaitAgentResult {
                id,
                job_id,
                agent_name,
                timeout_secs,
                thought_signature,
            } => Some(Self::AwaitAgentResult {
                id,
                thought_signature,
                job_id,
                agent_name,
                timeout_secs,
            }),
            AiEvent::RunLogTail {
                id,
                run_id,
                limit,
                thought_signature,
            } => Some(Self::RunLogTail {
                id,
                thought_signature,
                run_id,
                limit,
            }),
            AiEvent::RunLogSearch {
                id,
                run_id,
                query,
                limit,
                thought_signature,
            } => Some(Self::RunLogSearch {
                id,
                thought_signature,
                run_id,
                query,
                limit,
            }),
            AiEvent::GetTurn {
                id,
                run_id,
                turn,
                thought_signature,
            } => Some(Self::GetTurn {
                id,
                thought_signature,
                run_id,
                turn,
            }),
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
