use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use std::time::Duration;

use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::client::{make_client, AiEvent, Message, ToolCall, ToolResult};
use crate::ai::filter::mask_sensitive;
use crate::config::{Config, load_named_prompt};
use crate::sys_context::get_or_init_sys_context;

/// In-memory record of an active chat session.
/// Evicted by the cleanup task after 30 minutes of inactivity.
struct SessionEntry {
    /// Full trimmed message history for this session (bounded to `MAX_HISTORY`).
    messages: Vec<Message>,
    /// Wall-clock time of the last `Ask` request; used to prune idle sessions.
    last_accessed: Instant,
}

/// Thread-safe, shared session store passed to every client handler.
type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;

const FALLBACK_SESSION: &str = "daemoneye";
/// Maximum number of messages retained per session (in memory and on disk).
const MAX_HISTORY: usize = 40;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the hostname of the machine running the daemon.
fn daemon_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Check whether a tmux pane's foreground process is SSH or mosh.
/// Returns a human-readable description if the pane is on a remote host.
fn get_pane_remote_host(pane_id: &str) -> Option<String> {
    let out = std::process::Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_current_command}"])
        .output()
        .ok()?;
    let cmd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match cmd.as_str() {
        "ssh" | "mosh-client" | "mosh" => Some(format!("remote (via {})", cmd)),
        _ => None,
    }
}

/// True if the command string contains `sudo` as a standalone word.
fn command_has_sudo(cmd: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:^|[;&|])\s*sudo\b").unwrap());
    re.is_match(cmd)
}

/// Rewrite a command that starts with `sudo` to add `-S -p ""` so the
/// password can be piped in via stdin without an interactive prompt.
fn inject_sudo_flags(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some(rest) = t.strip_prefix("sudo ") {
        format!(r#"sudo -S -p "" {}"#, rest)
    } else {
        cmd.to_string()
    }
}

/// Map a process exit code to a human-readable label for AI consumption.
/// Covers the most common POSIX exit codes; anything else becomes "non-zero exit".
fn classify_exit_code(code: i32) -> &'static str {
    match code {
        1   => "generic failure",
        2   => "misuse of shell built-in",
        126 => "permission denied (not executable)",
        127 => "command not found",
        128 => "invalid exit argument",
        130 => "interrupted (Ctrl-C)",
        137 => "killed (SIGKILL / OOM)",
        143 => "terminated (SIGTERM)",
        _   => "non-zero exit",
    }
}

/// Extract the output produced by a foreground command from a post-run pane snapshot.
///
/// `tmux capture-pane -S -N` returns up to N lines of scrollback oldest-first.
/// The relevant content (prompt + command + output) is at the *end* of that
/// string, not the beginning.  We find the command line by searching for the
/// last line in the capture whose text ends with the exact command string (the
/// shell echoes it as `<prompt> <cmd>`).  Everything from that line onward is
/// the command output.
///
/// Falls back to the last 50 lines of the capture when the command line cannot
/// be located (e.g. if the command string itself appears in output lines).
fn extract_command_output(after: &str, cmd: &str) -> String {
    let lines: Vec<&str> = after.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    if !cmd.is_empty() {
        // `rposition` gives the LAST (most recent) matching line so earlier
        // history entries with the same command don't confuse the search.
        if let Some(start) = lines.iter().rposition(|l| l.trim_end().ends_with(cmd)) {
            return lines[start..].join("\n");
        }
    }
    // Fallback: the last 50 lines cover the output of most commands.
    let tail = lines.len().saturating_sub(50);
    lines[tail..].join("\n")
}

/// Normalise command output for display and AI context:
/// - trims trailing whitespace from every line
/// - strips leading and trailing blank lines
/// - returns an empty string when all lines are blank
fn normalize_output(s: &str) -> String {
    let trimmed: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    let start = trimmed.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let end   = trimmed.iter().rposition(|l| !l.is_empty()).map(|i| i + 1).unwrap_or(0);
    if start >= end { return String::new(); }
    trimmed[start..end].join("\n")
}

// ---------------------------------------------------------------------------
// File-backed session persistence
// ---------------------------------------------------------------------------

/// Path to the JSONL file storing a session's message history.
fn session_file(id: &str) -> std::path::PathBuf {
    crate::config::sessions_dir().join(format!("{}.jsonl", id))
}

/// Write the current (already-trimmed) message history to disk, overwriting
/// the previous snapshot.  Failures are non-fatal — we just skip persistence.
fn write_session_file(id: &str, messages: &[Message]) {
    use std::io::Write;
    let path = session_file(id);
    if let Ok(mut f) = std::fs::File::create(&path) {
        for msg in messages {
            if let Ok(line) = serde_json::to_string(msg) {
                let _ = writeln!(f, "{}", line);
            }
        }
    }
}

