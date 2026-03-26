use crate::ai::types::AiEvent;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Unified tool schema
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum ParamTy {
    Str,
    Bool,
    Int,
}

impl ParamTy {
    fn as_str(self) -> &'static str {
        match self {
            ParamTy::Str => "string",
            ParamTy::Bool => "boolean",
            ParamTy::Int => "integer",
        }
    }

    fn as_gemini_str(self) -> &'static str {
        match self {
            ParamTy::Str => "STRING",
            ParamTy::Bool => "BOOLEAN",
            ParamTy::Int => "INTEGER",
        }
    }
}

pub struct ParamDef {
    pub name: &'static str,
    pub ty: ParamTy,
    pub description: &'static str,
    pub required: bool,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamDef],
}

pub static TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "run_terminal_command",
        description: "Execute a bash command in one of two modes:\n\
             - background=true: Runs in a dedicated tmux window on the DAEMON HOST. Output is \
             captured silently and returned to you. Use for read-only diagnostics (ls, ps, cat, \
             grep, df, curl, etc.). If the user is SSH'd into a remote host, this still runs \
             locally on the daemon machine. Supports sudo: the user will be prompted for their \
             password in the chat interface.\n\
             - background=false (default): Injects the command into the USER'S TERMINAL PANE via \
             tmux send-keys. The command is visible and interactive. Use for state-changing \
             commands, service restarts, file edits, or anything that must run on the user's \
             active host. If the user's pane is SSH'd to a remote machine, the command runs \
             there. Supports sudo: the user types their password directly in the terminal pane.",
        params: &[
            ParamDef {
                name: "command",
                ty: ParamTy::Str,
                required: true,
                description: "The bash command to execute.",
            },
            ParamDef {
                name: "background",
                ty: ParamTy::Bool,
                required: false,
                description: "true = daemon host tmux window (captured output); false = user's \
                              terminal pane (visible, interactive, possibly remote). Defaults to false.",
            },
            ParamDef {
                name: "target_pane",
                ty: ParamTy::Str,
                required: false,
                description: "Optional: tmux pane ID (e.g. \"%3\") to target for foreground \
                              commands. Get IDs from [VISIBLE PANE], [BACKGROUND PANE], or \
                              [SESSION PANE] context blocks, or call list_panes to discover them. \
                              Background commands always run on the daemon host — do not set \
                              target_pane for them.",
            },
            ParamDef {
                name: "retry_in_pane",
                ty: ParamTy::Str,
                required: false,
                description: "Optional: pane ID of a previous background window (from a \
                              [Background Task Completed] message) to reuse for a retry. \
                              Only valid with background=true. The command runs in the same \
                              tmux window, keeping the failure output visible in scrollback \
                              above the new run. Omit to create a fresh background window.",
            },
        ],
    },
    ToolDef {
        name: "schedule_command",
        description: "Schedule a task to run once at a specific UTC time or repeatedly on an \
                      interval. Two modes: (1) Script mode — set command to a script name and \
                      is_script=true to run a pre-vetted script from ~/.daemoneye/scripts/; \
                      optionally pair with runbook for watchdog AI analysis of output. \
                      (2) Ghost mode — set ghost_runbook to a runbook name to spawn an \
                      autonomous Ghost Shell session instead of running a command; the runbook \
                      governs what the ghost may do. ghost_runbook is mutually exclusive with \
                      command/is_script.",
        params: &[
            ParamDef {
                name: "name",
                ty: ParamTy::Str,
                required: true,
                description: "Human-readable name for this scheduled job.",
            },
            ParamDef {
                name: "command",
                ty: ParamTy::Str,
                required: false,
                description: "Script name (when is_script=true) to execute. Omit when using ghost_runbook.",
            },
            ParamDef {
                name: "is_script",
                ty: ParamTy::Bool,
                required: false,
                description: "If true, 'command' is a script name in ~/.daemoneye/scripts/ to execute.",
            },
            ParamDef {
                name: "run_at",
                ty: ParamTy::Str,
                required: false,
                description: "ISO 8601 UTC datetime for a one-shot job, e.g. '2026-03-01T15:00:00Z'. Omit if using interval.",
            },
            ParamDef {
                name: "interval",
                ty: ParamTy::Str,
                required: false,
                description: "ISO 8601 duration for repeating jobs, e.g. PT30S (30 sec), PT1M (1 min), PT5M (5 min), PT1H (1 hour), P1D (1 day). Must be ISO 8601 — never a bare number or plain English string. Omit if using run_at.",
            },
            ParamDef {
                name: "runbook",
                ty: ParamTy::Str,
                required: false,
                description: "Watchdog runbook: name of a runbook for AI analysis of script \
                              output after the script finishes (script mode only). NOT for ghost \
                              jobs — use ghost_runbook for that.",
            },
            ParamDef {
                name: "ghost_runbook",
                ty: ParamTy::Str,
                required: false,
                description: "Ghost mode: name of a runbook that governs an autonomous Ghost \
                              Shell session. When set, the job spawns a Ghost Shell instead of \
                              running a command — do NOT also set command/is_script/runbook. \
                              The runbook frontmatter controls ghost policy (approved scripts, \
                              sudo, SSH target, turn budget). Mutually exclusive with \
                              command/is_script.",
            },
            ParamDef {
                name: "cron",
                ty: ParamTy::Str,
                required: false,
                description: "5-field cron expression for recurring jobs (e.g. '*/5 * * * *' for \
                              every 5 minutes, '0 9 * * 1-5' for weekdays at 09:00 UTC). \
                              Mutually exclusive with interval and run_at.",
            },
        ],
    },
    ToolDef {
        name: "list_schedules",
        description: "Return the current list of scheduled jobs with their status, schedule, and next run time.",
        params: &[],
    },
    ToolDef {
        name: "cancel_schedule",
        description: "Cancel a scheduled job by its UUID. The job will no longer fire but \
                      remains visible in list_schedules with status 'cancelled'.",
        params: &[ParamDef {
            name: "id",
            ty: ParamTy::Str,
            required: true,
            description: "UUID of the scheduled job to cancel.",
        }],
    },
    ToolDef {
        name: "delete_schedule",
        description: "Permanently delete a scheduled job by its UUID, removing it from \
                      the schedule store entirely. Unlike cancel_schedule, the job will \
                      no longer appear in list_schedules.",
        params: &[ParamDef {
            name: "id",
            ty: ParamTy::Str,
            required: true,
            description: "UUID of the scheduled job to delete.",
        }],
    },
    ToolDef {
        name: "write_script",
        description: "Create or update a reusable script in ~/.daemoneye/scripts/. The user will \
                      be shown the full content and must approve before it is written. Scripts are \
                      saved with chmod 700.",
        params: &[
            ParamDef {
                name: "script_name",
                ty: ParamTy::Str,
                required: true,
                description: "Filename for the script (e.g. 'check-disk.sh').",
            },
            ParamDef {
                name: "content",
                ty: ParamTy::Str,
                required: true,
                description: "Full content of the script, including the shebang line.",
            },
        ],
    },
    ToolDef {
        name: "list_scripts",
        description: "Return the list of scripts in ~/.daemoneye/scripts/ with their sizes.",
        params: &[],
    },
    ToolDef {
        name: "read_script",
        description: "Read the content of a script from ~/.daemoneye/scripts/.",
        params: &[ParamDef {
            name: "script_name",
            ty: ParamTy::Str,
            required: true,
            description: "Name of the script to read.",
        }],
    },
    ToolDef {
        name: "delete_script",
        description: "Permanently delete a script from ~/.daemoneye/scripts/. The user must \
                      approve before the file is removed. Also removes any sidecar .meta.toml.",
        params: &[ParamDef {
            name: "script_name",
            ty: ParamTy::Str,
            required: true,
            description: "Name of the script to delete.",
        }],
    },
    ToolDef {
        name: "watch_pane",
        description: "Passively monitor a background tmux pane. Blocks until the pane's \
                      command completes (returns to shell prompt), or until a specific string \
                      or regex pattern appears in the pane output (if `pattern` is set), or \
                      until `timeout_secs` elapses. Use for build completion, service startup \
                      events, or any output-triggered condition.",
        params: &[
            ParamDef {
                name: "pane_id",
                ty: ParamTy::Str,
                required: true,
                description: "Tmux pane ID to monitor (e.g. \"%3\"). Get IDs from context blocks ([VISIBLE PANE], [BACKGROUND PANE], [SESSION PANE]), background=true tool results, or list_panes.",
            },
            ParamDef {
                name: "timeout_secs",
                ty: ParamTy::Int,
                required: false,
                description: "Maximum seconds to wait. Defaults to 300 (5 minutes).",
            },
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex pattern. When set, returns as soon as the \
                                     pattern matches any line in the pane output — does not wait \
                                     for the command to exit. Example: 'listening on port \\d+' \
                                     or 'build (succeeded|failed)'.",
            },
        ],
    },
    ToolDef {
        name: "read_file",
        description: "Read a file with line-range pagination and optional grep filtering. \
                      Sensitive data is masked. \
                      Without target_pane: reads directly from the DAEMON HOST filesystem. \
                      With target_pane: runs sed/grep in that pane — use this when the file \
                      is on a remote SSH host the user is connected to.",
        params: &[
            ParamDef {
                name: "path",
                ty: ParamTy::Str,
                required: true,
                description: "Absolute path to the file to read.",
            },
            ParamDef {
                name: "offset",
                ty: ParamTy::Int,
                required: false,
                description: "Line number to start reading from (1-based). Omit to read from the beginning.",
            },
            ParamDef {
                name: "limit",
                ty: ParamTy::Int,
                required: false,
                description: "Maximum number of lines to return. Defaults to 200, capped at 500.",
            },
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex pattern. When set, only lines matching the \
                                     pattern are returned (like grep). Applied after offset/limit.",
            },
            ParamDef {
                name: "target_pane",
                ty: ParamTy::Str,
                required: false,
                description: "Optional tmux pane ID. When set, the read runs inside that \
                                     pane (useful for files on a remote SSH host). Omit for \
                                     daemon-host files.",
            },
        ],
    },
    ToolDef {
        name: "edit_file",
        description: "Safely replace an exact string in a file. Finds `old_string` (must appear \
                      exactly once), replaces with `new_string`, writes atomically. \
                      User approval required before the write is committed. \
                      Without target_pane: edits on the DAEMON HOST filesystem. \
                      With target_pane: runs a Python3/Perl replacement script in that pane — \
                      use this for files on a remote SSH host.",
        params: &[
            ParamDef {
                name: "path",
                ty: ParamTy::Str,
                required: true,
                description: "Absolute path to the file to edit.",
            },
            ParamDef {
                name: "old_string",
                ty: ParamTy::Str,
                required: true,
                description: "Exact text to find in the file. Must appear exactly once. \
                                     Include enough surrounding context (e.g. the whole line) to \
                                     be unique.",
            },
            ParamDef {
                name: "new_string",
                ty: ParamTy::Str,
                required: true,
                description: "Replacement text. Use empty string to delete old_string.",
            },
            ParamDef {
                name: "target_pane",
                ty: ParamTy::Str,
                required: false,
                description: "Optional tmux pane ID. When set, the edit runs inside that \
                                     pane via Python3 (Perl fallback) — use this for files on a \
                                     remote SSH host. Omit for daemon-host files.",
            },
        ],
    },
    ToolDef {
        name: "write_runbook",
        description: "Create or update a runbook in ~/.daemoneye/runbooks/. Must include \
                      '# Runbook:' heading and '## Alert Criteria' section. Optionally starts \
                      with YAML frontmatter (---) containing 'tags: [...]' and 'memories: [...]'. \
                      User approval required.",
        params: &[
            ParamDef {
                name: "name",
                ty: ParamTy::Str,
                required: true,
                description: "Filename key for the runbook (no extension, e.g. 'disk-check').",
            },
            ParamDef {
                name: "content",
                ty: ParamTy::Str,
                required: true,
                description: "Full markdown content of the runbook, including optional YAML frontmatter.",
            },
        ],
    },
    ToolDef {
        name: "delete_runbook",
        description: "Delete a runbook from ~/.daemoneye/runbooks/. User approval required. \
                      Will warn if active scheduled jobs reference this runbook.",
        params: &[ParamDef {
            name: "name",
            ty: ParamTy::Str,
            required: true,
            description: "Name of the runbook to delete (no extension).",
        }],
    },
    ToolDef {
        name: "read_runbook",
        description: "Read the full content of a named runbook from ~/.daemoneye/runbooks/.",
        params: &[ParamDef {
            name: "name",
            ty: ParamTy::Str,
            required: true,
            description: "Name of the runbook to read (no extension).",
        }],
    },
    ToolDef {
        name: "list_runbooks",
        description: "List all runbooks in ~/.daemoneye/runbooks/ with their tags.",
        params: &[],
    },
    ToolDef {
        name: "add_memory",
        description: "Store a persistent memory entry in ~/.daemoneye/memory/<category>/<key>.md. \
                      category: 'session' (loaded at every session start — keep brief), \
                      'knowledge' (loaded on-demand via runbook references or read_memory), \
                      'incident' (historical, searchable only).",
        params: &[
            ParamDef {
                name: "key",
                ty: ParamTy::Str,
                required: true,
                description: "Unique key for this memory entry (no path separators).",
            },
            ParamDef {
                name: "value",
                ty: ParamTy::Str,
                required: true,
                description: "Markdown content to store.",
            },
            ParamDef {
                name: "category",
                ty: ParamTy::Str,
                required: true,
                description: "'session', 'knowledge', or 'incident'.",
            },
        ],
    },
    ToolDef {
        name: "delete_memory",
        description: "Remove a memory entry from ~/.daemoneye/memory/<category>/<key>.md.",
        params: &[
            ParamDef {
                name: "key",
                ty: ParamTy::Str,
                required: true,
                description: "Key of the memory entry to delete.",
            },
            ParamDef {
                name: "category",
                ty: ParamTy::Str,
                required: true,
                description: "'session', 'knowledge', or 'incident'.",
            },
        ],
    },
    ToolDef {
        name: "read_memory",
        description: "Read a specific memory entry by key and category.",
        params: &[
            ParamDef {
                name: "key",
                ty: ParamTy::Str,
                required: true,
                description: "Key of the memory entry to read.",
            },
            ParamDef {
                name: "category",
                ty: ParamTy::Str,
                required: true,
                description: "'session', 'knowledge', or 'incident'.",
            },
        ],
    },
    ToolDef {
        name: "list_memories",
        description: "List all memory keys, optionally filtered by category.",
        params: &[ParamDef {
            name: "category",
            ty: ParamTy::Str,
            required: false,
            description: "Optional: 'session', 'knowledge', or 'incident'. Omit to list all.",
        }],
    },
    ToolDef {
        name: "search_repository",
        description: "Search across runbooks, scripts, memory, or the event log for a keyword. \
                      kind: 'runbooks' | 'scripts' | 'memory' | 'events' | 'all'.",
        params: &[
            ParamDef {
                name: "query",
                ty: ParamTy::Str,
                required: true,
                description: "Search term (case-insensitive).",
            },
            ParamDef {
                name: "kind",
                ty: ParamTy::Str,
                required: true,
                description: "'runbooks', 'scripts', 'memory', 'events', or 'all'.",
            },
        ],
    },
    ToolDef {
        name: "get_terminal_context",
        description: "Capture a fresh snapshot of the current tmux session: active pane contents, \
                      background panes, session topology, and environment variables. \
                      Call this when you need to see what is on the user's screen, check live \
                      command output, or understand the current terminal state. \
                      The terminal snapshot is NOT automatically included in every message — \
                      call this tool to get it on demand.",
        params: &[],
    },
    ToolDef {
        name: "list_panes",
        description: "List all active panes in the current tmux session with their pane ID, \
                      window name, foreground command, working directory, and terminal title. \
                      Use this to discover which panes exist — especially to find panes running \
                      SSH sessions, editors, REPLs, or other processes that can be targeted with \
                      run_terminal_command. After identifying the right pane ID, pass it as the \
                      target_pane argument to run_terminal_command to execute a command there. \
                      This tool reads from an in-memory cache (refreshed every 2 s) and returns \
                      immediately with no tmux subprocess overhead.",
        params: &[],
    },
    ToolDef {
        name: "close_background_window",
        description: "Close a background tmux window that is no longer needed. \
                      Call this after you have finished with a background window — \
                      once you have read its output and will not be issuing any more \
                      commands there. Frees the slot immediately rather than waiting \
                      for the cap eviction. Up to 5 background windows exist per session; \
                      closing idle ones proactively prevents cap exhaustion.",
        params: &[ParamDef {
            name: "pane_id",
            ty: ParamTy::Str,
            required: true,
            description: "Pane ID of the background window to close (e.g. \"%3\"). \
                              Obtained from a [Background Task Completed] message or \
                              a [BACKGROUND PANE] context block.",
        }],
    },
    ToolDef {
        name: "spawn_ghost_shell",
        description: "Spawn an autonomous Ghost Shell session that runs in the background \
                      without requiring your attention. The ghost follows the named runbook \
                      autonomously — running pre-approved scripts, reading logs, taking \
                      corrective actions — and injects lifecycle events into the session \
                      history when it starts, completes, or fails. Use this when you want \
                      to delegate an investigation or remediation task while continuing to \
                      assist the user. The ghost's policy (approved scripts, sudo access, \
                      SSH target, turn budget) is governed entirely by the runbook frontmatter. \
                      Returns the ghost session ID.",
        params: &[
            ParamDef {
                name: "runbook",
                ty: ParamTy::Str,
                required: true,
                description: "Name of the runbook in ~/.daemoneye/runbooks/ that governs \
                              the ghost shell's behaviour and policy.",
            },
            ParamDef {
                name: "message",
                ty: ParamTy::Str,
                required: true,
                description: "Human-readable description of the problem or task to hand off \
                              to the ghost. This becomes the ghost's initial user turn.",
            },
        ],
    },
];

