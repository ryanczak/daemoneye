use super::args::*;
use super::schema::{ParamTy, ToolDef};
use crate::ai::tools::defs::TOOLS;
use crate::ai::types::AiEvent;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Tool event dispatcher (shared by all three provider backends)
// ---------------------------------------------------------------------------

/// Dispatch arm helper — deserialises `args` into `T` and constructs the event.
fn dispatch<T: ToolArgs>(id: &str, args: Value, ts: Option<String>) -> Option<AiEvent> {
    T::from_value(args).map(|a| a.to_event(id, ts))
}

/// Given a tool call ID, name, and parsed arguments, produce the corresponding
/// [`AiEvent`].  Returns `None` for unrecognised tool names.
pub fn dispatch_tool_event(
    id: &str,
    name: &str,
    args: &Value,
    ts: Option<String>,
) -> Option<AiEvent> {
    let args = args.clone();
    match name {
        "run_terminal_command" => dispatch::<RunTerminalCommandArgs>(id, args, ts),
        "load_tools" => dispatch::<LoadToolsArgs>(id, args, ts),
        "schedule_command" => dispatch::<ScheduleCommandArgs>(id, args, ts),
        "list_schedules" => Some(AiEvent::ListSchedules {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_scripts" => Some(AiEvent::ListScripts {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_runbooks" => Some(AiEvent::ListRunbooks {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "get_shell_context" => dispatch::<GetShellContextArgs>(id, args, ts),
        "list_shells" => dispatch::<ListShellsArgs>(id, args, ts),
        "cancel_schedule" => {
            schedule_id_event(args, ts.clone(), |jid, t| AiEvent::CancelSchedule {
                id: id.to_string(),
                job_id: jid,
                thought_signature: t,
            })
        }
        "delete_schedule" => schedule_id_event(args, ts, |jid, t| AiEvent::DeleteSchedule {
            id: id.to_string(),
            job_id: jid,
            thought_signature: t,
        }),
        "write_script" => dispatch::<WriteScriptArgs>(id, args, ts),
        "read_script" => dispatch::<ReadScriptArgs>(id, args, ts),
        "delete_script" => dispatch::<DeleteScriptArgs>(id, args, ts),
        "read_file" => dispatch::<ReadFileArgs>(id, args, ts),
        "read_shell" => dispatch::<ReadShellArgs>(id, args, ts),
        "read_shell_screen" => dispatch::<ReadShellScreenArgs>(id, args, ts),
        "find_in_shells" => dispatch::<FindInShellsArgs>(id, args, ts),
        "open_shell" => dispatch::<OpenShellArgs>(id, args, ts),
        "close_shell" => dispatch::<CloseShellArgs>(id, args, ts),
        "watch_shell" => dispatch::<WatchShellArgs>(id, args, ts),
        "send_input" => dispatch::<SendInputArgs>(id, args, ts),
        "list_shell_logs" => dispatch::<ListShellLogsArgs>(id, args, ts),
        "read_shell_log" => dispatch::<ReadShellLogArgs>(id, args, ts),
        "edit_file" => dispatch::<EditFileArgs>(id, args, ts),
        "write_runbook" => dispatch::<WriteRunbookArgs>(id, args, ts),
        "delete_runbook" => runbook_name_event(args, ts, |nm, t| AiEvent::DeleteRunbook {
            id: id.to_string(),
            name: nm,
            thought_signature: t,
        }),
        "read_runbook" => dispatch::<ReadRunbookArgs>(id, args, ts),
        "run_log_tail" => dispatch::<RunLogTailArgs>(id, args, ts),
        "run_log_search" => dispatch::<RunLogSearchArgs>(id, args, ts),
        "get_turn" => dispatch::<GetTurnArgs>(id, args, ts),
        "add_memory" => dispatch::<AddMemoryArgs>(id, args, ts),
        "update_memory" => dispatch::<UpdateMemoryArgs>(id, args, ts),
        "delete_memory" => dispatch::<DeleteMemoryArgs>(id, args, ts),
        "read_memory" => dispatch::<ReadMemoryArgs>(id, args, ts),
        "list_memories" => dispatch::<ListMemoriesArgs>(id, args, ts),
        "search_repository" => dispatch::<SearchRepositoryArgs>(id, args, ts),
        "recall_context" => dispatch::<RecallContextArgs>(id, args, ts),
        "spawn_ghost_shell" => dispatch::<SpawnGhostArgs>(id, args, ts),
        "create_agent" => dispatch::<CreateAgentArgs>(id, args, ts),
        "read_agent" => dispatch::<ReadAgentArgs>(id, args, ts),
        "list_agents" => Some(AiEvent::ListAgents {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "delete_agent" => runbook_name_event(args, ts, |nm, t| AiEvent::DeleteAgent {
            id: id.to_string(),
            name: nm,
            thought_signature: t,
        }),
        "await_agent_result" => dispatch::<AwaitAgentResultArgs>(id, args, ts),
        _ => None,
    }
}

/// A tool call to a **known** tool whose arguments do not match its schema:
/// list every mistake in `error` instead of letting the call drop as an
/// "unknown tool" notice (M27 phase-07d, F10). `None` for a name that is not
/// in `TOOLS` — those keep the unknown-tool path.
///
/// `ts` carries the provider's thought signature so the retry loop can keep
/// interleaved-thinking state consistent, exactly as `dispatch_tool_event`
/// does.
pub fn invalid_args_event(
    id: &str,
    name: &str,
    args: &Value,
    ts: Option<String>,
) -> Option<AiEvent> {
    let def = TOOLS.iter().find(|t| t.name == name)?;
    let mut problems = Vec::new();

    for param in def.params {
        match args.get(param.name) {
            None => {
                if param.required {
                    problems.push(format!(
                        "missing required parameter '{} ({}, required)'",
                        param.name,
                        param.ty.as_str()
                    ));
                }
            }
            Some(value) => {
                let actual = json_type_name(value);
                let expected = match param.ty {
                    ParamTy::Str => "string",
                    ParamTy::Bool => "boolean",
                    ParamTy::Int => "integer",
                };
                if actual != expected {
                    problems.push(format!(
                        "parameter '{} ({}, {})' is {actual}, expected {expected}",
                        param.name,
                        param.ty.as_str(),
                        if param.required {
                            "required"
                        } else {
                            "optional"
                        }
                    ));
                }
            }
        }
    }

    // Undeclared keys — `background` habit, chat prose leaking in — get
    // their own problem line, so the model sees *what* to remove rather
    // than a generic schema complaint it cannot act on.
    let declared: std::collections::BTreeSet<&str> = def.params.iter().map(|p| p.name).collect();
    let mut undeclared: Vec<&String> = Vec::new();
    if let Some(obj) = args.as_object() {
        for key in obj.keys() {
            if !declared.contains(key.as_str()) {
                undeclared.push(key);
            }
        }
    }
    undeclared.sort();
    for name in undeclared {
        let ty = json_type_name(&args[name]);
        problems.push(format!(
            "unexpected parameter '{name}' ({ty}): not in this tool's schema; remove it"
        ));
    }

    let error = if problems.is_empty() {
        format!(
            "the arguments do not match the schema; expected: {}",
            expected_params(def)
        )
    } else {
        format!(
            "{}; expected: {}",
            problems.join("; "),
            expected_params(def)
        )
    };

    Some(AiEvent::InvalidToolArgs {
        id: id.to_string(),
        name: def.name,
        arguments: args.to_string(),
        error,
        thought_signature: ts,
    })
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "float"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn expected_params(def: &ToolDef) -> String {
    def.params
        .iter()
        .map(|p| {
            format!(
                "{} ({}, {})",
                p.name,
                p.ty.as_str(),
                if p.required { "required" } else { "optional" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