/// Trim a message history Vec to at most `MAX_HISTORY` entries.
///
/// Layout after trim: `[first_message] [placeholder] [tail…]`
/// - `first_message` is the initial user turn (contains injected system context).
/// - `placeholder` is a synthetic assistant message noting the truncation so the
///   AI understands it is not seeing the full history.
/// - `tail` is the most-recent slice, always starting at an even index (user turn)
///   to keep the strict `user → assistant → user → …` alternation valid.
///
/// Returns `messages` unchanged when `messages.len() <= MAX_HISTORY`.
fn trim_history(messages: Vec<Message>) -> Vec<Message> {
    if messages.len() <= MAX_HISTORY {
        return messages;
    }
    // raw_tail_start ensures result length ≤ MAX_HISTORY:
    //   1 (first) + 1 (placeholder) + (N - tail_start) ≤ MAX_HISTORY
    let raw_tail_start = messages.len() - MAX_HISTORY + 2;
    // Round up to even so the tail begins on a user message.
    let tail_start = if raw_tail_start % 2 == 0 {
        raw_tail_start
    } else {
        raw_tail_start + 1
    };
    let dropped = tail_start - 1;
    let first = messages[0].clone();
    let placeholder = Message {
        role: "assistant".to_string(),
        content: format!(
            "[{} earlier messages were trimmed to fit the context window. \
             The conversation continues from a later point in the session.]",
            dropped
        ),
        tool_calls: None,
        tool_results: None,
    };
    let mut trimmed = Vec::with_capacity(MAX_HISTORY);
    trimmed.push(first);
    trimmed.push(placeholder);
    trimmed.extend_from_slice(&messages[tail_start..]);
    trimmed
}

/// Load message history from a session file, returning at most `MAX_HISTORY`
/// tail messages.  Returns an empty Vec if the file does not exist or is unreadable.
fn read_session_file(id: &str) -> Vec<Message> {
    let path = session_file(id);
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let msgs: Vec<Message> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if msgs.len() <= MAX_HISTORY {
        msgs
    } else {
        msgs[msgs.len() - MAX_HISTORY..].to_vec()
    }
}

/// Append a single-line execution record to the command log.
/// Does nothing when `log_path` is `None` (logging disabled).
fn log_command(
    log_path: Option<&std::path::Path>,
    session_id: Option<&str>,
    mode: &str,
    pane: &str,
    command: &str,
    status: &str,
    output_excerpt: &str,
) {
    let Some(path) = log_path else { return; };

    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let session = session_id.unwrap_or("-");
    // Escape embedded newlines so each log event stays on one line.
    let cmd: String = command.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    let out: String = output_excerpt
        .chars()
        .take(200)
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let line = format!(
        "[{ts}] session={session} mode={mode} pane={pane} status={status} cmd={cmd} out={out}\n"
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Conventional environment variable for each provider's API key.
fn api_key_env_var(provider: &str) -> &'static str {
    match provider {
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    }
}

/// Return the effective API key: config value if non-empty, else the env var.
fn resolve_api_key(config: &Config) -> String {
    if !config.ai.api_key.is_empty() {
        return config.ai.api_key.clone();
    }
    std::env::var(api_key_env_var(&config.ai.provider)).unwrap_or_default()
}

/// Returns `(session_name, newly_created)`.
/// If the daemon was launched from inside an existing tmux session, that
/// session is used and `newly_created` is false.
/// Otherwise the fallback "daemoneye" session is used; `newly_created` is true
/// when this call actually created it (not when it already existed).
fn detect_or_create_session() -> Result<(String, bool)> {
    if std::env::var("TMUX").is_ok() {
        let out = std::process::Command::new("tmux")
            .args(["display-message", "-p", "#S"])
            .output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok((s, false));
            }
        }
    }
    let already_exists = tmux::has_session(FALLBACK_SESSION);
    if !already_exists {
        tmux::create_session(FALLBACK_SESSION)?;
    }
    Ok((FALLBACK_SESSION.to_string(), !already_exists))
}

/// After the daemon socket is bound, open the AI chat pane in the newly
/// created session so the user sees it immediately on `tmux attach`.
async fn open_chat_pane(session_name: String) {
    // Brief pause so the accept loop is running before the chat client connects.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let pane_target = format!("{}:0.0", session_name);

    // Resolve the global pane ID (e.g. %3) of the shell pane so we can pass
    // it as DAEMONEYE_SOURCE_PANE — the pane where commands should be injected.
    let shell_pane_id = match std::process::Command::new("tmux")
        .args(["display-message", "-t", &pane_target, "-p", "#{pane_id}"])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => {
            eprintln!("Warning: could not read shell pane ID: {e}");
            return;
        }
    };

    if shell_pane_id.is_empty() {
        eprintln!("Warning: empty shell pane ID, skipping chat pane setup");
        return;
    }

    // Use the exact binary that is currently running so the path is always
    // correct regardless of how the daemon was invoked.
    let daemon_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    let chat_cmd = format!("{} chat", daemon_bin);

    let result = std::process::Command::new("tmux")
        .args([
            "split-window", "-h",
            "-t", &pane_target,
            "-e", &format!("DAEMONEYE_SOURCE_PANE={}", shell_pane_id),
            &chat_cmd,
        ])
        .output();

    match result {
        Ok(o) if o.status.success() => {
            println!("Chat pane ready. Attach with:  tmux attach -t {}", session_name);
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            eprintln!("Warning: could not open chat pane: {}", err.trim());
            eprintln!("Attach manually with:  tmux attach -t {}  then run `daemoneye chat`", session_name);
        }
        Err(e) => {
            eprintln!("Warning: could not open chat pane: {e}");
        }
    }
}

/// Returns true if a daemon is already listening and responding on the socket.
/// Uses a 2-second timeout so a hung process doesn't block startup.
async fn daemon_is_running() -> bool {
    let Ok(stream) = tokio::net::UnixStream::connect(DEFAULT_SOCKET_PATH).await else {
        return false;
    };
    let (rx_half, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx_half);

    let Ok(mut data) = serde_json::to_vec(&Request::Ping) else {
        return false;
    };
    data.push(b'\n');
    if tx.write_all(&data).await.is_err() {
        return false;
    }

    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(2), rx.read_line(&mut line)).await {
        Ok(Ok(_)) => matches!(serde_json::from_str::<Response>(line.trim()), Ok(Response::Ok)),
        _ => false,
    }
}

