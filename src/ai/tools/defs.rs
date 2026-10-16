//! The `TOOLS` data table. Intentionally a single flat array literal with no
//! internal seam — kept whole rather than split (see phase-07). It is data, not
//! logic; size here is expected.

use super::schema::{Category, ParamDef, ParamTy, ToolDef};

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
    "send_input",
    "write_runbook",
    "write_script",
];

pub static TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "run_terminal_command",
        description: "Execute a bash command in one of two modes:\n\
             - wait=true (default): runs in a dedicated PTY shell on the daemon host and \
             waits for the command to finish, returning its captured output, exit status \
             and shell id. Use for system diagnostics (ls, ps, df, curl, systemctl, etc.) \
             and commands that need shell features (pipes, loops, process control). For \
             reading files, prefer read_file instead. Supports sudo: the user will be \
             prompted for their password in the chat interface.\n\
             - wait=false: starts the command in a dedicated shell and returns its shell \
             id immediately, without waiting for the command to finish. Use for \
             long-running commands you want to keep going while you do something else. \
             Follow up with watch_shell to wait for a specific line of output or read_shell \
             to read what it has recorded so far. The command keeps running in that shell \
             after this call returns. For file edits prefer edit_file; for file reads \
             prefer read_file.",
        params: &[
            ParamDef {
                name: "command",
                ty: ParamTy::Str,
                required: true,
                description: "The bash command to execute.",
            },
            ParamDef {
                name: "wait",
                ty: ParamTy::Bool,
                required: false,
                description: "Whether to wait for the command to finish. Default true: the \
                              call blocks until the command completes and returns the full \
                              output. wait: false returns immediately with the shell id the \
                              command runs in — the output keeps accumulating there; follow \
                              it up with watch_shell (wait for a line) or read_shell (read \
                              what it has recorded so far).",
            },
            ParamDef {
                name: "shell",
                ty: ParamTy::Str,
                required: false,
                description: "Optional shell id (\"s3\") from list_shells to run the command \
                              in (PTY backend only); or \"new\" to open a fresh shell for this \
                              command. Omitted: the command reuses this session's \
                              lowest-numbered open shell. A shell owned by another session \
                              or a ghost job is refused, and a non-wait run's dedicated \
                              shell is not a target.",
            },
            ParamDef {
                name: "host",
                ty: ParamTy::Str,
                required: false,
                description: "Optional configured host (a [hosts.<name>] key, e.g. \"pinky\") \
                              to run the command on. Omitted or \"local\": the daemon host. \
                              With host and no shell the command reuses this session's \
                              lowest-numbered open shell on that host, or opens one there; \
                              a shell id naming a shell on a **different** host is refused \
                              (the refusal names both hosts) rather than relocated — a \
                              command runs only where you asked.",
            },
            ParamDef {
                name: "timeout_secs",
                ty: ParamTy::Int,
                required: false,
                description: "Optional maximum seconds to wait for the command (PTY backend \
                              only). Default 45, capped at 3600. 0 means the default.",
            },
        ],
        deferred_group: None,
        category: Category::Run,
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
        category: Category::Write,
    },
    ToolDef {
        name: "list_schedules",
        description: "Return the current list of scheduled jobs with their status, schedule, and next run time.",
        params: &[],
        deferred_group: None,
        category: Category::Read,
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
        category: Category::Write,
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
        category: Category::Write,
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
        category: Category::Write,
    },
    ToolDef {
        name: "list_scripts",
        deferred_group: Some("scripts"),
        category: Category::Read,
        description: "Return the list of scripts in ~/.daemoneye/scripts/ with their sizes.",
        params: &[],
    },
    ToolDef {
        name: "read_script",
        deferred_group: Some("scripts"),
        category: Category::Read,
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
        category: Category::Write,
    },
    ToolDef {
        name: "read_file",
        description: "Preferred over cat/head/tail/grep for reading files. \
                      Read a file with line-range pagination and optional grep filtering. \
                      Sensitive data is masked. \
                      Without host: reads directly from the DAEMON HOST filesystem. \
                      With host: reads on that host through a shell there — use this \
                      when the file is on a remote SSH host.",
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
                name: "host",
                ty: ParamTy::Str,
                required: false,
                description: "Optional configured host (a [hosts.<name>] key, e.g. \"pinky\") \
                              to read the file on. Omitted or \"local\": the daemon host. \
                              When set, the read runs in this session's lowest-numbered open \
                              shell on that host (or opens one there) and no approval is \
                              needed — a read changes nothing.",
            },
        ],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "edit_file",
        description: "Preferred over sed/awk/echo/tee for file modifications. \
                      Perform a file operation on the daemon-host filesystem (or on a \
                      configured remote host via host). Requires user approval before any \
                      change is committed. \
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
                name: "host",
                ty: ParamTy::Str,
                required: false,
                description: "Optional configured host (a [hosts.<name>] key, e.g. \"pinky\") \
                              to write the file on. Omitted or \"local\": the daemon host. \
                              When set, the operation runs in this session's lowest-numbered \
                              open shell on that host (or opens one there) after the same \
                              approval prompt — a write is gated on a remote exactly as it \
                              is locally.",
            },
            ParamDef {
                name: "sudo",
                ty: ParamTy::Bool,
                required: false,
                description: "Optional. true runs an operation=\"edit\" as root on the \
                              configured host named by host, answering the password prompt \
                              through the user. Only for operation=\"edit\" with a host; \
                              always asks for approval, never approved for the session.",
            },
        ],
        deferred_group: None,
        category: Category::Write,
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
        category: Category::Write,
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
        category: Category::Write,
    },
    ToolDef {
        name: "read_runbook",
        deferred_group: Some("runbooks"),
        category: Category::Read,
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
        category: Category::Read,
        description: "List all runbooks in ~/.daemoneye/runbooks/ with their tags.",
        params: &[],
    },
    ToolDef {
        name: "run_log_tail",
        deferred_group: Some("runs"),
        category: Category::Read,
        description: "Read the last N records of a run's turn log (var/log/runs/<run_id>.jsonl), \
                      oldest first. Records are run_started / turn_started / note / run_finished \
                      events with timestamps. limit defaults to 50 and is capped at 200; the \
                      header of the result always says how many records exist and whether the \
                      request was capped. Use run_log_search to find specific text instead of \
                      paging through the whole log.",
        params: &[
            ParamDef {
                name: "run_id",
                ty: ParamTy::Str,
                required: true,
                description: "The 16-hex id of the run whose turn log to read.",
            },
            ParamDef {
                name: "limit",
                ty: ParamTy::Int,
                required: false,
                description: "How many of the most recent records to show. Default 50, \
                              maximum 200 (larger values are clamped and the result says so).",
            },
        ],
    },
    ToolDef {
        name: "run_log_search",
        deferred_group: Some("runs"),
        category: Category::Search,
        description: "Search one run's turn log for records whose text contains the query, \
                      case-insensitively. Returns matching events (notes, turn boundaries, \
                      finish states) oldest first. limit defaults to 50 and is capped at 200. \
                      Use this to find when something happened inside a long run without paging.",
        params: &[
            ParamDef {
                name: "run_id",
                ty: ParamTy::Str,
                required: true,
                description: "The 16-hex id of the run whose turn log to search.",
            },
            ParamDef {
                name: "query",
                ty: ParamTy::Str,
                required: true,
                description: "Substring to look for, case-insensitively.",
            },
            ParamDef {
                name: "limit",
                ty: ParamTy::Int,
                required: false,
                description: "Maximum number of matching records to show. Default 50, \
                              maximum 200 (larger values are clamped and the result says so).",
            },
        ],
    },
    ToolDef {
        name: "get_turn",
        deferred_group: Some("runs"),
        category: Category::Read,
        description: "Read every record one turn logged, in order — the uncapped form of \
                      run_log_tail for a caller that has already narrowed to a single turn. \
                      Returns the turn_started line, each note after it, and the next \
                      turn_started (exclusive). Errors as a result line when the run has no \
                      such turn; never an exception.",
        params: &[
            ParamDef {
                name: "run_id",
                ty: ParamTy::Str,
                required: true,
                description: "The 16-hex id of the run whose turn log to read.",
            },
            ParamDef {
                name: "turn",
                ty: ParamTy::Int,
                required: true,
                description: "Which turn to read (the turn numbers the run's own \
                              turn_started events carry).",
            },
        ],
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
        category: Category::Write,
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
        category: Category::Write,
    },
    ToolDef {
        name: "delete_memory",
        deferred_group: Some("memory"),
        category: Category::Write,
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
        category: Category::Read,
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
        category: Category::Read,
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
        category: Category::Search,
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
        category: Category::Search,
    },
    ToolDef {
        name: "get_shell_context",
        description: "Capture a fresh snapshot of the terminal state: the open PTY \
                      shells and which one commands land in by default, and — unless \
                      you narrow the scope — the user's tmux panes, session topology \
                      and environment variables. Call this when you need to see what \
                      is on the user's screen, check live command output, or find out \
                      which shells exist. The snapshot is NOT automatically included \
                      in every message — call this tool to get it on demand.",
        params: &[ParamDef {
            name: "scope",
            ty: ParamTy::Str,
            required: false,
            description: "Optional breadth: \"shells\" (only the open shells and the \
                          default one — the cheapest call, no terminal capture), \
                          \"session\" (default, the user's session with its panes) or \
                          \"all\" (adds pane metadata for other tmux sessions).",
        }],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "list_shells",
        description: "List the PTY shells the daemon is running on the daemon host, with \
                      each shell's id (like \"s3\"), owner, age, and flags (`dedicated` / \
                      `exited`) plus what held the shell when last observed (`running <cmd>`, \
                      `running <cmd> (TUI)`, `paused <cmd>`; exited shells are kept listed). \
                      An observability tool: use it to see which shells exist and what \
                      state they are in.",
        params: &[ParamDef {
            name: "scope",
            ty: ParamTy::Str,
            required: false,
            description: "Optional: \"mine\" (default — only the shells this session owns) \
                          or \"all\" (every shell the daemon is running). Any other value \
                          is rejected.",
        }],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "read_shell",
        description: "Read the recorded output of a PTY shell's past commands — its command \
                      log, not the live screen. Each command the shell ran is recorded with \
                      its exit code; `since_command` selects from one command's index \
                      onward, `filter` matches lines of the recorded output, `lines` clamps \
                      the result to its last N lines. Use it after or during a background \
                      command's run to see what it printed. This does not change or drive a \
                      shell; it reads what the shell already recorded.",
        params: &[
            ParamDef {
                name: "shell",
                ty: ParamTy::Str,
                required: true,
                description: "Shell id to read (e.g. \"s3\"). Get ids from list_shells.",
            },
            ParamDef {
                name: "lines",
                ty: ParamTy::Int,
                required: false,
                description: "How many lines of the tail to return. Defaults to 200, \
                              capped at 2000. Omit to get the whole selection clamped.",
            },
            ParamDef {
                name: "since_command",
                ty: ParamTy::Int,
                required: false,
                description: "Only commands from this command index onward — 0 is the \
                              shell's first recorded command. Omit to read everything. \
                              An index beyond the highest recorded one is an error.",
            },
            ParamDef {
                name: "filter",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex; only matching lines of the recorded output \
                              are returned. A syntactically invalid regex is an error, \
                              not a literal match.",
            },
        ],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "read_shell_screen",
        description: "Read a PTY shell's live screen — what a person looking at that \
                      terminal would see right now, as of this call. The header names \
                      what holds the terminal (`shell prompt`, `running <cmd>`, \
                      `running <cmd> (TUI)`, `paused <cmd>`), the size, the cursor and \
                      whether a full-screen app is up. Use it to see an interactive \
                      program such as an editor, a pager or Claude Code, or to check \
                      what a shell is doing before typing into it with send_input. \
                      Inverse-video runs (a menu's selected row) appear as \
                      `[SEL: …]`. This is the viewport, and it is lossy: output that \
                      scrolled away or was redrawn is gone. For what a command \
                      printed, the record is read_shell / read_shell_log. \
                      wait_for / settle_ms make it wait before reading: block until \
                      the screen shows a pattern or goes quiet (e.g. Claude Code \
                      finished its turn), then return the screen as it now stands.",
        params: &[
            ParamDef {
                name: "shell",
                ty: ParamTy::Str,
                required: true,
                description: "Shell id to read (e.g. \"s3\"). Get ids from list_shells.",
            },
            ParamDef {
                name: "lines",
                ty: ParamTy::Int,
                required: false,
                description: "Return only the last N visible rows. Omit for the whole \
                              screen.",
            },
            ParamDef {
                name: "scrollback",
                ty: ParamTy::Int,
                required: false,
                description: "Also return up to N rows that scrolled off the top, in a \
                              section above the screen (not counted by `lines`). An \
                              inline TUI's earlier output lives there.",
            },
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex; only matching rows are returned. A \
                              syntactically invalid regex is an error.",
            },
            ParamDef {
                name: "annotated",
                ty: ParamTy::Bool,
                required: false,
                description: "Also wrap red / yellow / green runs as [ERROR: …] / \
                              [WARN: …] / [OK: …]. Default false: a TUI's colours are \
                              rarely those meanings.",
            },
            ParamDef {
                name: "wait_for",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex; block until the rendered screen shows it, \
                              then return the screen. Matching happens on the screen's \
                              **text** as rendered, not the byte stream, so it works on \
                              TUI output where watch_shell's match does not. A \
                              syntactically invalid regex is an error. Bounded by \
                              `timeout_secs`; running out is a result, not an error.",
            },
            ParamDef {
                name: "settle_ms",
                ty: ParamTy::Int,
                required: false,
                description: "Block until the screen goes quiet: two reads this many \
                              milliseconds apart show the same text. The primitive for \
                              \"the app finished drawing\" — after typing into Claude \
                              Code, settle_ms: 1500 returns the finished answer. A \
                              screen that never settles (top, a clock, a spinner) is a \
                              result, not an error: use wait_for there. Bounds: 50–5000, \
                              bounded by timeout_secs.",
            },
            ParamDef {
                name: "timeout_secs",
                ty: ParamTy::Int,
                required: false,
                description: "Maximum seconds to wait for wait_for / settle_ms. Default \
                              30, clamped to 1–300. With neither wait parameter set this \
                              is ignored and the call returns immediately.",
            },
        ],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "find_in_shells",
        description: "Search the recorded output of the PTY shells the daemon is \
                      running for a regular expression, in one call — the way \
                      to answer \"which shell printed the error?\" without \
                      reading each shell's log. Each match reports the shell \
                      id, its 1-indexed line number in the recording, and the \
                      line itself with one line of context either side. Like \
                      read_shell, this searches the per-command recorded log, \
                      not any live screen. The most recent 2000 lines of each \
                      shell's recording are searched and the result is capped \
                      at 50 matches across all shells; the output says when \
                      the cap was hit. A shell whose recording cannot be read \
                      is skipped and named, not fatal.",
        params: &[
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: true,
                description: "Regular expression matched against every line of \
                              every in-scope shell's recorded output.",
            },
            ParamDef {
                name: "scope",
                ty: ParamTy::Str,
                required: false,
                description: "Optional: \"mine\" (default — only the shells this \
                              session owns) or \"all\" (every shell the daemon is \
                              running). Any other value is rejected.",
            },
        ],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "open_shell",
        description: "Start a new PTY shell and return its id. Opens it on the daemon \
                      host by default, or on a configured host when you name one \
                      with `host`. A \
                      created shell is ordinary: list_shells lists it, read_shell and \
                      find_in_shells read its recorded output, and close_shell ends it. \
                      **Note:** commands keep running in this session's \
                      lowest-numbered open shell unless you say otherwise — pass \
                      `shell: \"<id>\"` to run_terminal_command to target a specific \
                      shell, or `shell: \"new\"` to make one command use a fresh shell.",
        params: &[
            ParamDef {
                name: "host",
                ty: ParamTy::Str,
                required: false,
                description: "Optional configured host name to open the shell on (a \
                              [hosts.<name>] key, e.g. \"pinky\"). Omitted: the daemon \
                              host. The shell is ordinary once open — run commands in it \
                              by passing its id as run_terminal_command's `shell`.",
            },
            ParamDef {
                name: "rows",
                ty: ParamTy::Int,
                required: false,
                description: "Optional terminal height the shell opens with. Default: \
                              the configured [shells] rows (40 unless set). Clamped \
                              to 10..=200.",
            },
            ParamDef {
                name: "cols",
                ty: ParamTy::Int,
                required: false,
                description: "Optional terminal width the shell opens with, e.g. 200 \
                              for a wide TUI. Default: the configured [shells] cols \
                              (120 unless set). Clamped to 40..=500.",
            },
        ],
        deferred_group: None,
        category: Category::Run,
    },
    ToolDef {
        name: "close_shell",
        description: "End one of your PTY shells by id: hang it up (SIGHUP, which also \
                      ends a command running in it) and report whether its host actually \
                      went away. Uses the shell id from list_shells (e.g. \"s3\"). Refuses \
                      a shell this session does not own — another session's or a ghost \
                      job's shell is not yours to end. A host that has not gone away \
                      within a moment is reported as still finishing. Use close_shell to \
                      free a per-owner shell slot or end a shell you no longer need.",
        params: &[ParamDef {
            name: "shell",
            ty: ParamTy::Str,
            required: true,
            description: "Shell id to close (e.g. \"s3\"). Must be one this session owns; \
                          get ids from list_shells.",
        }],
        deferred_group: None,
        category: Category::Run,
    },
    ToolDef {
        name: "watch_shell",
        description: "Block on a PTY shell's arriving output and return what it shows. \
                      With `pattern`: wait until a line of the output written **after \
                      this call** matches the regex, then return that line with one \
                      line of context either side — the way to wait for \"build \
                      finished\" or \"listening on port \\d+\" from a command still \
                      running in a dedicated shell. Without `pattern`: wait until the \
                      command running in the shell finishes, the shell is reported \
                      idle with nothing running, or it pauses, exits or dies. This \
                      tool sees only new output — it never searches what the shell \
                      already recorded; use read_shell for history. The shell to \
                      watch is the dedicated one a wait=false run_terminal_command \
                      creates; list_shells shows which shells exist and which are \
                      dedicated.",
        params: &[
            ParamDef {
                name: "shell",
                ty: ParamTy::Str,
                required: true,
                description: "Shell id to watch (e.g. \"s3\"). Get ids from list_shells.",
            },
            ParamDef {
                name: "timeout_secs",
                ty: ParamTy::Int,
                required: false,
                description: "Maximum seconds to wait. Default 300, capped at 3600.",
            },
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex matched against the shell's output written \
                              after the call. A syntactically invalid regex is an error, \
                              not a literal match.",
            },
        ],
        deferred_group: None,
        category: Category::Read,
    },
    ToolDef {
        name: "send_input",
        description: "Write text, named keys, or both into a live PTY shell's input as \
                      if typed — the way to answer a program's stdin prompt or \
                      drive an interactive process the daemon is already running \
                      (e.g. typing a command at a shell you opened, or responding \
                      to a `(y/n)` question, or pressing Escape to cancel a \
                      menu). It does **not** run the text through a command \
                      wrapper: there is no exit code, it does not wait for the \
                      program to react, and it returns **no output** — read the \
                      effect afterwards with read_shell, read_shell_screen or \
                      watch_shell. A shell sitting at its prompt will run the \
                      text as a command, without that command's approval, capture \
                      or exit code. Requires user approval; a ghost shell may not \
                      use it unless its agent ToolPolicy explicitly allows it.",
        params: &[
            ParamDef {
                name: "shell",
                ty: ParamTy::Str,
                required: true,
                description: "Shell id to type into (e.g. \"s3\"). Get ids from list_shells.",
            },
            ParamDef {
                name: "text",
                ty: ParamTy::Str,
                required: false,
                description: "The characters to write, exactly as if typed. Default \
                              empty, when the call only presses keys.",
            },
            ParamDef {
                name: "keys",
                ty: ParamTy::Str,
                required: false,
                description: "Named keys to press after the text, in order — tmux's key \
                              vocabulary: Enter, Escape, Tab, BTab, Up, Down, Right, Left, \
                              Home, End, PageUp, PageDown, Delete, BSpace, Space, F1 through \
                              F12, or one printable character, with the C-, M- and S- \
                              prefixes in any order (e.g. \"C-c\", \"M-b\", \"S-Enter\"). \
                              A combination with no legacy byte goes out as CSI u; the \
                              result names those keys. Unknown names are an error and \
                              nothing is sent.",
            },
            ParamDef {
                name: "paste",
                ty: ParamTy::Bool,
                required: false,
                description: "Send the text as one bracketed paste (wrapped in the shell's \
                              bracket markers) instead of typed characters — how a \
                              multi-line prompt reaches a program that submits per line. \
                              Default: brackets when the text contains a newline and the \
                              shell has bracketed paste mode on. `paste: true` while that \
                              mode is off is an error, not a silent downgrade.",
            },
            ParamDef {
                name: "enter",
                ty: ParamTy::Bool,
                required: false,
                description: "Press Enter after the text and keys: sends a carriage \
                              return (the byte the Enter key sends), which a shell's line \
                              discipline turns into a newline. Defaults to true when \
                              `keys` is empty and to false when keys are pressed — \
                              `send_input(shell, keys: [\"Escape\"])` does not also press \
                              Enter.",
            },
        ],
        deferred_group: None,
        category: Category::Run,
    },
    ToolDef {
        name: "list_shell_logs",
        description: "List the PTY shell recordings on disk — live sessions and finished \
                      ones alike, newest first. Each row carries the recording's \
                      **log id** (like \"s3-1788470000\", the shell id plus the session's \
                      start timestamp), its age, its command count, and the first \
                      command as a hint of what the session was. Recordings without a \
                      readable command index are listed with \"commands: unknown\" — \
                      never 0. This is the way to ask \"which shell sessions ran \
                      recently?\" and to find the log id read_shell_log takes.",
        params: &[
            ParamDef {
                name: "since",
                ty: ParamTy::Int,
                required: false,
                description: "Optional unix timestamp: keep only recordings started at or \
                              after it.",
            },
            ParamDef {
                name: "limit",
                ty: ParamTy::Int,
                required: false,
                description: "Optional: how many recordings to show. Default 50, \
                              capped at 500 (the result says when a request was clamped).",
            },
        ],
        deferred_group: Some("history"),
        category: Category::Read,
    },
    ToolDef {
        name: "read_shell_log",
        description: "Read one PTY shell **recording** by its log id — the id \
                      `list_shell_logs` prints, like \"s3-1788470000\" (the shell id plus \
                      the session's start timestamp; a bare shell id is refused, because \
                      ids are reused across daemon restarts). Reads finished sessions as \
                      well as live ones: each recorded command's output, masked, with its \
                      exit code in the header when a single command is selected. \
                      `command_index` selects one command, `lines` clamps the tail, \
                      `filter` regex-selects lines. For a live shell by plain id, use \
                      read_shell instead.",
        params: &[
            ParamDef {
                name: "log",
                ty: ParamTy::Str,
                required: true,
                description: "The log id to read, exactly as list_shell_logs prints it \
                              (e.g. \"s3-1788470000\") — not the bare shell id.",
            },
            ParamDef {
                name: "command_index",
                ty: ParamTy::Int,
                required: false,
                description: "Optional: read one command by its recorded index (0 is the \
                              first). An index past the highest recorded is an error \
                              naming the highest. Omit to read every recorded command \
                              in order.",
            },
            ParamDef {
                name: "lines",
                ty: ParamTy::Int,
                required: false,
                description: "How many lines of the tail to return. Defaults to 200, \
                              capped at 2000.",
            },
            ParamDef {
                name: "filter",
                ty: ParamTy::Str,
                required: false,
                description: "Optional regex; only matching lines are returned. A \
                              syntactically invalid regex is an error, not a literal \
                              match.",
            },
        ],
        deferred_group: Some("history"),
        category: Category::Read,
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
        category: Category::Meta,
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
            ParamDef {
                name: "host",
                ty: ParamTy::Str,
                required: false,
                description: "Run the specialist's commands on this configured \
                              [hosts.<name>] host instead of its runbook's ssh_target; \
                              \"local\" means the daemon host. Refused before the ghost \
                              starts unless the host is configured and the specialist's \
                              runbook hosts: list grants it — and, when you are a ghost, \
                              your own hosts: list too.",
            },
        ],
        deferred_group: None,
        category: Category::Run,
    },
    ToolDef {
        name: "create_agent",
        deferred_group: Some("agents"),
        category: Category::Write,
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
        category: Category::Read,
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
        category: Category::Read,
        description: "List all named agents in ~/.daemoneye/agents/ with their descriptions and models.",
        params: &[],
    },
    ToolDef {
        name: "delete_agent",
        deferred_group: Some("agents"),
        category: Category::Write,
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
        category: Category::Read,
    },
];
