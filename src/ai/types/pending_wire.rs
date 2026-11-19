use serde_json::{Value, json};

use super::PendingCall;
use super::wire::ToolCall;

/// Serialize a tool-call argument object, omitting entries whose value is
/// JSON `null`. Optional parameters the model did not pass must not be
/// echoed back into conversation history: models imitate `"grep": null`
/// as the string `"null"` on later calls, which then filters pane reads
/// to lines containing "null".
fn args_to_string(mut v: Value) -> String {
    if let Value::Object(map) = &mut v {
        map.retain(|_, val| !val.is_null());
    }
    v.to_string()
}

impl PendingCall {
    pub fn to_tool_call(&self) -> ToolCall {
        let arguments = match self {
            PendingCall::InvalidArgs { arguments, .. } => arguments.clone(),
            _ => args_to_string(self.arguments_value()),
        };
        ToolCall {
            id: self.id().to_string(),
            thought_signature: self.thought_signature().map(str::to_string),
            name: self.tool_name().to_string(),
            arguments,
        }
    }

    fn arguments_value(&self) -> Value {
        if let Some(v) = self.shell_arguments_value() {
            return v;
        }
        if let Some(v) = self.knowledge_arguments_value() {
            return v;
        }
        match self {
            PendingCall::ScheduleCommand {
                name,
                command,
                is_script,
                run_at,
                interval,
                runbook,
                ghost_runbook,
                cron,
                ..
            } => json!({
                "name": name,
                "command": command,
                "is_script": is_script,
                "run_at": run_at,
                "interval": interval,
                "runbook": runbook,
                "ghost_runbook": ghost_runbook,
                "cron": cron,
            }),
            PendingCall::ListSchedules { .. }
            | PendingCall::ListScripts { .. }
            | PendingCall::ListRunbooks { .. }
            | PendingCall::ListAgents { .. }
            | PendingCall::InvalidArgs { .. } => json!({}),
            PendingCall::LoadTools { groups, .. } => json!({ "groups": groups }),
            PendingCall::CancelSchedule { job_id, .. }
            | PendingCall::DeleteSchedule { job_id, .. } => json!({ "id": job_id }),
            PendingCall::WriteScript {
                script_name,
                content,
                ..
            } => json!({ "script_name": script_name, "content": content }),
            PendingCall::ReadScript { script_name, .. }
            | PendingCall::DeleteScript { script_name, .. } => {
                json!({ "script_name": script_name })
            }
            PendingCall::ReadFile {
                path,
                offset,
                limit,
                pattern,
                host,
                ..
            } => json!({
                "path": path,
                "offset": offset,
                "limit": limit,
                "pattern": pattern,
                "host": host,
            }),
            PendingCall::EditFile {
                path,
                operation,
                old_string,
                new_string,
                content,
                dest_path,
                host,
                sudo,
                ..
            } => json!({
                "path": path,
                "operation": operation,
                "old_string": old_string,
                "new_string": new_string,
                "content": content,
                "dest_path": dest_path,
                "host": host,
                "sudo": sudo,
            }),
            PendingCall::WriteRunbook { name, content, .. } => {
                json!({ "name": name, "content": content })
            }
            PendingCall::DeleteRunbook { name, .. }
            | PendingCall::ReadRunbook { name, .. }
            | PendingCall::ReadAgent { name, .. }
            | PendingCall::DeleteAgent { name, .. } => json!({ "name": name }),
            PendingCall::RunLogTail { run_id, limit, .. } => {
                json!({ "run_id": run_id, "limit": limit })
            }
            PendingCall::RunLogSearch {
                run_id,
                query,
                limit,
                ..
            } => json!({ "run_id": run_id, "query": query, "limit": limit }),
            PendingCall::GetTurn { run_id, turn, .. } => {
                json!({ "run_id": run_id, "turn": turn })
            }
            _ => unreachable!("all other variants handled by shell/knowledge helpers"),
        }
    }