/// Start the daemon process.
///
/// Lifecycle:
/// 1. Redirect stdout/stderr to `log_file` (if provided).
/// 2. Validate the configured AI API key; bail immediately if absent.
/// 3. Detect or create a tmux session to monitor.
/// 4. Spawn the pane-cache refresh loop (every 2 s).
/// 5. Bind the Unix domain socket and enter the accept loop.
/// 6. Optionally open the chat pane if the daemon just created the tmux session.
/// 7. Shut down cleanly on SIGTERM or SIGINT.
pub async fn run_daemon(log_file: Option<PathBuf>, command_log: Option<PathBuf>) -> Result<()> {
    if let Some(ref path) = log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("Failed to open log file {}", path.display()))?;
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // Redirect stdout (1) and stderr (2) to the log file.
        // dup2 creates independent FDs 1/2 pointing to the file; `file` can drop safely after.
        unsafe {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
        }
    }
    // Validate API key before binding the socket so the error is immediate
    // and obvious rather than surfacing as a cryptic 401 mid-conversation.
    let startup_config = Config::load().unwrap_or_default();

    // Initialise the masking filter with built-in patterns + any user-defined extras.
    crate::ai::filter::init_masking(&startup_config.masking.extra_patterns);

    if resolve_api_key(&startup_config).is_empty() {
        let env_var = api_key_env_var(&startup_config.ai.provider);
        anyhow::bail!(
            "No API key found for provider '{provider}'.\n\
             Set 'api_key' in ~/.daemoneye/config.toml  or  export {env_var}=<your-key>",
            provider = startup_config.ai.provider,
            env_var = env_var,
        );
    }
    println!("Provider: {} / {}", startup_config.ai.provider, startup_config.ai.model);

    let (session_name, session_was_created) = detect_or_create_session()?;
    println!("Monitoring tmux session: {}", session_name);

    let cache = Arc::new(SessionCache::new(&session_name));

    let cache_monitor = Arc::clone(&cache);
    tokio::spawn(async move {
        loop {
            if let Err(e) = cache_monitor.refresh() {
                eprintln!("Failed to refresh tmux cache: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
    let command_log = Arc::new(command_log);

    // Prune chat sessions idle for more than 30 minutes.
    let sessions_cleanup = Arc::clone(&sessions);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let now = Instant::now();
            sessions_cleanup
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, v| now.duration_since(v.last_accessed) < Duration::from_secs(1800));
        }
    });

    if daemon_is_running().await {
        anyhow::bail!(
            "A daemon is already running on {}.\n\
             Stop it with:  daemoneye stop",
            DEFAULT_SOCKET_PATH,
        );
    }

    let socket_path = Path::new(DEFAULT_SOCKET_PATH);

    if socket_path.exists() {
        std::fs::remove_file(socket_path)
            .context("Failed to remove stale socket file")?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("Failed to bind to socket at {}", DEFAULT_SOCKET_PATH))?;

    println!("Daemon listening on {}", DEFAULT_SOCKET_PATH);

    // If the daemon just created the tmux session, open the chat pane inside
    // it now that the socket is ready. Users can then simply
    // `tmux attach -t daemoneye` and start chatting immediately.
    if session_was_created {
        let sn = session_name.clone();
        tokio::spawn(async move { open_chat_pane(sn).await });
    }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Failed to install SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Failed to install SIGINT handler")?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let cache_conn = Arc::clone(&cache);
                        let sessions_conn = Arc::clone(&sessions);
                        let cmd_log_conn = Arc::clone(&command_log);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, cache_conn, sessions_conn, cmd_log_conn).await {
                                eprintln!("Error handling client: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Failed to accept incoming connection: {}", e);
                    }
                }
            }
            _ = sigterm.recv() => {
                println!("Received SIGTERM, shutting down.");
                break;
            }
            _ = sigint.recv() => {
                println!("Received SIGINT, shutting down.");
                break;
            }
        }
    }

    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