// ---------------------------------------------------------------------------
// Provider renderers
// ---------------------------------------------------------------------------

fn build_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            (
                p.name.to_string(),
                json!({
                    "type": p.ty.as_str(),
                    "description": p.description,
                }),
            )
        })
        .collect()
}

fn build_gemini_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            (
                p.name.to_string(),
                json!({
                    "type": p.ty.as_gemini_str(),
                    "description": p.description,
                }),
            )
        })
        .collect()
}

fn required_names(params: &[ParamDef]) -> Vec<&'static str> {
    params
        .iter()
        .filter(|p| p.required)
        .map(|p| p.name)
        .collect()
}

fn render_anthropic(tools: &[ToolDef]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let props = build_properties(t.params);
                let req = required_names(t.params);
                let mut schema = json!({ "type": "object", "properties": props });
                if !req.is_empty() {
                    schema["required"] = json!(req);
                }
                json!({ "name": t.name, "description": t.description, "input_schema": schema })
            })
            .collect(),
    )
}

fn render_openai(tools: &[ToolDef]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let props = build_properties(t.params);
                let req = required_names(t.params);
                let mut params = json!({ "type": "object", "properties": props });
                if !req.is_empty() {
                    params["required"] = json!(req);
                }
                json!({ "type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": params
                }})
            })
            .collect(),
    )
}