    fn knowledge_arguments_value(&self) -> Option<Value> {
        let v = match self {
            PendingCall::AddMemory {
                key,
                value,
                category,
                ..
            } => json!({ "key": key, "value": value, "category": category }),
            PendingCall::UpdateMemory {
                key,
                category,
                body,
                append,
                tags,
                summary,
                relates_to,
                expires,
                ..
            } => json!({
                "key": key,
                "category": category,
                "body": body,
                "append": append,
                "tags": tags,
                "summary": summary,
                "relates_to": relates_to,
                "expires": expires,
            }),
            PendingCall::DeleteMemory { key, category, .. }
            | PendingCall::ReadMemory { key, category, .. } => {
                json!({ "key": key, "category": category })
            }
            PendingCall::ListMemories { category, .. } => json!({ "category": category }),
            PendingCall::SearchRepository { query, kind, .. } => {
                json!({ "query": query, "kind": kind })
            }
            PendingCall::RecallContext {
                query,
                turn_start,
                turn_end,
                scope,
                ..
            } => json!({
                "query": query,
                "turn_start": turn_start,
                "turn_end": turn_end,
                "scope": scope,
            }),
            PendingCall::SpawnGhost {
                runbook,
                message,
                agent,
                host,
                ..
            } => json!({
                "runbook": runbook,
                "message": message,
                "agent": agent,
                "host": host,
            }),
            PendingCall::CreateAgent {
                name,
                description,
                prompt,
                model,
                memory_namespace,
                max_turns,
                auto_approve_read_only,
                auto_approve_scripts,
                ..
            } => json!({
                "name": name,
                "description": description,
                "prompt": prompt,
                "model": model,
                "memory_namespace": memory_namespace,
                "max_turns": max_turns,
                "auto_approve_read_only": auto_approve_read_only,
                "auto_approve_scripts": auto_approve_scripts,
            }),
            PendingCall::AwaitAgentResult {
                job_id,
                agent_name,
                timeout_secs,
                ..
            } => json!({
                "job_id": job_id,
                "agent_name": agent_name,
                "timeout_secs": timeout_secs,
            }),
            _ => return None,
        };
        Some(v)
    }

    fn shell_arguments_value(&self) -> Option<Value> {
        let v = match self {
            PendingCall::Foreground {
                cmd,
                shell,
                host,
                timeout_secs,
                interactive,
                ..
            } => json!({
                "command": cmd,
                "background": false,
                "interactive": interactive,
                "shell": shell,
                "host": host,
                "timeout_secs": timeout_secs,
            }),
            PendingCall::Background { cmd, host, .. } => json!({
                "command": cmd,
                "background": true,
                "host": host,
            }),
            PendingCall::WatchShell {
                shell,
                timeout_secs,
                pattern,
                ..
            } => json!({
                "shell": shell,
                "timeout_secs": timeout_secs,
                "pattern": pattern,
            }),
            PendingCall::SendInput {
                shell,
                text,
                keys,
                paste,
                enter,
                ..
            } => {
                // Every field goes back in: a resumed or replayed history
                // that dropped `keys` or `paste` would re-run a different
                // call from the one the user approved.
                json!({
                    "shell": shell,
                    "text": text,
                    "keys": keys,
                    "paste": paste,
                    "enter": enter,
                })
            }
            PendingCall::ListShellLogs { since, limit, .. } => {
                json!({ "since": since, "limit": limit })
            }
            PendingCall::ReadShellLog {
                log,
                command_index,
                lines,
                filter,
                ..
            } => json!({
                "log": log,
                "command_index": command_index,
                "lines": lines,
                "filter": filter,
            }),
            PendingCall::GetShellContext { scope, .. } | PendingCall::ListShells { scope, .. } => {
                json!({ "scope": scope })
            }
            PendingCall::ReadShell {
                shell,
                lines,
                since_command,
                filter,
                ..
            } => json!({
                "shell": shell,
                "lines": lines,
                "since_command": since_command,
                "filter": filter,
            }),
            PendingCall::ReadShellScreen {
                shell,
                lines,
                scrollback,
                pattern,
                annotated,
                wait_for,
                settle_ms,
                timeout_secs,
                ..
            } => json!({
                "shell": shell,
                "lines": lines,
                "scrollback": scrollback,
                "pattern": pattern,
                "annotated": annotated,
                "wait_for": wait_for,
                "settle_ms": settle_ms,
                "timeout_secs": timeout_secs,
            }),
            PendingCall::FindInShells { pattern, scope, .. } => json!({
                "pattern": pattern,
                "scope": scope,
            }),
            PendingCall::OpenShell {
                host, rows, cols, ..
            } => json!({
                "host": host,
                "rows": rows,
                "cols": cols,
            }),
            PendingCall::CloseShell { shell, .. } => json!({ "shell": shell }),
            _ => return None,
        };
        Some(v)
    }
}