/// Handle one client connection end-to-end.
///
/// ## Request routing
/// - `Ping` / `Shutdown` / `Refresh` are dispatched and returned immediately.
/// - `Ask` drives the full conversation turn: load history → build prompt →
///   stream AI response → collect tool calls → execute each (background or
///   foreground) → loop back for the next AI turn until no tool calls remain.
///
/// ## Tool call execution
/// Each tool call goes through an approval gate:
/// - The client is sent a `ToolCallPrompt`; the user approves or denies.
/// - **Background** (`background: true`): the daemon runs the command as a
///   subprocess (`tokio::process`). If sudo is needed a `CredentialPrompt` is sent
///   and the credential is piped to `sudo -S`.
/// - **Foreground** (`background: false`): `tmux send-keys` dispatches to the
///   user's working pane. If sudo is detected the daemon switches focus to that
///   pane and waits for `pane_current_command` to leave "sudo".
///
/// ## Session persistence
/// Message history is stored both in the in-memory `sessions` map (fast lookup
/// within the same daemon run) and in `~/.daemoneye/sessions/<id>.jsonl` (survives
/// restarts). History is trimmed to `MAX_HISTORY` messages before each save.
async fn handle_client(stream: UnixStream, cache: Arc<SessionCache>, sessions: SessionStore, command_log: Arc<Option<PathBuf>>) -> Result<()> {
    let mut config = Config::load().unwrap_or_else(|_| {
        eprintln!("Warning: failed to load config, using defaults");
        Config {
            ai: crate::config::AiConfig {
                provider: "anthropic".to_string(),
                api_key: String::new(),
                model: "claude-sonnet-4-6".to_string(),
                prompt: "sre".to_string(),
                position: "right".to_string(),
            },
            masking: Default::default(),
            context: Default::default(),
        }
    });
    // If the config file has no key, fall back to the provider's env var.
    if config.ai.api_key.is_empty() {
        config.ai.api_key = std::env::var(api_key_env_var(&config.ai.provider))
            .unwrap_or_default();
    }

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }

    let request: Request = match serde_json::from_str(line.trim()) {
        Ok(req) => req,
        Err(e) => {
            let mut stream = reader.into_inner();
            send_response(&mut stream, Response::Error(format!("Invalid request: {}", e))).await?;
            return Ok(());
        }
    };

    let (rx_half, mut tx) = reader.into_inner().into_split();
    let mut rx = BufReader::new(rx_half);

    let (initial_query, client_pane, session_id, chat_pane, prompt_override, chat_width) = match request {
        Request::Ping => {
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        Request::Shutdown => {
            send_response_split(&mut tx, Response::Ok).await?;
            let socket_path = Path::new(DEFAULT_SOCKET_PATH);
            let _ = std::fs::remove_file(socket_path);
            std::process::exit(0);
        }
        Request::Ask { query, tmux_pane, session_id, chat_pane, prompt, chat_width } => (query, tmux_pane, session_id, chat_pane, prompt, chat_width),
        Request::Refresh => {
            crate::sys_context::refresh_sys_context();
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        _ => return Ok(()),
    };

    // Load existing message history for this session (if any).
    // Fast path: in-memory store (same daemon run).
    // Slow path: file on disk (survives daemon restarts).
    let mut messages: Vec<Message> = session_id
        .as_ref()
        .and_then(|id| {
            let mem = sessions.lock().unwrap_or_else(|e| e.into_inner());
            mem.get(id).map(|e| e.messages.clone())
        })
        .or_else(|| {
            session_id.as_ref().map(|id| read_session_file(id))
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();

    // Trim history to keep the context window bounded.
    // Layout after trim: [messages[0]] [placeholder] [tail...]
    // messages[0] is the first-turn user message containing sys_ctx.
    // The placeholder is a synthetic assistant message so role alternation
    // (user→assistant→user→…) is preserved at the join point.
    // tail_start is snapped to an even index so the tail always starts on a
    // user message, which keeps alternation valid regardless of how many
    // messages are dropped.
    messages = trim_history(messages);

    let is_first_turn = messages.is_empty();

    // Build labeled terminal context: active pane at full depth, background panes as summaries.
    let session_summary = cache.get_labeled_context(client_pane.as_deref());
    let safe_query = mask_sensitive(&initial_query);

    // First turn: include full host context. Subsequent turns: fresh terminal
    // snapshot only (sys_ctx is already in the conversation history).
    let prompt = if is_first_turn {
        let sys_ctx = get_or_init_sys_context().format_for_ai();
        let daemon_host = daemon_hostname();
        let environment = &config.context.environment;
        let pane_location = client_pane.as_deref()
            .and_then(get_pane_remote_host)
            .map(|h| format!("REMOTE — {}", h))
            .unwrap_or_else(|| format!("LOCAL — same host as daemon ({})", daemon_host));
        let width_hint = chat_width
            .map(|w| format!("\n- Chat display width: {w} columns (write prose as continuous paragraphs; the terminal word-wraps automatically — do not insert hard line breaks within paragraphs)"))
            .unwrap_or_default();
        format!(
            "## Host Context\n```\n{sys_ctx}\n```\n\n\
             ## Execution Context\n\
             - Environment: {environment}\n\
             - Daemon host: {daemon_host}\n\
             - User's terminal pane: {pane_location}\
             {width_hint}\n\
             - background=true  → runs on DAEMON HOST ({daemon_host})\n\
             - background=false → runs in USER'S PANE ({pane_location})\n\n\
             ## Terminal Session\n```\n{session_summary}\n```\n\n\
             User: {safe_query}"
        )
    } else {
        format!(
            "## Terminal Session (updated)\n```\n{session_summary}\n```\n\n\
             User: {safe_query}"
        )
    };

    let prompt_name = prompt_override.as_deref().unwrap_or(&config.ai.prompt);
    let sys_prompt = load_named_prompt(prompt_name).system;

    let history_count = messages.len();
    messages.push(Message {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_results: None,
    });

    send_response_split(&mut tx, Response::SessionInfo { message_count: history_count }).await?;

    loop {
        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        let client_instance = make_client(&config.ai.provider, config.ai.api_key.clone(), config.ai.model.clone());
        let sys_prompt_turn = sys_prompt.clone();
        let messages_clone = messages.clone();
        
        tokio::spawn(async move {
            if let Err(e) = client_instance.chat(&sys_prompt_turn, messages_clone, ai_tx.clone()).await {
                let _ = ai_tx.send(AiEvent::Error(e.to_string()));
            }
        });

        let mut full_response = String::new();
        // Collect all tool calls emitted in this turn before processing any.
        let mut pending_calls: Vec<(String, String, bool)> = Vec::new(); // (id, cmd, bg)

        while let Some(event) = ai_rx.recv().await {
            match event {
                AiEvent::Token(t) => {
                    full_response.push_str(&t);
                    send_response_split(&mut tx, Response::Token(t)).await?;
                }
                AiEvent::ToolCall(id, cmd, bg) => {
                    pending_calls.push((id, cmd, bg));
                }
                AiEvent::Error(e) => {
                    send_response_split(&mut tx, Response::Error(e)).await?;
                    return Ok(());
                }
                AiEvent::Done => {
                    if pending_calls.is_empty() {
                        // No tool calls — this is the final answer.
                        if !full_response.is_empty() {
                            messages.push(Message {
                                role: "assistant".to_string(),
                                content: full_response.clone(),
                                tool_calls: None,
                                tool_results: None,
                            });
                        }
                        // Persist the conversation for the next turn.
                        // In-memory: fast lookup within the same daemon run.
                        // On-disk: survives daemon restarts.
                        if let Some(ref id) = session_id {
                            if let Ok(mut store) = sessions.lock() {
                                store.insert(id.clone(), SessionEntry {
                                    messages: messages.clone(),
                                    last_accessed: Instant::now(),
                                });
                            }
                            write_session_file(id, &messages);
                        }
                        send_response_split(&mut tx, Response::Ok).await?;
                        return Ok(());
                    }

                    // One or more tool calls — push a single assistant message
                    // listing all of them, then execute each sequentially.
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: full_response.clone(),
                        tool_calls: Some(pending_calls.iter().map(|(id, cmd, bg)| ToolCall {
                            id: id.clone(),
                            name: "run_terminal_command".to_string(),
                            arguments: serde_json::json!({"command": cmd, "background": bg}).to_string(),
                        }).collect()),
                        tool_results: None,
                    });

                    // --- Tool call execution loop ---
                    // Each pending call goes through:
                    //   1. Approval gate  (client sends ToolCallResponse)
                    //   2. Execution      (background subprocess OR foreground tmux send-keys)
                    //   3. Result capture (output appended to tool_results for the next AI turn)
                    let mut tool_results = Vec::new();
                    for (id, cmd, bg) in &pending_calls {
                        send_response_split(&mut tx, Response::ToolCallPrompt {
                            id: id.clone(),
                            command: cmd.clone(),
                            background: *bg,
                        }).await?;

                        let mut line = String::new();
                        // Give the user 60 s to approve or deny. Timeout and IO errors
                        // are treated as denial so message history stays valid.
                        let read_result = tokio::time::timeout(
                            Duration::from_secs(60),
                            rx.read_line(&mut line),
                        ).await;

                        if matches!(read_result, Ok(Ok(0))) {
                            // Client disconnected cleanly; nothing left to do.
                            return Ok(());
                        }

                        let timed_out = read_result.is_err();
                        let approved = match read_result {
                            Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                                Ok(Request::ToolCallResponse { id: resp_id, approved }) if resp_id == *id => approved,
                                _ => false,
                            },
                            _ => false,
                        };

                        let output = if approved {
                            if *bg {
                                // Background commands run in a daemon subprocess: output
                                // is captured and returned to the AI invisibly.
                                let needs_sudo = command_has_sudo(cmd);
                                let (exec_cmd, opt_password) = if needs_sudo {
                                    // Ask the client for the credential before running.
                                    send_response_split(&mut tx, Response::CredentialPrompt {
                                        id: id.clone(),
                                        prompt: format!("[sudo] password required for: {}", cmd),
                                    }).await?;

                                    let mut cred_line = String::new();
                                    let credential = match tokio::time::timeout(
                                        Duration::from_secs(120),
                                        rx.read_line(&mut cred_line),
                                    ).await {
                                        Ok(Ok(_)) => match serde_json::from_str::<Request>(cred_line.trim()) {
                                            Ok(Request::CredentialResponse { credential, .. }) => credential,
                                            _ => String::new(),
                                        },
                                        _ => String::new(),
                                    };
                                    (inject_sudo_flags(cmd), Some(credential))
                                } else {
                                    (cmd.clone(), None)
                                };

                                use std::process::Stdio;

                                const BG_CMD_TIMEOUT: Duration = Duration::from_secs(30);

                                let mut proc = tokio::process::Command::new("sh");
                                proc.args(["-c", &exec_cmd])
                                    .stdout(Stdio::piped())
                                    .stderr(Stdio::piped());
                                if opt_password.is_some() {
                                    proc.stdin(Stdio::piped());
                                } else {
                                    proc.stdin(Stdio::null());
                                }

                                // Apply resource limits in the child before exec.
                                unsafe {
                                    proc.pre_exec(|| {
                                        // Virtual address space: 512 MiB
                                        let mem = libc::rlimit {
                                            rlim_cur: 512 * 1024 * 1024,
                                            rlim_max: 512 * 1024 * 1024,
                                        };
                                        libc::setrlimit(libc::RLIMIT_AS, &mem);
                                        // Open file descriptors: 256
                                        let fds = libc::rlimit { rlim_cur: 256, rlim_max: 256 };
                                        libc::setrlimit(libc::RLIMIT_NOFILE, &fds);
                                        Ok(())
                                    });
                                }

                                let result = match proc.spawn() {
                                    Ok(mut child) => {
                                        if let Some(ref password) = opt_password {
                                            if let Some(mut stdin) = child.stdin.take() {
                                                let _ = stdin.write_all(
                                                    format!("{}\n", password).as_bytes()
                                                ).await;
                                            }
                                        }
                                        // Drain stdout/stderr in background tasks so we can
                                        // independently time-out the wait() call and kill if needed.
                                        let stdout_handle = child.stdout.take();
                                        let stderr_handle = child.stderr.take();
                                        let out_task = tokio::spawn(async move {
                                            let mut v = Vec::new();
                                            if let Some(mut s) = stdout_handle {
                                                let _ = s.read_to_end(&mut v).await;
                                            }
                                            v
                                        });
                                        let err_task = tokio::spawn(async move {
                                            let mut v = Vec::new();
                                            if let Some(mut s) = stderr_handle {
                                                let _ = s.read_to_end(&mut v).await;
                                            }
                                            v
                                        });
                                        match tokio::time::timeout(BG_CMD_TIMEOUT, child.wait()).await {
                                            Ok(Ok(status)) => {
                                                let out = out_task.await.unwrap_or_default();
                                                let err = err_task.await.unwrap_or_default();
                                                let combined = format!(
                                                    "{}{}",
                                                    String::from_utf8_lossy(&out),
                                                    String::from_utf8_lossy(&err),
                                                );
                                                let r = normalize_output(&combined);
                                                let body = if r.is_empty() { "(no output)".to_string() } else { mask_sensitive(&r) };
                                                if status.success() {
                                                    body
                                                } else {
                                                    let code = status.code().unwrap_or(-1);
                                                    format!("exit {} · {}\n--- output ---\n{}", code, classify_exit_code(code), body)
                                                }
                                            }
                                            Ok(Err(e)) => format!("Failed to run command: {}", e),
                                            Err(_elapsed) => {
                                                let _ = child.kill().await;
                                                out_task.abort();
                                                err_task.abort();
                                                format!("Command timed out after {} s and was killed", BG_CMD_TIMEOUT.as_secs())
                                            }
                                        }
                                    }
                                    Err(e) => format!("Failed to spawn command: {}", e),
                                };
                                log_command(command_log.as_ref().as_deref(), session_id.as_deref(), "background", "", cmd, "approved", &result);
                                result
                            } else {
                                // Foreground commands are injected into the user's working pane
                                // via tmux send-keys so they are visible and interactive.
                                let active_pane_fallback = cache.active_pane.read().unwrap_or_else(|e| e.into_inner()).clone();
                                let target = client_pane.as_deref()
                                    .or_else(|| active_pane_fallback.as_deref())
                                    .unwrap_or("");
                                if !target.is_empty() {
                                    // Snapshot the shell name before sending keys so we
                                    // can detect when the command finishes (pane_current_command
                                    // returns to the idle shell value).
                                    // Snapshot the shell's PID before injecting the command
                                    // so we can watch /proc for child processes appearing
                                    // and disappearing — giving low-latency completion
                                    // detection without modifying the visible command text.
                                    let shell_pid = tmux::pane_pid(target).ok();
                                    let idle_cmd = tmux::pane_current_command(target)
                                        .unwrap_or_default();
                                    match tmux::send_keys(target, cmd) {
                                        Ok(()) => {
                                            // Track whether we switched the user to their working
                                            // pane so we know to switch back afterward.
                                            let mut switched_to_working = false;

                                            if command_has_sudo(cmd) {
                                                // Poll the pane at 100 ms intervals to detect a
                                                // sudo password prompt.  Exit early if:
                                                //   - "sudo" is the foreground process (prompt found)
                                                //   - the shell is already idle (NOPASSWD / cached)
                                                //   - 3 s have elapsed without a prompt
                                                let poll = Duration::from_millis(100);
                                                let mut waited = Duration::ZERO;
                                                let prompt_timeout = Duration::from_secs(3);
                                                let needs_password = loop {
                                                    tokio::time::sleep(poll).await;
                                                    waited += poll;
                                                    let cur = tmux::pane_current_command(target)
                                                        .unwrap_or_default();
                                                    if cur == "sudo"   { break true;  }
                                                    if cur == idle_cmd { break false; }
                                                    if waited >= prompt_timeout { break false; }
                                                };

                                                if needs_password {
                                                    send_response_split(&mut tx, Response::SystemMsg(
                                                        "sudo password prompt detected — \
                                                         switching to your terminal pane. \
                                                         Type your password there.".to_string()
                                                    )).await?;
                                                    let _ = tmux::select_pane(target);
                                                    switched_to_working = true;
                                                }
                                            }

                                            // Detect command completion via /proc child-process
                                            // tracking.  When the shell forks to run the command a
                                            // child PID appears in:
                                            //   /proc/<shell_pid>/task/<shell_pid>/children
                                            // and disappears when that child exits.  This lets us
                                            // detect completion within one 20 ms tick without
                                            // appending anything to the visible command text.
                                            //
                                            // Fallback: if /proc is unavailable or the command
                                            // completed before the first poll (e.g. on a very fast
                                            // `ls`), we exit once the shell is idle and we have
                                            // waited at least 100 ms to rule out a startup race.
                                            let cmd_timeout = Duration::from_secs(30);
                                            let poll = Duration::from_millis(20);
                                            let mut waited = Duration::ZERO;
                                            let mut saw_child = false;
                                            loop {
                                                tokio::time::sleep(poll).await;
                                                waited += poll;

                                                let has_child = shell_pid.map(|pid| {
                                                    std::fs::read_to_string(format!(
                                                        "/proc/{}/task/{}/children", pid, pid
                                                    ))
                                                    .map(|s| !s.trim().is_empty())
                                                    .unwrap_or(false)
                                                }).unwrap_or(false);

                                                if has_child { saw_child = true; }

                                                let back = tmux::pane_current_command(target)
                                                    .map(|c| c == idle_cmd)
                                                    .unwrap_or(true);

                                                // Normal path: child came and went, shell is idle.
                                                // Fallback: never saw a child (fast command or
                                                //   /proc unavailable) — exit once idle and the
                                                //   startup-race grace period has elapsed.
                                                let done = (saw_child && !has_child && back)
                                                    || (!saw_child && back && waited >= Duration::from_millis(100))
                                                    || waited >= cmd_timeout;
                                                if done { break; }
                                            }
                                            // Brief settle: lets the shell draw its prompt before
                                            // capture_pane so the extracted output is complete.
                                            tokio::time::sleep(Duration::from_millis(50)).await;

                                            let result = match tmux::capture_pane(target, 200) {
                                                Ok(snap) => {
                                                    let extracted = extract_command_output(&snap, cmd);
                                                    mask_sensitive(&normalize_output(&extracted))
                                                }
                                                Err(_) => "Command sent but could not capture output.".to_string(),
                                            };

                                            // Return focus to the AI chat pane now that the
                                            // command is done and the user typed their password.
                                            if switched_to_working {
                                                if let Some(ref cp) = chat_pane {
                                                    let _ = tmux::select_pane(cp);
                                                }
                                            }

                                            log_command(command_log.as_ref().as_deref(), session_id.as_deref(), "foreground", target, cmd, "approved", &result);
                                            result
                                        }
                                        Err(e) => {
                                            let msg = format!("Failed to send command: {}", e);
                                            log_command(command_log.as_ref().as_deref(), session_id.as_deref(), "foreground", target, cmd, "send-failed", &msg);
                                            msg
                                        }
                                    }
                                } else {
                                    "No active pane found.".to_string()
                                }
                            }
                        } else if timed_out {
                            log_command(command_log.as_ref().as_deref(), session_id.as_deref(), if *bg { "background" } else { "foreground" }, "", cmd, "timeout", "");
                            "Approval timed out (60 s); command not executed.".to_string()
                        } else {
                            log_command(command_log.as_ref().as_deref(), session_id.as_deref(), if *bg { "background" } else { "foreground" }, "", cmd, "denied", "");
                            "User denied execution".to_string()
                        };

                        // Show the command output to the user before the AI continues.
                        if approved {
                            send_response_split(&mut tx, Response::ToolResult(output.clone())).await?;
                        }
                        tool_results.push(ToolResult { tool_call_id: id.clone(), content: output });
                    }

                    // Push one message with all results so message history is valid.
                    messages.push(Message {
                        role: "user".to_string(),
                        content: String::new(),
                        tool_calls: None,
                        tool_results: Some(tool_results),
                    });
                    break; // break inner loop; outer loop makes the next AI call
                }
            }
        }
        
    }
}