pub fn render_gemini(tools: &[ToolDef]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let props = build_gemini_properties(t.params);
                let req = required_names(t.params);
                let mut params = json!({ "type": "OBJECT", "properties": props });
                if !req.is_empty() {
                    params["required"] = json!(req);
                }
                json!({ "name": t.name, "description": t.description, "parameters": params })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Public API (unchanged callers)
// ---------------------------------------------------------------------------

pub fn get_tool_definition() -> Value {
    render_anthropic(TOOLS)
}

pub fn get_openai_tool_definition() -> Value {
    render_openai(TOOLS)
}

pub fn get_gemini_tool_definition() -> Value {
    render_gemini(TOOLS)
}

// ---------------------------------------------------------------------------
// Tool event dispatcher (shared by all three provider backends)
// ---------------------------------------------------------------------------

/// Given a tool call ID, name, and parsed arguments, produce the corresponding
/// [`AiEvent`].  Returns `None` for unrecognised tool names.
pub fn dispatch_tool_event(
    id: &str,
    name: &str,
    args: &Value,
    ts: Option<String>,
) -> Option<AiEvent> {
    match name {
        "run_terminal_command" => {
            let cmd = args["command"].as_str()?;
            let bg = args["background"].as_bool().unwrap_or(false);
            let target = args["target_pane"].as_str().map(|s| s.to_string());
            let retry = args["retry_in_pane"].as_str().map(|s| s.to_string());
            Some(AiEvent::ToolCall(
                id.to_string(),
                cmd.to_string(),
                bg,
                target,
                retry,
                ts,
            ))
        }
        "schedule_command" => Some(AiEvent::ScheduleCommand {
            id: id.to_string(),
            name: args["name"].as_str().unwrap_or("unnamed").to_string(),
            command: args["command"].as_str().unwrap_or("").to_string(),
            is_script: args["is_script"].as_bool().unwrap_or(false),
            run_at: args["run_at"].as_str().map(|s| s.to_string()),
            interval: args["interval"].as_str().map(|s| s.to_string()),
            runbook: args["runbook"].as_str().map(|s| s.to_string()),
            ghost_runbook: args["ghost_runbook"].as_str().map(|s| s.to_string()),
            cron: args["cron"].as_str().map(|s| s.to_string()),
            thought_signature: ts,
        }),
        "list_schedules" => Some(AiEvent::ListSchedules {
            id: id.to_string(),
            thought_signature: ts,
        }),
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
        "list_scripts" => Some(AiEvent::ListScripts {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "read_script" => Some(AiEvent::ReadScript {
            id: id.to_string(),
            script_name: args["script_name"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "delete_script" => Some(AiEvent::DeleteScript {
            id: id.to_string(),
            script_name: args["script_name"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "watch_pane" => Some(AiEvent::WatchPane {
            id: id.to_string(),
            pane_id: args["pane_id"].as_str().unwrap_or("").to_string(),
            timeout_secs: args["timeout_secs"].as_u64().unwrap_or(300),
            pattern: args["pattern"].as_str().map(|s| s.to_string()),
            thought_signature: ts,
        }),
        "read_file" => Some(AiEvent::ReadFile {
            id: id.to_string(),
            path: args["path"].as_str().unwrap_or("").to_string(),
            offset: args["offset"].as_u64(),
            limit: args["limit"].as_u64(),
            pattern: args["pattern"].as_str().map(|s| s.to_string()),
            target_pane: args["target_pane"].as_str().map(|s| s.to_string()),
            thought_signature: ts,
        }),
        "edit_file" => Some(AiEvent::EditFile {
            id: id.to_string(),
            path: args["path"].as_str().unwrap_or("").to_string(),
            old_string: args["old_string"].as_str().unwrap_or("").to_string(),
            new_string: args["new_string"].as_str().unwrap_or("").to_string(),
            target_pane: args["target_pane"].as_str().map(|s| s.to_string()),
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
        "get_terminal_context" => Some(AiEvent::GetTerminalContext {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_panes" => Some(AiEvent::ListPanes {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "close_background_window" => Some(AiEvent::CloseBackgroundWindow {
            id: id.to_string(),
            pane_id: args["pane_id"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        "spawn_ghost_shell" => Some(AiEvent::SpawnGhost {
            id: id.to_string(),
            runbook: args["runbook"].as_str().unwrap_or("").to_string(),
            message: args["message"].as_str().unwrap_or("").to_string(),
            thought_signature: ts,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool in TOOLS must appear in the Gemini render, in order.
    /// This is the regression test that would have caught every previous
    /// "tool missing from Gemini" bug.
    #[test]
    fn render_gemini_names_match_tools_slice() {
        let rendered = render_gemini(TOOLS);
        let arr = rendered
            .as_array()
            .expect("render_gemini must return an array");
        assert_eq!(
            arr.len(),
            TOOLS.len(),
            "rendered Gemini tool count ({}) != TOOLS slice length ({})",
            arr.len(),
            TOOLS.len()
        );
        for (i, (entry, def)) in arr.iter().zip(TOOLS.iter()).enumerate() {
            assert_eq!(
                entry["name"].as_str().unwrap(),
                def.name,
                "tool at index {} name mismatch",
                i
            );
        }
    }

    /// Parameter types must use Gemini's uppercase strings, not the lowercase
    /// variants used by Anthropic/OpenAI.
    #[test]
    fn render_gemini_types_are_uppercase() {
        let rendered = render_gemini(TOOLS);
        let arr = rendered.as_array().unwrap();
        let rtc = arr
            .iter()
            .find(|e| e["name"] == "run_terminal_command")
            .expect("run_terminal_command must be present");
        let props = &rtc["parameters"]["properties"];
        assert_eq!(props["command"]["type"], "STRING");
        assert_eq!(props["background"]["type"], "BOOLEAN");
        // target_pane is STRING too
        assert_eq!(props["target_pane"]["type"], "STRING");
    }

    /// Required fields must match the ParamDef required flags.
    #[test]
    fn render_gemini_required_fields_correct() {
        let rendered = render_gemini(TOOLS);
        let arr = rendered.as_array().unwrap();

        // run_terminal_command: only "command" is required
        let rtc = arr
            .iter()
            .find(|e| e["name"] == "run_terminal_command")
            .unwrap();
        let req = rtc["parameters"]["required"].as_array().unwrap();
        assert_eq!(req, &[serde_json::json!("command")]);

        // edit_file: path, old_string, new_string are required
        let ef = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let req_ef: Vec<&str> = ef["parameters"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(req_ef.contains(&"path"));
        assert!(req_ef.contains(&"old_string"));
        assert!(req_ef.contains(&"new_string"));
    }

    /// Tools with no params must not have a "required" key (would be an API error).
    #[test]
    fn render_gemini_no_required_for_empty_params() {
        let rendered = render_gemini(TOOLS);
        let arr = rendered.as_array().unwrap();
        let ls = arr.iter().find(|e| e["name"] == "list_schedules").unwrap();
        assert!(
            ls["parameters"].get("required").is_none(),
            "list_schedules must not have a 'required' key"
        );
    }
}
