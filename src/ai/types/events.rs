use super::wire::TokenBreakdown;

#[derive(Debug)]
pub enum AiEvent {
    Token(String),
    ToolCall {
        id: String,
        cmd: String,
        wait: Option<bool>,
        interactive: bool,
        thought_signature: Option<String>,
        shell: Option<String>,
        host: Option<String>,
        timeout_secs: Option<u64>,
    },
    /// A call to a **known** tool whose arguments do not match its schema.
    /// Nothing is executed; the model gets the mistakes as a tool result it
    /// can retry against (M27 phase-07d, F10).
    InvalidToolArgs {
        id: String,
        name: &'static str,
        arguments: String,
        error: String,
        thought_signature: Option<String>,
    },
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
    ReadFile {
        id: String,
        path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pattern: Option<String>,
        host: Option<String>,
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
        host: Option<String>,
        sudo: Option<bool>,
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
    RecallContext {
        id: String,
        query: Option<String>,
        turn_start: Option<u32>,
        turn_end: Option<u32>,
        scope: Option<String>,
        thought_signature: Option<String>,
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
    /// Read a shell's recorded output (asciicast log slice).
    ReadShell {
        id: String,
        shell: String,
        lines: Option<u64>,
        since_command: Option<u32>,
        filter: Option<String>,
        thought_signature: Option<String>,
    },
    /// Read a shell's live rendered screen (M28 phase 01c); 03a adds the
    /// wait parameters — block until the screen shows `wait_for` or goes
    /// quiet for `settle_ms`, bounded by `timeout_secs`.
    ReadShellScreen {
        id: String,
        shell: String,
        lines: Option<u64>,
        scrollback: Option<u16>,
        pattern: Option<String>,
        annotated: bool,
        wait_for: Option<String>,
        settle_ms: Option<u64>,
        timeout_secs: Option<u64>,
        thought_signature: Option<String>,
    },
    /// Regex-search every in-scope shell's recorded output (M22 phase-03c).
    FindInShells {
        id: String,
        pattern: String,
        scope: Option<String>,
        thought_signature: Option<String>,
    },
    /// Open a new PTY shell for this session (M22 phase-04a), optionally on a
    /// configured `[hosts.<name>]` host (M21 phase-04b). `host` names the
    /// shell's destination; `None` is the daemon host. M28 phase-03c adds
    /// `rows`/`cols`: the size the shell is opened at, `None` for the
    /// configured `[shells]` size.
    OpenShell {
        id: String,
        host: Option<String>,
        rows: Option<u64>,
        cols: Option<u64>,
        thought_signature: Option<String>,
    },
    /// Close one of this session's PTY shells (M22 phase-04a).
    CloseShell {
        id: String,
        shell: String,
        thought_signature: Option<String>,
    },
    /// Block on a PTY shell's arriving output: pattern match on the output
    /// written after the call, the running command's finish, or a timeout
    /// (M22 phase-04b).
    WatchShell {
        id: String,
        shell: String,
        timeout_secs: u64,
        pattern: Option<String>,
        thought_signature: Option<String>,
    },
    /// Write text, named keys, or both into a live PTY shell's input (M22
    /// phase-05; M28 phase-02e adds `keys` and `paste`). Approval-gated; the
    /// result is the host's ack, not the program's reaction — reading that
    /// is `read_shell` / `watch_shell` / `read_shell_screen`.
    SendInput {
        id: String,
        shell: String,
        text: String,
        keys: Vec<String>,
        paste: Option<bool>,
        enter: bool,
        thought_signature: Option<String>,
    },
    /// List the shell recordings on disk, newest first (M22 phase-06b). Each
    /// row carries the **log id** (`<shell id>-<started unix>`), not the bare
    /// shell id.
    ListShellLogs {
        id: String,
        since: Option<u64>,
        limit: Option<u64>,
        thought_signature: Option<String>,
    },
    /// Read one shell recording by its **log id** (M22 phase-06b) — the
    /// filename stem `list_shell_logs` prints, not the bare shell id.
    ReadShellLog {
        id: String,
        log: String,
        command_index: Option<u32>,
        lines: Option<u64>,
        filter: Option<String>,
        thought_signature: Option<String>,
    },
    /// Spawn an autonomous Ghost Shell session in the background.
    SpawnGhost {
        id: String,
        runbook: String,
        message: String,
        agent: Option<String>,
        host: Option<String>,
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
    /// Read the last N records of one run's turn log (M24 phase-02b).
    RunLogTail {
        id: String,
        run_id: String,
        limit: Option<u64>,
        thought_signature: Option<String>,
    },
    /// Search one run's turn log for records containing a query substring.
    RunLogSearch {
        id: String,
        run_id: String,
        query: String,
        limit: Option<u64>,
        thought_signature: Option<String>,
    },
    /// Read every record one turn logged — the uncapped escape hatch.
    GetTurn {
        id: String,
        run_id: String,
        turn: u32,
        thought_signature: Option<String>,
    },
    Done(TokenBreakdown),
    Error(String),
    /// Non-terminal advisory the user should see (truncation, refusal,
    /// dropped/unknown tool call, malformed provider frames). Forwarded to the
    /// chat client as a SystemMsg; log-and-ignore in non-interactive consumers.
    Notice(String),
}
