use crate::ai::types::AiEvent;
use serde::Deserialize;
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
             captured silently and returned to you. Use for system diagnostics (ls, ps, df, \
             curl, systemctl, etc.) and commands that need shell features (pipes, loops, process \
             control). For reading files, prefer read_file instead. If the user is SSH'd into a \
             remote host, this still runs locally on the daemon machine. Supports sudo: the user \
             will be prompted for their password in the chat interface.\n\
             - background=false (default): Injects the command into the USER'S TERMINAL PANE via \
             tmux send-keys. The command is visible and interactive. Use for state-changing \
             commands, service restarts, or anything that must run on the user's active host. \
             For file edits prefer edit_file; for file reads prefer read_file. \
             If the user's pane is SSH'd to a remote machine, the command runs \
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
                description: "Optional: tmux pane ID (e.g. \"%3\") for foreground commands. \
                              Use the pane ID from [FOREGROUND TARGET] when present — that is \
                              the user's designated work pane. Otherwise resolve from [PANE MAP] \
                              (format: idx:N=<id>). If still unsure, call list_panes or ask the \
                              user. Do not set this for background=true commands.",
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
                      saved with chmod 700. Always include a comment header block immediately after \
                      the shebang line so the script is discoverable by tags and search: \
                      `# --- daemoneye ---` / `# tags: [tag1, tag2]` / \
                      `# summary: one-line description` / `# relates_to: [runbook-name]` / \
                      `# --- /daemoneye ---`. Use `//` as the prefix for JS/TS/Rust/Go scripts. \
                      The same field names are used by memory frontmatter — one mental model for \
                      all artifact types. Extra fields (e.g. `run_with_sudo: true`) are also \
                      supported and captured in the header.",
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
                description: "Full content of the script, including the shebang line and \
                              the daemoneye comment header block.",
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
                      approve before the file is removed.",
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
        description: "Preferred over cat/head/tail/grep for reading files. \
                      Read a file with line-range pagination and optional grep filtering. \
                      Sensitive data is masked. \
                      Without target_pane: reads directly from the DAEMON HOST filesystem. \
                      With target_pane: runs in that pane — use this when the file \
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
        description: "Preferred over sed/awk/echo/tee for file modifications. \
                      Perform a file operation on the daemon-host filesystem (or a remote host \
                      via target_pane). Requires user approval before any change is committed. \
                      The approval prompt shows a colored unified diff. \
                      operation=\"edit\" (default): atomically replace old_string with new_string — \
                        old_string must appear exactly once. \
                      operation=\"create\": write a new file with the given content — \
                        fails if the file already exists. \
                      operation=\"delete\": permanently remove a file — \
                        shows the full file content as a deletion diff before prompting. \
                      operation=\"copy\": copy path to dest_path — \
                        fails if dest_path already exists.",
        params: &[
            ParamDef {
                name: "path",
                ty: ParamTy::Str,
                required: true,
                description: "Absolute path to the target file.",
            },
            ParamDef {
                name: "operation",
                ty: ParamTy::Str,
                required: false,
                description: "One of: \"edit\" (default), \"create\", \"delete\", \"copy\". \
                              Determines which other parameters are required.",
            },
            ParamDef {
                name: "old_string",
                ty: ParamTy::Str,
                required: false,
                description: "Required for operation=\"edit\". \
                              Exact text to find — must appear exactly once. \
                              Include enough surrounding context to be unique.",
            },
            ParamDef {
                name: "new_string",
                ty: ParamTy::Str,
                required: false,
                description: "Required for operation=\"edit\". \
                              Replacement text. Use empty string to delete old_string.",
            },
            ParamDef {
                name: "content",
                ty: ParamTy::Str,
                required: false,
                description: "Required for operation=\"create\". \
                              Full content to write to the new file.",
            },
            ParamDef {
                name: "dest_path",
                ty: ParamTy::Str,
                required: false,
                description: "Required for operation=\"copy\". \
                              Absolute destination path. Fails if the destination already exists.",
            },
            ParamDef {
                name: "target_pane",
                ty: ParamTy::Str,
                required: false,
                description: "Optional tmux pane ID. When set, the operation runs inside that \
                              pane via shell commands — use this for files on a remote SSH host. \
                              Omit for daemon-host files.",
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
        name: "update_memory",
        description: "Update specific fields of an existing memory entry without rewriting the \
                      entire file. Only provided fields are changed; omitted fields are preserved. \
                      Creates the entry if it does not exist. Automatically sets the `updated` \
                      timestamp. Prefer this over read+delete+add_memory cycles for partial updates.",
        params: &[
            ParamDef {
                name: "key",
                ty: ParamTy::Str,
                required: true,
                description: "Key of the memory entry to update.",
            },
            ParamDef {
                name: "category",
                ty: ParamTy::Str,
                required: true,
                description: "'session', 'knowledge', or 'incident'.",
            },
            ParamDef {
                name: "body",
                ty: ParamTy::Str,
                required: false,
                description: "New body content. Replaces existing body unless append=true.",
            },
            ParamDef {
                name: "append",
                ty: ParamTy::Bool,
                required: false,
                description: "If true, append body to existing content instead of replacing. Default false.",
            },
            ParamDef {
                name: "tags",
                ty: ParamTy::Str,
                required: false,
                description: "JSON array of tags, e.g. [\"postgres\",\"database\"]. Replaces existing tags.",
            },
            ParamDef {
                name: "summary",
                ty: ParamTy::Str,
                required: false,
                description: "One-line description of this memory entry.",
            },
            ParamDef {
                name: "relates_to",
                ty: ParamTy::Str,
                required: false,
                description: "JSON array of related memory keys, runbook names, or script names.",
            },
            ParamDef {
                name: "expires",
                ty: ParamTy::Str,
                required: false,
                description: "ISO date when this memory expires, e.g. '2026-04-15'. For time-bounded facts.",
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
                      Optionally specify an `agent` to use a named agent profile for model, \
                      prompt, and memory namespace. Returns the ghost session ID, job_id, \
                      and agent name — use these with await_agent_result to collect the result.",
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
            ParamDef {
                name: "agent",
                ty: ParamTy::Str,
                required: false,
                description: "Name of a named agent to use as the executor identity. \
                              Inherits prompt, model, and memory namespace from the agent config. \
                              Omit to use the default ghost shell identity.",
            },
        ],
    },
    ToolDef {
        name: "create_agent",
        description: "Create or update a named agent config in ~/.daemoneye/agents/<name>/config.toml. \
                      An agent defines *who* executes a ghost shell — the role, model, memory namespace, \
                      and trust boundaries — separate from *what* the runbook asks it to do. \
                      User approval required before the config is written.",
        params: &[
            ParamDef {
                name: "name",
                ty: ParamTy::Str,
                required: true,
                description: "Unique slug for the agent (lowercase letters, digits, hyphens; 1-48 chars, no leading/trailing dash).",
            },
            ParamDef {
                name: "description",
                ty: ParamTy::Str,
                required: true,
                description: "Short human-readable description shown in listings.",
            },
            ParamDef {
                name: "prompt",
                ty: ParamTy::Str,
                required: true,
                description: "Role-defining system prompt addition layered on top of the default SRE prompt when this agent runs.",
            },
            ParamDef {
                name: "model",
                ty: ParamTy::Str,
                required: false,
                description: "Model key from [models.*] in config.toml. Omit to use daemon default.",
            },
            ParamDef {
                name: "memory_namespace",
                ty: ParamTy::Str,
                required: false,
                description: "Memory namespace for this agent's scoped memories. Defaults to the agent name if empty.",
            },
            ParamDef {
                name: "max_turns",
                ty: ParamTy::Int,
                required: false,
                description: "Per-invocation turn budget. Omit to use daemon default.",
            },
            ParamDef {
                name: "auto_approve_read_only",
                ty: ParamTy::Bool,
                required: false,
                description: "Allow the agent to run non-sudo commands without listing them in auto_approve_scripts. Default false.",
            },
            ParamDef {
                name: "auto_approve_scripts",
                ty: ParamTy::Str,
                required: false,
                description: "JSON array of script names in ~/.daemoneye/scripts/ pre-approved for sudo execution. Accepts either a real array [\"check.sh\"] or a JSON-encoded string \"[\\\"check.sh\\\"]\".",
            },
        ],
    },
    ToolDef {
        name: "read_agent",
        description: "Read the full config of a named agent from ~/.daemoneye/agents/<name>/config.toml.",
        params: &[ParamDef {
            name: "name",
            ty: ParamTy::Str,
            required: true,
            description: "Name of the agent to read.",
        }],
    },
    ToolDef {
        name: "list_agents",
        description: "List all named agents in ~/.daemoneye/agents/ with their descriptions and models.",
        params: &[],
    },
    ToolDef {
        name: "delete_agent",
        description: "Delete a named agent from ~/.daemoneye/agents/<name>/. User approval required. \
                      Will warn if any runbooks reference this agent.",
        params: &[ParamDef {
            name: "name",
            ty: ParamTy::Str,
            required: true,
            description: "Name of the agent to delete.",
        }],
    },
    ToolDef {
        name: "await_agent_result",
        description: "Wait for a spawned agent ghost shell to complete and return its result. \
                      Polls the specified agent's mailbox for a `job_id` returned by `spawn_ghost_shell`. \
                      Blocks until the result is available or the timeout expires. \
                      Use this after spawning specialist agents to collect their findings. \
                      The `agent_name` must match the agent that the child ghost shell was spawned with.",
        params: &[
            ParamDef {
                name: "job_id",
                ty: ParamTy::Str,
                required: true,
                description: "Job ID of the spawned ghost shell (returned by spawn_ghost_shell).",
            },
            ParamDef {
                name: "agent_name",
                ty: ParamTy::Str,
                required: true,
                description: "Name of the agent that the child ghost shell was spawned with. \
                              The child writes its result to this agent's mailbox directory.",
            },
            ParamDef {
                name: "timeout_secs",
                ty: ParamTy::Int,
                required: false,
                description: "Maximum seconds to wait before timing out. Default 300, capped at 3600.",
            },
        ],
    },
];

// ---------------------------------------------------------------------------
// Provider renderers
// ---------------------------------------------------------------------------

/// Closed value set for a parameter, keyed by param name. Returned as a JSON
/// `enum` in every provider's schema so the model is constrained to valid values
/// instead of relying on the prose description. Param names are globally unique
/// across `TOOLS` (operation→edit_file, kind→search_repository) or share one set
/// (category→the five memory tools), so name-keying is unambiguous.
fn enum_values(param_name: &str) -> Option<&'static [&'static str]> {
    match param_name {
        "operation" => Some(&["edit", "create", "delete", "copy"]),
        "kind" => Some(&["runbooks", "scripts", "memory", "events", "all"]),
        "category" => Some(&["session", "knowledge", "incident"]),
        _ => None,
    }
}

