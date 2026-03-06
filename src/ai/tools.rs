use serde_json::{Value, json};
use crate::ai::types::AiEvent;

pub fn get_tool_definition() -> Value {
    json!([
        {
            "name": "run_terminal_command",
            "description": "Execute a bash command in one of two modes:\n\
             - background=true: Runs in a dedicated tmux window on the DAEMON HOST. Output is captured silently and returned to you. Use for read-only diagnostics (ls, ps, cat, grep, df, curl, etc.). If the user is SSH'd into a remote host, this still runs locally on the daemon machine. Supports sudo: the user will be prompted for their password in the chat interface.\n\
             - background=false (default): Injects the command into the USER'S TERMINAL PANE via tmux send-keys. The command is visible and interactive. Use for state-changing commands, service restarts, file edits, or anything that must run on the user's active host. If the user's pane is SSH'd to a remote machine, the command runs there. Supports sudo: the user types their password directly in the terminal pane.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The bash command to execute."},
                    "background": {"type": "boolean", "default": false, "description": "true = daemon host tmux window (captured output); false = user's terminal pane (visible, interactive, possibly remote). Defaults to false."},
                    "target_pane": {"type": "string", "description": "Optional: tmux pane ID (e.g. \"%3\") to target for foreground commands. Only specify when context shows multiple panes and the command must run in a specific one. Background commands always run on the daemon host — do not set target_pane for them."}
                },
                "required": ["command"]
            }
        },
        {
            "name": "schedule_command",
            "description": "Schedule a shell command (or named script) to run once at a specific UTC time or repeatedly on an interval. For watchdog monitoring, specify a runbook name to enable AI analysis of the output.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Human-readable name for this scheduled job."},
                    "command": {"type": "string", "description": "Shell command to run, or script name if is_script=true."},
                    "is_script": {"type": "boolean", "default": false, "description": "If true, 'command' is a script name in ~/.daemoneye/scripts/ to execute."},
                    "run_at": {"type": "string", "description": "ISO 8601 UTC datetime for a one-shot job, e.g. '2026-03-01T15:00:00Z'. Omit if using interval."},
                    "interval": {"type": "string", "description": "ISO 8601 duration for repeating jobs, e.g. PT5M (5 min), PT1H (1 hour), P1D (1 day). Omit if using run_at."},
                    "runbook": {"type": "string", "description": "Optional name of a runbook in ~/.daemoneye/runbooks/ for watchdog AI analysis of command output."}
                },
                "required": ["name", "command"]
            }
        },
        {
            "name": "list_schedules",
            "description": "Return the current list of scheduled jobs with their status, schedule, and next run time.",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "cancel_schedule",
            "description": "Cancel a scheduled job by its UUID. The job will no longer fire.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "UUID of the scheduled job to cancel."}
                },
                "required": ["id"]
            }
        },
        {
            "name": "write_script",
            "description": "Create or update a reusable script in ~/.daemoneye/scripts/. The user will be shown the full content and must approve before it is written. Scripts are saved with chmod 700.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "script_name": {"type": "string", "description": "Filename for the script (e.g. 'check-disk.sh')."},
                    "content": {"type": "string", "description": "Full content of the script, including the shebang line."}
                },
                "required": ["script_name", "content"]
            }
        },
        {
            "name": "list_scripts",
            "description": "Return the list of scripts in ~/.daemoneye/scripts/ with their sizes.",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "read_script",
            "description": "Read the content of a script from ~/.daemoneye/scripts/.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "script_name": {"type": "string", "description": "Name of the script to read."}
                },
                "required": ["script_name"]
            }
        },
        {
            "name": "watch_pane",
            "description": "Passively monitor a background tmux pane for output changes. The tool returns immediately, and an out-of-band [System] Activity detected message will be injected into this chat session when the pane produces new output. Use this instead of polling to be notified when a long-running process (e.g. build, test, log tail) finishes or produces new output.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pane_id": {"type": "string", "description": "Tmux pane ID to monitor (e.g. \"%3\"). Get IDs from [BACKGROUND PANE] context blocks."},
                    "timeout_secs": {"type": "integer", "description": "Maximum seconds to wait for output. Defaults to 300 (5 minutes)."}
                },
                "required": ["pane_id"]
            }
        },
        {
            "name": "write_runbook",
            "description": "Create or update a runbook in ~/.daemoneye/runbooks/. Must include '# Runbook:' heading and '## Alert Criteria' section. Optionally starts with YAML frontmatter (---) containing 'tags: [...]' and 'memories: [...]'. User approval required.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Filename key for the runbook (no extension, e.g. 'disk-check')."},
                    "content": {"type": "string", "description": "Full markdown content of the runbook, including optional YAML frontmatter."}
                },
                "required": ["name", "content"]
            }
        },
        {
            "name": "delete_runbook",
            "description": "Delete a runbook from ~/.daemoneye/runbooks/. User approval required. Will warn if active scheduled jobs reference this runbook.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name of the runbook to delete (no extension)."}
                },
                "required": ["name"]
            }
        },
        {
            "name": "read_runbook",
            "description": "Read the full content of a named runbook from ~/.daemoneye/runbooks/.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name of the runbook to read (no extension)."}
                },
                "required": ["name"]
            }
        },
        {
            "name": "list_runbooks",
            "description": "List all runbooks in ~/.daemoneye/runbooks/ with their tags.",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "add_memory",
            "description": "Store a persistent memory entry in ~/.daemoneye/memory/<category>/<key>.md. category: 'session' (loaded at every session start — keep brief), 'knowledge' (loaded on-demand via runbook references or read_memory), 'incident' (historical, searchable only).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "Unique key for this memory entry (no path separators)."},
                    "value": {"type": "string", "description": "Markdown content to store."},
                    "category": {"type": "string", "description": "'session', 'knowledge', or 'incident'."}
                },
                "required": ["key", "value", "category"]
            }
        },
        {
            "name": "delete_memory",
            "description": "Remove a memory entry from ~/.daemoneye/memory/<category>/<key>.md.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "Key of the memory entry to delete."},
                    "category": {"type": "string", "description": "'session', 'knowledge', or 'incident'."}
                },
                "required": ["key", "category"]
            }
        },
        {
            "name": "read_memory",
            "description": "Read a specific memory entry by key and category.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "Key of the memory entry to read."},
                    "category": {"type": "string", "description": "'session', 'knowledge', or 'incident'."}
                },
                "required": ["key", "category"]
            }
        },
        {
            "name": "list_memories",
            "description": "List all memory keys, optionally filtered by category.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "category": {"type": "string", "description": "Optional: 'session', 'knowledge', or 'incident'. Omit to list all."}
                }
            }
        },
        {
            "name": "search_repository",
            "description": "Search across runbooks, scripts, memory, or the event log for a keyword. kind: 'runbooks' | 'scripts' | 'memory' | 'events' | 'all'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search term (case-insensitive)."},
                    "kind": {"type": "string", "description": "'runbooks', 'scripts', 'memory', 'events', or 'all'."}
                },
                "required": ["query", "kind"]
            }
        }
    ])
}

