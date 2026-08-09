//! The `TOOLS` data table. Intentionally a single flat array literal with no
//! internal seam — kept whole rather than split (see phase-07). It is data, not
//! logic; size here is expected.

use super::schema::{ParamDef, ParamTy, ToolDef};

/// Tools that send the user an approval prompt and wait for a decision before
/// they do anything. Derived by reading every executor arm, not by copying a
/// list: each of these reaches a `Response::*Prompt` send followed by a
/// blocking read.
///
/// **Single source of truth, used for two things.** `README.md` marks these
/// tools with a `⚠` (held in sync by `tests/doc_truth.rs`), and
/// `daemon::stream` exempts them from the per-turn, per-session and per-tool
/// budget caps — the user's prompt is the gate instead, so a call cap would be
/// redundant. `LimitsConfig::validate` reads the same list to warn about
/// `per_tool` entries that can have no effect.
///
/// Until 2026-08-08 the budget exemption was a hand-maintained copy that had
/// drifted from the prompting set in **both** directions: `spawn_ghost_shell`
/// and `delete_schedule` were exempt from every cap while prompting for
/// nothing, and `create_agent` / `delete_agent` prompted the user and were
/// capped anyway. Reconciling them is what made one list possible.
///
/// **Adding a tool here changes runtime behaviour**, not just documentation.
/// Add it only after confirming its executor arm really does send a
/// `Response::*Prompt` and block on the reply.
pub static APPROVAL_GATED_TOOLS: &[&str] = &[
    "create_agent",
    "delete_agent",
    "delete_runbook",
    "delete_script",
    "edit_file",
    "run_terminal_command",
    "schedule_command",
    "tmux_control",
    "write_runbook",
    "write_script",
];

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
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "list_schedules",
        description: "Return the current list of scheduled jobs with their status, schedule, and next run time.",
        params: &[],
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "list_scripts",
        deferred_group: Some("scripts"),
        description: "Return the list of scripts in ~/.daemoneye/scripts/ with their sizes.",
        params: &[],
    },
    ToolDef {
        name: "read_script",
        deferred_group: Some("scripts"),
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
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "read_runbook",
        deferred_group: Some("runbooks"),
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
        deferred_group: Some("runbooks"),
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
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "delete_memory",
        deferred_group: Some("memory"),
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
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "search_repository",
        description: "Search across runbooks, scripts, memory, the event log, archived turns, \
                      or epoch narratives for a keyword. Matching uses stemming (e.g. \
                      'restarting' finds 'restart') and results are relevance-ranked. \
                      kind: 'runbooks' | 'scripts' | 'memory' | 'events' | 'turns' | 'epochs' | 'all'. \
                      Note: 'turns' and 'epochs' are opt-in and are NOT included in 'all'.",
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
                description: "'runbooks', 'scripts', 'memory', 'events', 'turns', 'epochs', or 'all'. \
                              'turns' and 'epochs' are opt-in and not included in 'all'.",
            },
        ],
        deferred_group: None,
    },
    ToolDef {
        name: "recall_context",
        description: "Retrieve archived conversation turns from the current session that were \
                      compacted out of the live context. Search by substring query, by turn \
                      range (epoch summaries in [Session Context] give turn ranges), or both. \
                      Use when you need details an epoch summary or an \"[elided: …]\" \
                      placeholder refers to.",
        params: &[
            ParamDef {
                name: "query",
                ty: ParamTy::Str,
                required: false,
                description: "Optional: substring to search for (case-insensitive).",
            },
            ParamDef {
                name: "turn_start",
                ty: ParamTy::Int,
                required: false,
                description: "Optional: starting turn number (inclusive).",
            },
            ParamDef {
                name: "turn_end",
                ty: ParamTy::Int,
                required: false,
                description: "Optional: ending turn number (inclusive).",
            },
            ParamDef {
                name: "scope",
                ty: ParamTy::Str,
                required: false,
                description: "Optional: \"current\" (default) or \"all\" to search every session.",
            },
        ],
        deferred_group: None,
    },
    ToolDef {
        name: "get_terminal_context",
        description: "Capture a fresh snapshot of the current tmux session: active pane contents, \
                      background panes, session topology, and environment variables. \
                      Call this when you need to see what is on the user's screen, check live \
                      command output, or understand the current terminal state. \
                      The terminal snapshot is NOT automatically included in every message — \
                      call this tool to get it on demand.",
        params: &[ParamDef {
            name: "scope",
            ty: ParamTy::Str,
            required: false,
            description: "Optional breadth: \"window\" (only the chat pane's window), \
                          \"session\" (default, the user's tmux session), or \"all\" \
                          (home session plus foreign-session pane metadata).",
        }],
        deferred_group: None,
    },
    ToolDef {
        name: "load_tools",
        description: "Load an additional group of tools into your available tool set for the rest \
                      of this session. Some rarely-used tools are not loaded by default to save \
                      context; call this to enable a group, then call the tools it contains. \
                      Pass `groups` as an array of group names.",
        params: &[ParamDef {
            name: "groups",
            ty: ParamTy::Str,
            required: true,
            description: "Array of group names to load (e.g. [\"agents\"]). \
                          Accepts a real JSON array or a JSON-encoded string of an array.",
        }],
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "read_pane",
        description: "Read the visible content and scrollback of ANY tmux pane on \
             demand — including panes in other tmux sessions, and daemon-owned \
             background windows. This is how you inspect a pane the context block \
             only summarises in one line. Output is ANSI-annotated ([ERROR:], \
             [WARN:], [OK:]) and masked. The chat pane cannot be read: its content \
             is this conversation. For the user's active pane, get_terminal_context \
             already returns full content.",
        params: &[
            ParamDef {
                name: "pane_id",
                ty: ParamTy::Str,
                required: true,
                description: "tmux pane ID (e.g. \"%3\"). Resolve from [PANE MAP] \
                              (format: idx:N=<id>) or from list_panes.",
            },
            ParamDef {
                name: "lines",
                ty: ParamTy::Int,
                required: false,
                description: "How many lines of scrollback to capture. Defaults to \
                              200, capped at 2000 and at the pane's own history size.",
            },
            ParamDef {
                name: "grep",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex; only matching lines are returned. Use \
                              when the pane holds far more output than you need.",
            },
        ],
        deferred_group: None,
    },
    ToolDef {
        name: "find_in_panes",
        description: "Search every tmux pane's buffer for a regex and return the \
             matching lines with their pane id, window and status. Use this to \
             answer \"which pane has the error?\" in one call instead of reading \
             panes one by one. Output is masked and capped at 50 matches. The \
             chat pane is never searched.",
        params: &[
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: true,
                description: "Regular expression matched against each line of \
                              every pane's buffer.",
            },
            ParamDef {
                name: "scope",
                ty: ParamTy::Str,
                required: false,
                description: "\"session\" (default) searches the cached buffers \
                              of the user's own session. \"all\" additionally \
                              captures panes in other tmux sessions live, which \
                              is slower.",
            },
        ],
        deferred_group: None,
    },
    ToolDef {
        name: "tmux_control",
        description: "Act on the user's tmux session: move their focus to a pane, \
             zoom/unzoom a window, split a pane, rename a window, or kill a window. \
             Every action requires the user's approval before it runs, because each \
             one changes what they are looking at. Use it when the user asks to be \
             taken somewhere, or when showing them a pane is more useful than quoting \
             it back.",
        params: &[
            ParamDef {
                name: "action",
                ty: ParamTy::Str,
                required: true,
                description: "One of: \"focus\" (switch the user to this pane and \
                              its window), \"zoom\" (make this pane fill its \
                              window), \"unzoom\" (undo a zoom), \"split\" (split \
                              the pane into two), \"rename_window\" (rename the \
                              pane's window), \"kill_window\" (close the pane's \
                              window — refused for daemon-managed windows and for \
                              the window holding the chat pane).",
            },
            ParamDef {
                name: "pane_id",
                ty: ParamTy::Str,
                required: true,
                description: "tmux pane ID (e.g. \"%3\") to act on. Resolve from \
                              [PANE MAP] (format: idx:N=<id>) or from list_panes.",
            },
            ParamDef {
                name: "name",
                ty: ParamTy::Str,
                required: false,
                description: "New window name — required for `rename_window`, \
                              ignored otherwise.",
            },
            ParamDef {
                name: "direction",
                ty: ParamTy::Str,
                required: false,
                description: "\"vertical\" (default, new pane below) or \
                              \"horizontal\" (side by side) — `split` only.",
            },
        ],
        deferred_group: None,
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
        deferred_group: None,
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
        deferred_group: None,
    },
    ToolDef {
        name: "create_agent",
        deferred_group: Some("agents"),
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
        deferred_group: Some("agents"),
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
        deferred_group: Some("agents"),
        description: "List all named agents in ~/.daemoneye/agents/ with their descriptions and models.",
        params: &[],
    },
    ToolDef {
        name: "delete_agent",
        deferred_group: Some("agents"),
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
        deferred_group: None,
    },
];
