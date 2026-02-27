use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use std::time::Duration;

use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::client::{make_client, AiEvent, Message, ToolCall, ToolResult};
use crate::ai::filter::mask_sensitive;
use crate::config::{Config, load_named_prompt};
use crate::sys_context::get_or_init_sys_context;

struct SessionEntry {
    messages: Vec<Message>,
    last_accessed: Instant,
}

type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;

const FALLBACK_SESSION: &str = "t1000";

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
/// Otherwise the fallback "t1000" session is used; `newly_created` is true
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
    // it as T1000_SOURCE_PANE — the pane where commands should be injected.
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
    let t1000_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "t1000".to_string());

    let chat_cmd = format!("{} chat", t1000_bin);

    let result = std::process::Command::new("tmux")
        .args([
            "split-window", "-h",
            "-t", &pane_target,
            "-e", &format!("T1000_SOURCE_PANE={}", shell_pane_id),
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
            eprintln!("Attach manually with:  tmux attach -t {}  then run `t1000 chat`", session_name);
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
    if resolve_api_key(&startup_config).is_empty() {
        let env_var = api_key_env_var(&startup_config.ai.provider);
        anyhow::bail!(
            "No API key found for provider '{provider}'.\n\
             Set 'api_key' in ~/.t1000/config.toml  or  export {env_var}=<your-key>",
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
                .unwrap()
                .retain(|_, v| now.duration_since(v.last_accessed) < Duration::from_secs(1800));
        }
    });

    if daemon_is_running().await {
        anyhow::bail!(
            "A daemon is already running on {}.\n\
             Stop it with:  t1000 stop",
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
    // `tmux attach -t t1000` and start chatting immediately.
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

    let (initial_query, client_pane, session_id) = match request {
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
        Request::Ask { query, tmux_pane, session_id } => (query, tmux_pane, session_id),
        _ => return Ok(()),
    };

    // Load existing message history for this session (if any).
    let mut messages: Vec<Message> = session_id
        .as_ref()
        .and_then(|id| sessions.lock().unwrap().get(id).map(|e| e.messages.clone()))
        .unwrap_or_default();

    // Trim history to keep the context window bounded.
    // Layout after trim: [messages[0]] [placeholder] [tail...]
    // messages[0] is the first-turn user message containing sys_ctx.
    // The placeholder is a synthetic assistant message so role alternation
    // (user→assistant→user→…) is preserved at the join point.
    // tail_start is snapped to an even index so the tail always starts on a
    // user message, which keeps alternation valid regardless of how many
    // messages are dropped.
    const MAX_HISTORY: usize = 40;
    if messages.len() > MAX_HISTORY {
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
        messages = trimmed;
    }

    let is_first_turn = messages.is_empty();

    let pane_context = if let Some(ref pane_id) = client_pane {
        tmux::capture_pane(pane_id, 200).unwrap_or_else(|_| cache.get_context_summary())
    } else {
        cache.get_context_summary()
    };

    let session_summary = mask_sensitive(&pane_context);
    let safe_query = mask_sensitive(&initial_query);

    // First turn: include full host context. Subsequent turns: fresh terminal
    // snapshot only (sys_ctx is already in the conversation history).
    let prompt = if is_first_turn {
        let sys_ctx = get_or_init_sys_context().format_for_ai();
        let daemon_host = daemon_hostname();
        let pane_location = client_pane.as_deref()
            .and_then(get_pane_remote_host)
            .map(|h| format!("REMOTE — {}", h))
            .unwrap_or_else(|| format!("LOCAL — same host as daemon ({})", daemon_host));
        format!(
            "## Host Context\n```\n{sys_ctx}\n```\n\n\
             ## Execution Context\n\
             - Daemon host: {daemon_host}\n\
             - User's terminal pane: {pane_location}\n\
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

    let sys_prompt = load_named_prompt(&config.ai.prompt).system;

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
                        if let Some(ref id) = session_id {
                            if let Ok(mut store) = sessions.lock() {
                                store.insert(id.clone(), SessionEntry {
                                    messages: messages.clone(),
                                    last_accessed: Instant::now(),
                                });
                            }
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
                                    // Ask the client for the sudo password before running.
                                    send_response_split(&mut tx, Response::SudoPrompt {
                                        id: id.clone(),
                                        command: cmd.clone(),
                                    }).await?;

                                    let mut sudo_line = String::new();
                                    let password = match tokio::time::timeout(
                                        Duration::from_secs(30),
                                        rx.read_line(&mut sudo_line),
                                    ).await {
                                        Ok(Ok(_)) => match serde_json::from_str::<Request>(sudo_line.trim()) {
                                            Ok(Request::SudoPassword { password, .. }) => password,
                                            _ => String::new(),
                                        },
                                        _ => String::new(),
                                    };
                                    (inject_sudo_flags(cmd), Some(password))
                                } else {
                                    (cmd.clone(), None)
                                };

                                use std::process::Stdio;
                                let mut proc = tokio::process::Command::new("sh");
                                proc.args(["-c", &exec_cmd])
                                    .stdout(Stdio::piped())
                                    .stderr(Stdio::piped());
                                if opt_password.is_some() {
                                    proc.stdin(Stdio::piped());
                                } else {
                                    proc.stdin(Stdio::null());
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
                                        match child.wait_with_output().await {
                                            Ok(out) => {
                                                let stdout = String::from_utf8_lossy(&out.stdout);
                                                let stderr = String::from_utf8_lossy(&out.stderr);
                                                let combined = format!("{}{}", stdout, stderr);
                                                let r = combined.trim().to_string();
                                                if r.is_empty() { "(no output)".to_string() } else { mask_sensitive(&r) }
                                            }
                                            Err(e) => format!("Failed to run command: {}", e),
                                        }
                                    }
                                    Err(e) => format!("Failed to spawn command: {}", e),
                                };
                                log_command(command_log.as_ref().as_deref(), session_id.as_deref(), "background", "", cmd, "approved", &result);
                                result
                            } else {
                                // Foreground commands are injected into the user's working pane
                                // via tmux send-keys so they are visible and interactive.
                                let active_pane_fallback = cache.active_pane.read().unwrap().clone();
                                let target = client_pane.as_deref()
                                    .or_else(|| active_pane_fallback.as_deref())
                                    .unwrap_or("");
                                if !target.is_empty() {
                                    match tmux::send_keys(target, cmd) {
                                        Ok(()) => {
                                            // If sudo is involved, notify the user via the chat
                                            // interface so they know to type their password in the
                                            // terminal pane, then wait longer for the command.
                                            let wait_secs = if command_has_sudo(cmd) {
                                                send_response_split(&mut tx, Response::Token(
                                                    "\n[sudo] This command requires elevated privileges. \
                                                     Please type your password in the terminal pane.\n".to_string()
                                                )).await?;
                                                30u64
                                            } else {
                                                3u64
                                            };
                                            tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                                            let result = match tmux::capture_pane(target, 200) {
                                                Ok(out) => mask_sensitive(&out),
                                                Err(_) => "Command sent but could not capture output.".to_string(),
                                            };
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