pub fn get_openai_tool_definition() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "run_terminal_command",
                "description": "Execute a bash command in one of two modes:\n\
                 - background=true: Runs in a dedicated tmux window on the DAEMON HOST. Output captured silently. Use for read-only diagnostics. Supports sudo via chat interface.\n\
                 - background=false (default): Injects the command into the USER'S TERMINAL PANE via tmux. Visible and interactive. Use for state-changing commands. Sudo requires the user to type password in the pane.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "The bash command to execute."},
                        "background": {"type": "boolean", "default": false, "description": "true = daemon host tmux window (captured); false = user's terminal pane (visible, interactive). Defaults to false."},
                        "target_pane": {"type": "string", "description": "Optional: tmux pane ID (e.g. \"%3\") to target for foreground commands."}
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "schedule_command",
                "description": "Schedule a command or script to run once or on a repeating interval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "command": {"type": "string"},
                        "is_script": {"type": "boolean", "default": false},
                        "run_at": {"type": "string"},
                        "interval": {"type": "string"},
                        "runbook": {"type": "string"}
                    },
                    "required": ["name", "command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_schedules",
                "description": "Return the current list of scheduled jobs.",
                "parameters": {"type": "object", "properties": {}}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "cancel_schedule",
                "description": "Cancel a scheduled job by UUID.",
                "parameters": {
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_script",
                "description": "Create or update a reusable script in ~/.daemoneye/scripts/ (requires user approval).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "script_name": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["script_name", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_scripts",
                "description": "Return the list of scripts in ~/.daemoneye/scripts/.",
                "parameters": {"type": "object", "properties": {}}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_script",
                "description": "Read the content of a named script.",
                "parameters": {
                    "type": "object",
                    "properties": {"script_name": {"type": "string"}},
                    "required": ["script_name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "watch_pane",
                "description": "Passively monitor a background tmux pane for output changes. The monitoring runs asynchronously and notifies you out-of-band via a [System] chat message when activity occurs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pane_id": {"type": "string", "description": "Tmux pane ID (e.g. \"%3\") from [BACKGROUND PANE] context blocks."},
                        "timeout_secs": {"type": "integer", "description": "Max seconds to wait. Defaults to 300."}
                    },
                    "required": ["pane_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_runbook",
                "description": "Create or update a runbook (requires user approval).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["name", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "delete_runbook",
                "description": "Delete a runbook (requires user approval).",
                "parameters": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_runbook",
                "description": "Read the content of a named runbook.",
                "parameters": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_runbooks",
                "description": "List all runbooks with their tags.",
                "parameters": {"type": "object", "properties": {}}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "add_memory",
                "description": "Store a persistent memory entry.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"},
                        "value": {"type": "string"},
                        "category": {"type": "string"}
                    },
                    "required": ["key", "value", "category"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "delete_memory",
                "description": "Remove a memory entry.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"},
                        "category": {"type": "string"}
                    },
                    "required": ["key", "category"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_memory",
                "description": "Read a specific memory entry by key and category.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"},
                        "category": {"type": "string"}
                    },
                    "required": ["key", "category"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_memories",
                "description": "List all memory keys, optionally filtered by category.",
                "parameters": {
                    "type": "object",
                    "properties": {"category": {"type": "string"}}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_repository",
                "description": "Search across runbooks, scripts, memory, or events.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "kind": {"type": "string"}
                    },
                    "required": ["query", "kind"]
                }
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Tool event dispatcher (shared by all three provider backends)
// ---------------------------------------------------------------------------