fn build_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            let mut schema = json!({
                "type": p.ty.as_str(),
                "description": p.description,
            });
            if let Some(values) = enum_values(p.name) {
                schema["enum"] = json!(values);
            }
            (p.name.to_string(), schema)
        })
        .collect()
}

fn build_gemini_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            let mut schema = json!({
                "type": p.ty.as_gemini_str(),
                "description": p.description,
            });
            if let Some(values) = enum_values(p.name) {
                schema["enum"] = json!(values);
            }
            (p.name.to_string(), schema)
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

/// Trait for typed tool-argument deserialization + AiEvent construction.
/// Every tool in `TOOLS` must have a corresponding impl — the
/// `dispatch_roundtrip` test verifies coverage at compile time.
pub trait ToolArgs: Sized {
    fn from_value(value: Value) -> Option<Self>;
    fn to_event(self, id: &str, ts: Option<String>) -> AiEvent;
}

// ── Typed arg structs ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RunTerminalCommandArgs {
    command: String,
    #[serde(default)]
    background: bool,
    target_pane: Option<String>,
    retry_in_pane: Option<String>,
}

#[derive(Deserialize)]
struct ScheduleCommandArgs {
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
struct CancelDeleteScheduleArgs {
    id: String,
}

#[derive(Deserialize)]
struct CloseBgWindowArgs {
    pane_id: String,
}

#[derive(Deserialize)]
struct WriteScriptArgs {
    script_name: String,
    content: String,
}

#[derive(Deserialize)]
struct ReadScriptArgs {
    script_name: String,
}

#[derive(Deserialize)]
struct DeleteScriptArgs {
    script_name: String,
}

#[derive(Deserialize)]
struct WatchPaneArgs {
    pane_id: String,
    #[serde(default = "default_300")]
    timeout_secs: u64,
    pattern: Option<String>,
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<String>,
    target_pane: Option<String>,
}

#[derive(Deserialize)]
struct EditFileArgs {
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
struct WriteRunbookArgs {
    name: String,
    content: String,
}

#[derive(Deserialize)]
struct ReadRunbookArgs {
    name: String,
}

#[derive(Deserialize)]
struct AddMemoryArgs {
    key: String,
    value: String,
    #[serde(default = "default_knowledge")]
    category: String,
}

#[derive(Deserialize)]
struct UpdateMemoryArgs {
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
struct DeleteMemoryArgs {
    key: String,
    #[serde(default = "default_knowledge")]
    category: String,
}

#[derive(Deserialize)]
struct ReadMemoryArgs {
    key: String,
    #[serde(default = "default_knowledge")]
    category: String,
}

#[derive(Deserialize)]
struct ListMemoriesArgs {
    category: Option<String>,
}

#[derive(Deserialize)]
struct SearchRepositoryArgs {
    query: String,
    #[serde(default = "default_all")]
    kind: String,
}

#[derive(Deserialize)]
struct SpawnGhostArgs {
    runbook: String,
    message: String,
    agent: Option<String>,
}

#[derive(Deserialize)]
struct CreateAgentArgs {
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
struct ReadAgentArgs {
    name: String,
}

#[derive(Deserialize)]
struct AwaitAgentResultArgs {
    job_id: String,
    agent_name: String,
    #[serde(default = "default_300")]
    timeout_secs: u64,
}

// ── Default helpers ────────────────────────────────────────────────────────

fn default_unnamed() -> String {
    "unnamed".to_string()
}

fn default_300() -> u64 {
    300
}

fn default_edit() -> String {
    "edit".to_string()
}

fn default_knowledge() -> String {
    "knowledge".to_string()
}

fn default_all() -> String {
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
fn schedule_id_event<T>(args: Value, ts: Option<String>, mk: T) -> Option<AiEvent>
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
fn runbook_name_event<T>(args: Value, ts: Option<String>, mk: T) -> Option<AiEvent>
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
fn extract_string_vec(v: &Value) -> Option<Vec<String>> {
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
        "get_terminal_context" => Some(AiEvent::GetTerminalContext {
            id: id.to_string(),
            thought_signature: ts,
        }),
        "list_panes" => Some(AiEvent::ListPanes {
            id: id.to_string(),
            thought_signature: ts,
        }),
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
        "watch_pane" => dispatch::<WatchPaneArgs>(id, args, ts),
        "read_file" => dispatch::<ReadFileArgs>(id, args, ts),
        "edit_file" => dispatch::<EditFileArgs>(id, args, ts),
        "write_runbook" => dispatch::<WriteRunbookArgs>(id, args, ts),
        "delete_runbook" => runbook_name_event(args, ts, |nm, t| AiEvent::DeleteRunbook {
            id: id.to_string(),
            name: nm,
            thought_signature: t,
        }),
        "read_runbook" => dispatch::<ReadRunbookArgs>(id, args, ts),
        "add_memory" => dispatch::<AddMemoryArgs>(id, args, ts),
        "update_memory" => dispatch::<UpdateMemoryArgs>(id, args, ts),
        "delete_memory" => dispatch::<DeleteMemoryArgs>(id, args, ts),
        "read_memory" => dispatch::<ReadMemoryArgs>(id, args, ts),
        "list_memories" => dispatch::<ListMemoriesArgs>(id, args, ts),
        "search_repository" => dispatch::<SearchRepositoryArgs>(id, args, ts),
        "close_background_window" => dispatch::<CloseBgWindowArgs>(id, args, ts),
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

        // edit_file: only "path" is required; old_string/new_string/content/operation are optional
        let ef = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let req_ef: Vec<&str> = ef["parameters"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(req_ef.contains(&"path"));
        assert!(
            !req_ef.contains(&"old_string"),
            "old_string should now be optional"
        );
        assert!(
            !req_ef.contains(&"new_string"),
            "new_string should now be optional"
        );
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

    /// Every tool in TOOLS must have a dispatch arm that accepts conformant args.
    /// This catches the case where a tool is added to TOOLS but forgotten in
    /// dispatch_tool_event, or where a required param name diverges between the
    /// ToolDef schema and the typed arg struct.
    #[test]
    fn dispatch_roundtrip_all_tools() {
        fn minimal_args(name: &str) -> Value {
            match name {
                "run_terminal_command" => json!({"command": "echo hi"}),
                "schedule_command" => json!({"name": "job"}),
                "list_schedules" => json!({}),
                "cancel_schedule" => json!({"id": "00000000-0000-0000-0000-000000000000"}),
                "delete_schedule" => json!({"id": "00000000-0000-0000-0000-000000000000"}),
                "write_script" => json!({"script_name": "s.sh", "content": "#!/bin/sh"}),
                "list_scripts" => json!({}),
                "read_script" => json!({"script_name": "s.sh"}),
                "delete_script" => json!({"script_name": "s.sh"}),
                "watch_pane" => json!({"pane_id": "%1"}),
                "read_file" => json!({"path": "/tmp/f"}),
                "edit_file" => json!({"path": "/tmp/f"}),
                "write_runbook" => json!({"name": "rb", "content": "# RB"}),
                "delete_runbook" => json!({"name": "rb"}),
                "read_runbook" => json!({"name": "rb"}),
                "list_runbooks" => json!({}),
                "add_memory" => json!({"key": "k", "value": "v", "category": "knowledge"}),
                "update_memory" => json!({"key": "k", "category": "knowledge"}),
                "delete_memory" => json!({"key": "k", "category": "knowledge"}),
                "read_memory" => json!({"key": "k", "category": "knowledge"}),
                "list_memories" => json!({}),
                "search_repository" => json!({"query": "x", "kind": "all"}),
                "get_terminal_context" => json!({}),
                "list_panes" => json!({}),
                "close_background_window" => json!({"pane_id": "%1"}),
                "spawn_ghost_shell" => json!({"runbook": "rb", "message": "investigate"}),
                "create_agent" => {
                    json!({"name": "analyst", "description": "test", "prompt": "You are a test."})
                }
                "read_agent" => json!({"name": "analyst"}),
                "list_agents" => json!({}),
                "delete_agent" => json!({"name": "analyst"}),
                "await_agent_result" => json!({"job_id": "ghost-abc-123", "agent_name": "analyst"}),
                _ => json!({}),
            }
        }

        for tool in TOOLS {
            let args = minimal_args(tool.name);
            let ev = dispatch_tool_event("tc_1", tool.name, &args, None);
            assert!(
                ev.is_some(),
                "dispatch_tool_event returned None for tool '{}'. \
                 Either the dispatch arm is missing or the arg struct's required fields \
                 don't match the ToolDef schema.",
                tool.name
            );
        }
    }

    /// The model must not be able to inject a namespace into a read tool.
    /// This is the direct lock: if any read tool ever gains a `namespace` or
    /// `namespaces` param, this test fails.
    #[test]
    fn read_tools_expose_no_namespace_param() {
        for tool in ["read_memory", "list_memories", "search_repository"] {
            let def = TOOLS
                .iter()
                .find(|t| t.name == tool)
                .unwrap_or_else(|| panic!("tool {tool} missing from TOOLS"));
            for p in def.params {
                assert!(
                    p.name != "namespace" && p.name != "namespaces",
                    "{tool} must not expose a namespace param (got '{}') — the namespace \
                     set is built server-side, never caller-supplied",
                    p.name
                );
            }
        }
    }

    #[test]
    fn enum_values_known_params() {
        assert_eq!(
            enum_values("operation"),
            Some(&["edit", "create", "delete", "copy"] as &[&str])
        );
        assert_eq!(
            enum_values("kind"),
            Some(&["runbooks", "scripts", "memory", "events", "all"] as &[&str])
        );
        assert_eq!(
            enum_values("category"),
            Some(&["session", "knowledge", "incident"] as &[&str])
        );
        assert!(enum_values("command").is_none());
        assert!(enum_values("nonexistent").is_none());
    }

    #[test]
    fn anthropic_render_emits_enums() {
        let rendered = render_anthropic(TOOLS);
        let arr = rendered.as_array().unwrap();

        let edit_file = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let op = &edit_file["input_schema"]["properties"]["operation"];
        assert_eq!(
            op.get("enum"),
            Some(&json!(["edit", "create", "delete", "copy"])),
            "edit_file.operation must carry an enum"
        );

        let search = arr
            .iter()
            .find(|e| e["name"] == "search_repository")
            .unwrap();
        let kind = &search["input_schema"]["properties"]["kind"];
        assert_eq!(
            kind.get("enum"),
            Some(&json!(["runbooks", "scripts", "memory", "events", "all"])),
            "search_repository.kind must carry an enum"
        );

        let add_mem = arr.iter().find(|e| e["name"] == "add_memory").unwrap();
        let cat = &add_mem["input_schema"]["properties"]["category"];
        assert_eq!(
            cat.get("enum"),
            Some(&json!(["session", "knowledge", "incident"])),
            "add_memory.category must carry an enum"
        );

        let run_cmd = arr
            .iter()
            .find(|e| e["name"] == "run_terminal_command")
            .unwrap();
        let cmd = &run_cmd["input_schema"]["properties"]["command"];
        assert!(
            cmd.get("enum").is_none(),
            "run_terminal_command.command must NOT carry an enum"
        );
    }

    #[test]
    fn gemini_render_emits_enums() {
        let rendered = render_gemini(TOOLS);
        let arr = rendered.as_array().unwrap();

        let edit_file = arr.iter().find(|e| e["name"] == "edit_file").unwrap();
        let op = &edit_file["parameters"]["properties"]["operation"];
        assert_eq!(
            op.get("enum"),
            Some(&json!(["edit", "create", "delete", "copy"])),
            "edit_file.operation must carry an enum in Gemini render"
        );

        let search = arr
            .iter()
            .find(|e| e["name"] == "search_repository")
            .unwrap();
        let kind = &search["parameters"]["properties"]["kind"];
        assert_eq!(
            kind.get("enum"),
            Some(&json!(["runbooks", "scripts", "memory", "events", "all"])),
            "search_repository.kind must carry an enum in Gemini render"
        );

        let add_mem = arr.iter().find(|e| e["name"] == "add_memory").unwrap();
        let cat = &add_mem["parameters"]["properties"]["category"];
        assert_eq!(
            cat.get("enum"),
            Some(&json!(["session", "knowledge", "incident"])),
            "add_memory.category must carry an enum in Gemini render"
        );
    }

    #[test]
    fn create_agent_accepts_array_and_string_scripts() {
        // As a real JSON array
        let ev_array = dispatch_tool_event(
            "tc_1",
            "create_agent",
            &json!({
                "name": "analyst",
                "description": "test",
                "prompt": "You are a test.",
                "auto_approve_scripts": ["a.sh"],
            }),
            None,
        );
        if let Some(AiEvent::CreateAgent {
            auto_approve_scripts,
            ..
        }) = ev_array
        {
            assert_eq!(auto_approve_scripts, vec!["a.sh"]);
        } else {
            panic!("expected AiEvent::CreateAgent from array input");
        }

        // As a JSON-encoded string
        let ev_string = dispatch_tool_event(
            "tc_1",
            "create_agent",
            &json!({
                "name": "analyst",
                "description": "test",
                "prompt": "You are a test.",
                "auto_approve_scripts": "[\"a.sh\"]",
            }),
            None,
        );
        if let Some(AiEvent::CreateAgent {
            auto_approve_scripts,
            ..
        }) = ev_string
        {
            assert_eq!(auto_approve_scripts, vec!["a.sh"]);
        } else {
            panic!("expected AiEvent::CreateAgent from string input");
        }

        // Omitted — defaults to empty
        let ev_omit = dispatch_tool_event(
            "tc_1",
            "create_agent",
            &json!({
                "name": "analyst",
                "description": "test",
                "prompt": "You are a test.",
            }),
            None,
        );
        if let Some(AiEvent::CreateAgent {
            auto_approve_scripts,
            ..
        }) = ev_omit
        {
            assert!(auto_approve_scripts.is_empty());
        } else {
            panic!("expected AiEvent::CreateAgent from omitted input");
        }
    }
}
