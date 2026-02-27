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

/// Returns the tmux session the daemon should monitor.
/// If launched from inside a tmux session, uses that session.
/// Otherwise creates/attaches the fallback "t1000" session.
fn detect_or_create_session() -> Result<String> {
    if std::env::var("TMUX").is_ok() {
        let out = std::process::Command::new("tmux")
            .args(["display-message", "-p", "#S"])
            .output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(s);
            }
        }
    }
    if !tmux::has_session(FALLBACK_SESSION) {
        tmux::create_session(FALLBACK_SESSION)?;
    }
    Ok(FALLBACK_SESSION.to_string())
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

pub async fn run_daemon(log_file: Option<PathBuf>) -> Result<()> {
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

    let session_name = detect_or_create_session()?;
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
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, cache_conn, sessions_conn).await {
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

async fn handle_client(stream: UnixStream, cache: Arc<SessionCache>, sessions: SessionStore) -> Result<()> {
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
        format!(
            "## Host Context\n```\n{sys_ctx}\n```\n\n\
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
                            let active_pane_fallback = cache.active_pane.read().unwrap().clone();
                            let target = client_pane.as_deref()
                                .or_else(|| active_pane_fallback.as_deref())
                                .unwrap_or("");
                            if !target.is_empty() {
                                let cmd_to_run = if *bg { format!("{} &", cmd) } else { cmd.clone() };
                                match tmux::send_keys(target, &cmd_to_run) {
                                    Ok(()) => {
                                        let delay = if *bg {
                                            Duration::from_millis(500)
                                        } else {
                                            Duration::from_secs(3)
                                        };
                                        tokio::time::sleep(delay).await;
                                        match tmux::capture_pane(target, 200) {
                                            Ok(out) => mask_sensitive(&out),
                                            Err(_) => "Command sent but could not capture output.".to_string(),
                                        }
                                    }
                                    Err(e) => format!("Failed to send command: {}", e),
                                }
                            } else {
                                "No active pane found.".to_string()
                            }
                        } else if timed_out {
                            "Approval timed out (60 s); command not executed.".to_string()
                        } else {
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