/// Given a tool call ID, name, and parsed arguments, produce the corresponding
/// [`AiEvent`].  Returns `None` for unrecognised tool names.
pub fn dispatch_tool_event(id: &str, name: &str, args: &Value, ts: Option<String>) -> Option<AiEvent> {
    match name {
        "run_terminal_command" => {
            let cmd = args["command"].as_str()?;
            let bg = args["background"].as_bool().unwrap_or(false);
            let target = args["target_pane"].as_str().map(|s| s.to_string());
            Some(AiEvent::ToolCall(id.to_string(), cmd.to_string(), bg, target, ts))
        }
        "schedule_command" => Some(AiEvent::ScheduleCommand {
            id: id.to_string(),
            name: args["name"].as_str().unwrap_or("unnamed").to_string(),
            command: args["command"].as_str().unwrap_or("").to_string(),
            is_script: args["is_script"].as_bool().unwrap_or(false),
            run_at: args["run_at"].as_str().map(|s| s.to_string()),
            interval: args["interval"].as_str().map(|s| s.to_string()),
            runbook: args["runbook"].as_str().map(|s| s.to_string()),
            thought_signature: ts,
        }),
        "list_schedules" => Some(AiEvent::ListSchedules { id: id.to_string(), thought_signature: ts }),
        "cancel_schedule" => Some(AiEvent::CancelSchedule {
            id: id.to_string(),
            job_id: args["id"].as_str().unwrap_or("").to_string(),
            thought_signature: ts.clone(),
        }),
        "delete_schedule" => Some(AiEvent::DeleteSchedule {
            id: id.to_string(),
            job_id: args["id"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "write_script" => Some(AiEvent::WriteScript {
            id: id.to_string(),
            script_name: args["script_name"].as_str().unwrap_or("").to_string(),
            content: args["content"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "list_scripts" => Some(AiEvent::ListScripts { id: id.to_string(), thought_signature: ts }),
        "read_script" => Some(AiEvent::ReadScript {
            id: id.to_string(),
            script_name: args["script_name"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "watch_pane" => Some(AiEvent::WatchPane {
            id: id.to_string(),
            pane_id: args["pane_id"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "write_runbook" => Some(AiEvent::WriteRunbook {
            id: id.to_string(),
            name: args["name"].as_str().unwrap_or("").to_string(),
            content: args["content"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "delete_runbook" => Some(AiEvent::DeleteRunbook {
            id: id.to_string(),
            name: args["name"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "read_runbook" => Some(AiEvent::ReadRunbook {
            id: id.to_string(),
            name: args["name"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "list_runbooks" => Some(AiEvent::ListRunbooks {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "add_memory" => Some(AiEvent::AddMemory {
            id: id.to_string(),
            key: args["key"].as_str().unwrap_or("").to_string(),
            value: args["value"].as_str().unwrap_or("").to_string(),
            category: args["category"].as_str().unwrap_or("knowledge").to_string(),
            thought_signature: ts,
        }),
        "delete_memory" => Some(AiEvent::DeleteMemory {
            id: id.to_string(),
            key: args["key"].as_str().unwrap_or("").to_string(),
            category: args["category"].as_str().unwrap_or("knowledge").to_string(),
            thought_signature: ts,
        }),
        "read_memory" => Some(AiEvent::ReadMemory {
            id: id.to_string(),
            key: args["key"].as_str().unwrap_or("").to_string(),
            category: args["category"].as_str().unwrap_or("knowledge").to_string(),
            thought_signature: ts,
        }),
        "list_memories" => Some(AiEvent::ListMemories {
            id: id.to_string(),
            category: args["category"].as_str().map(|s| s.to_string()),
            thought_signature: ts,
        }),
        "search_repository" => Some(AiEvent::SearchRepository {
            id: id.to_string(),
            query: args["query"].as_str().unwrap_or("").to_string(),
            kind: args["kind"].as_str().unwrap_or("all").to_string(),
            thought_signature: ts,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