async fn send_response(stream: &mut UnixStream, response: Response) -> Result<()> {
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    stream.write_all(&data).await?;
    Ok(())
}

async fn send_response_split(tx: &mut tokio::net::unix::OwnedWriteHalf, response: Response) -> Result<()> {
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::client::Message;

    // ── normalize_output ─────────────────────────────────────────────────────

    #[test]
    fn normalize_basic() {
        assert_eq!(normalize_output("hello"), "hello");
    }

    #[test]
    fn normalize_trims_trailing_whitespace_per_line() {
        let input = "line one   \nline two  \nline three";
        let out = normalize_output(input);
        assert_eq!(out, "line one\nline two\nline three");
    }

    #[test]
    fn normalize_strips_leading_blank_lines() {
        let input = "\n\n\nhello\nworld";
        assert_eq!(normalize_output(input), "hello\nworld");
    }

    #[test]
    fn normalize_strips_trailing_blank_lines() {
        let input = "hello\nworld\n\n\n";
        assert_eq!(normalize_output(input), "hello\nworld");
    }

    #[test]
    fn normalize_all_blank_returns_empty() {
        assert_eq!(normalize_output("   \n  \n   "), "");
    }

    #[test]
    fn normalize_empty_input_returns_empty() {
        assert_eq!(normalize_output(""), "");
    }

    #[test]
    fn normalize_preserves_internal_blank_lines() {
        let input = "a\n\nb\n\nc";
        assert_eq!(normalize_output(input), "a\n\nb\n\nc");
    }

    // ── classify_exit_code ───────────────────────────────────────────────────

    #[test]
    fn classify_known_codes() {
        assert_eq!(classify_exit_code(1),   "generic failure");
        assert_eq!(classify_exit_code(2),   "misuse of shell built-in");
        assert_eq!(classify_exit_code(126), "permission denied (not executable)");
        assert_eq!(classify_exit_code(127), "command not found");
        assert_eq!(classify_exit_code(128), "invalid exit argument");
        assert_eq!(classify_exit_code(130), "interrupted (Ctrl-C)");
        assert_eq!(classify_exit_code(137), "killed (SIGKILL / OOM)");
        assert_eq!(classify_exit_code(143), "terminated (SIGTERM)");
    }

    #[test]
    fn classify_unknown_code_returns_generic() {
        assert_eq!(classify_exit_code(42),  "non-zero exit");
        assert_eq!(classify_exit_code(255), "non-zero exit");
    }

    // ── inject_sudo_flags ────────────────────────────────────────────────────

    #[test]
    fn inject_sudo_flags_rewrites_sudo_prefix() {
        let out = inject_sudo_flags("sudo apt update");
        assert!(out.starts_with("sudo -S -p \"\""));
        assert!(out.contains("apt update"));
    }

    #[test]
    fn inject_sudo_flags_leaves_non_sudo_unchanged() {
        let cmd = "ls -la /etc";
        assert_eq!(inject_sudo_flags(cmd), cmd);
    }

    #[test]
    fn inject_sudo_flags_trims_leading_whitespace() {
        let out = inject_sudo_flags("  sudo reboot");
        assert!(out.starts_with("sudo -S -p \"\""));
    }

    // ── command_has_sudo ─────────────────────────────────────────────────────

    #[test]
    fn command_has_sudo_simple() {
        assert!(command_has_sudo("sudo apt install vim"));
    }

    #[test]
    fn command_has_sudo_in_pipeline() {
        assert!(command_has_sudo("echo hi | sudo tee /etc/hosts"));
    }

    #[test]
    fn command_has_sudo_after_semicolon() {
        assert!(command_has_sudo("cd /tmp; sudo rm -rf foo"));
    }

    #[test]
    fn command_has_sudo_false_positive_guard() {
        // "sudoer" is not "sudo" — word-boundary check must hold.
        assert!(!command_has_sudo("cat /etc/sudoers"));
    }

    #[test]
    fn command_has_sudo_no_sudo() {
        assert!(!command_has_sudo("ls -la /home"));
    }

    // ── trim_history ─────────────────────────────────────────────────────────

    fn make_msg(role: &str, content: &str) -> Message {
        Message { role: role.to_string(), content: content.to_string(), tool_calls: None, tool_results: None }
    }

    fn make_history(n: usize) -> Vec<Message> {
        (0..n).map(|i| make_msg(if i % 2 == 0 { "user" } else { "assistant" }, &format!("msg {i}"))).collect()
    }

    #[test]
    fn trim_history_unchanged_when_under_limit() {
        let msgs = make_history(10);
        let out = trim_history(msgs.clone());
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].content, "msg 0");
    }

    #[test]
    fn trim_history_at_exact_limit_unchanged() {
        let msgs = make_history(MAX_HISTORY);
        let out = trim_history(msgs);
        assert_eq!(out.len(), MAX_HISTORY);
    }

    #[test]
    fn trim_history_over_limit_bounded() {
        let msgs = make_history(MAX_HISTORY + 10);
        let out = trim_history(msgs);
        assert!(out.len() <= MAX_HISTORY);
    }

    #[test]
    fn trim_history_preserves_first_message() {
        let msgs = make_history(MAX_HISTORY + 5);
        let out = trim_history(msgs);
        assert_eq!(out[0].content, "msg 0");
    }

    #[test]
    fn trim_history_placeholder_is_assistant() {
        let msgs = make_history(MAX_HISTORY + 5);
        let out = trim_history(msgs);
        // position 1 is the placeholder
        assert_eq!(out[1].role, "assistant");
        assert!(out[1].content.contains("trimmed"));
    }

    #[test]
    fn trim_history_tail_starts_on_user_turn() {
        // After [first, placeholder], the next message must be a user message
        // so the user→assistant alternation is valid.
        let msgs = make_history(MAX_HISTORY + 5);
        let out = trim_history(msgs);
        assert_eq!(out[2].role, "user", "tail must start on a user message");
    }

    // ── session file round-trip ───────────────────────────────────────────────

    #[test]
    fn session_file_roundtrip() {
        // Write messages to a temp session file and read them back.
        let id = format!("test_{}", std::process::id());
        // Temporarily point sessions_dir() at /tmp to avoid HOME dependency.
        // We call the helpers directly using /tmp as the base.
        let dir = std::path::PathBuf::from("/tmp");
        let path = dir.join(format!("{}.jsonl", id));

        let msgs = vec![
            make_msg("user", "hello"),
            make_msg("assistant", "hi there"),
        ];

        // Replicate write_session_file logic with a known path.
        use std::io::Write;
        let mut f = std::fs::File::create(&path).unwrap();
        for m in &msgs {
            writeln!(f, "{}", serde_json::to_string(m).unwrap()).unwrap();
        }

        // Replicate read_session_file logic with the same path.
        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Vec<Message> = text.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "hello");
        assert_eq!(loaded[1].role, "assistant");

        let _ = std::fs::remove_file(&path);
    }

    // ── extract_command_output ───────────────────────────────────────────────

    fn pane_snap(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn extract_finds_command_line_by_suffix() {
        let snap = pane_snap(&[
            "matt@host:~$ ls",
            "file1  file2",
            "matt@host:~$ cat README.md",
            "# DaemonEye",
            "An AI-powered operator.",
            "matt@host:~$ ",
        ]);
        let result = extract_command_output(&snap, "cat README.md");
        assert!(result.starts_with("matt@host:~$ cat README.md"),
            "first line should be the prompt+command, got: {:?}", &result[..result.find('\n').unwrap_or(result.len())]);
        assert!(result.contains("# DaemonEye"));
    }

    #[test]
    fn extract_uses_rposition_to_pick_most_recent_invocation() {
        // The command appeared earlier in history — we want the most recent one.
        let snap = pane_snap(&[
            "matt@host:~$ ls -la",
            "old output line",
            "matt@host:~$ echo hi",
            "hi",
            "matt@host:~$ ls -la",
            "newer output",
            "matt@host:~$ ",
        ]);
        let result = extract_command_output(&snap, "ls -la");
        // Should start from the SECOND "ls -la" invocation, not the first.
        assert_eq!(result.lines().next().unwrap(), "matt@host:~$ ls -la");
        assert!(result.contains("newer output"));
        assert!(!result.contains("old output line"));
    }

    #[test]
    fn extract_fallback_when_cmd_not_found() {
        // Command string doesn't appear as a suffix anywhere — use last 50 lines.
        let mut lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
        lines.push("final line".to_string());
        let snap = lines.join("\n");
        let result = extract_command_output(&snap, "mystery_cmd_xyz");
        // Should contain the tail, not the beginning.
        assert!(result.contains("final line"));
        assert!(!result.contains("line 0"));
    }

    #[test]
    fn extract_empty_snap_returns_empty() {
        assert_eq!(extract_command_output("", "ls"), "");
    }
}
