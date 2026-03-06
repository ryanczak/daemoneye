
use crate::daemon::session::*;
use crate::daemon::utils::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use std::time::Duration;
use crate::ipc::{PaneInfo, Request, Response, ScheduleListItem, ScriptListItem, DEFAULT_SOCKET_PATH};
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::{make_client, next_tool_id, AiEvent, Message, ToolCall, ToolResult};
use crate::ai::filter::mask_sensitive;
use crate::config::{Config, load_named_prompt};
use crate::sys_context::get_or_init_sys_context;
use crate::scheduler::{ActionOn, ScheduleKind, ScheduledJob, ScheduleStore};
use crate::runbook;
use crate::scripts;

/// Poll a tmux pane until its output is marked dead or the timeout expires.
/// Returns `Some(exit_code)` if the pane died, or `None` if it timed out.
#[allow(dead_code)]
pub async fn poll_until_dead(pane_id: &str, timeout: Duration) -> Option<i32> {
    let poll = Duration::from_millis(200);
    let mut waited = Duration::ZERO;
    loop {
        tokio::time::sleep(poll).await;
        waited += poll;
        
        if let Some(exit_code) = tmux::pane_dead_status(pane_id) {
            return Some(exit_code);
        }
        
        if waited >= timeout {
            return None;
        }
    }
}

/// Run a command in a dedicated tmux window (`de-bg-<id_short>`) on the daemon host.
///
/// The window is always killed after the output is captured.
/// If the command contains sudo and a `credential` is provided, it is injected
/// into the window after the sudo password prompt is detected.
pub async fn run_background_in_window(
    session: &str,
    tool_id: &str,
    cmd: &str,
    credential: Option<&str>,
    session_id: Option<String>,
    sessions: SessionStore,
) -> String {
    let id_short = &tool_id[..tool_id.len().min(8)];
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let win_name = format!("de-bg-{}-{}-{}", session, now, id_short);
    let wrapped = format!("{}; exit $?", cmd);

    let pane_id = match tmux::create_job_window(session, &win_name) {
        Ok(p) => p,
        Err(e) => return format!("Failed to create background window: {}", e),
    };
    
    let started_at = tokio::time::Instant::now();
    let pane_id_log = pane_id.clone();
    
    // P7: keep the pane alive in a '<dead>' state so we can query pane_dead_status.
    let _ = tmux::set_remain_on_exit(&pane_id, true);

    if let Err(e) = tmux::send_keys(&pane_id, &wrapped) {
        let _ = tmux::kill_job_window(session, &win_name);
        return format!("Failed to send command to window: {}", e);
    }

    // If sudo is expected, watch for the password prompt and inject the credential.
    if let Some(cred) = credential {
        let poll = Duration::from_millis(200);
        let prompt_timeout = Duration::from_secs(10);
        let mut waited = Duration::ZERO;
        loop {
            tokio::time::sleep(poll).await;
            waited += poll;
            let snap = tmux::capture_pane(&pane_id, 50).unwrap_or_default();
            // Common sudo prompt patterns
            let has_prompt = snap.contains("password") || snap.contains("Password") || snap.contains("[sudo]");
            if has_prompt {
                let _ = tmux::send_keys(&pane_id, cred);
                break;
            }
            if waited >= prompt_timeout || tmux::pane_dead_status(&pane_id).is_some() {
                break;
            }
        }
    }

    let session_owned = session.to_string();
    let win_owned = win_name.clone();
    let cmd_owned = cmd.to_string();

    let session_id_clone = session_id.clone();
    tokio::spawn(async move {
        let mut rx = bg_done_subscribe();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3600); // 1 hour max
        
        let exit_code = loop {
            if let Some(code) = tmux::pane_dead_status(&pane_id) {
                break code;
            }
            if tokio::time::Instant::now() >= deadline {
                break 124;
            }
            tokio::select! {
                result = rx.recv() => {
                    if let Ok(notified_pane) = result {
                        if notified_pane == pane_id {
                            if let Some(code) = tmux::pane_dead_status(&pane_id) {
                                break code;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    break 124;
                }
            }
        };

        notify_job_completion(
            pane_id, cmd_owned, win_owned, session_owned, exit_code,
            session_id_clone, sessions, None, started_at
        ).await;
    });

    log_event("job_start", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "job_id": id_short,
        "job_name": win_name,
        "pane": pane_id_log,
    }));

    format!("Started background command in window {}", win_name)
}

/// Shared completion handler: called after any background pane exits.
///
/// Handles:
/// - Capture + normalize + mask pane output
/// - Archive to `pane_logs`
/// - Inject AI context message into session history (if `session_id` is set)
/// - Send `tmux display-message` overlay to the chat pane (if known)
/// - Send `Response::SystemMsg` via `notify_tx` (if set)
/// - Emits `job_complete` and `gc_window` events.
/// - GC: kill the job window after a delay
pub async fn notify_job_completion(
    pane_id: String,
    cmd: String,
    win_name: String,
    session: String,
    exit_code: i32,
    session_id: Option<String>,
    sessions: SessionStore,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
    started_at: tokio::time::Instant,
) {
    let raw = tmux::capture_pane(&pane_id, 5000).unwrap_or_default();
    let duration_ms = started_at.elapsed().as_millis() as u64;

    log_event("job_complete", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "job_id": win_name.split('-').last().unwrap_or(""),
        "job_name": win_name,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
    }));

    // Archive logs
    let logs_dir = crate::config::config_dir().join("pane_logs");
    let _ = std::fs::create_dir_all(&logs_dir);
    let _ = tmux::pane::capture_pane_to_file(&pane_id, &logs_dir.join(format!("{}.log", win_name)));

    let normalized = normalize_output(&raw);
    let body = if normalized.is_empty() {
        "(no output)".to_string()
    } else {
        mask_sensitive(&normalized)
    };

    let status_word = if exit_code == 0 { "succeeded" } else { "failed" };
    let alert_msg = format!("`{}` {} in pane {}", cmd, status_word, pane_id);

    // Inject AI context + tmux display-message (if a session is associated)
    if let Some(ref sid) = session_id {
        if let Ok(mut store) = sessions.lock() {
            if let Some(entry) = store.get_mut(sid) {
                let history_msg = format!(
                    "Background command `{}` in window {} finished with exit code {}.\n<output>\n{}\n</output>",
                    cmd, win_name, exit_code, body
                );
                entry.messages.push(Message {
                    role: "user".to_string(),
                    content: format!("[Background Task Completed]\n{}", history_msg),
                    tool_calls: None,
                    tool_results: None,
                });
                crate::daemon::session::write_session_file(sid, &entry.messages);

                if let Some(ref cp) = entry.chat_pane {
                    let _ = std::process::Command::new("tmux")
                        .args(["display-message", "-d", "5000", "-t", cp, &alert_msg])
                        .output();
                }
            }
        }
    }

    // Also send as a SystemMsg to any listening chat client
    if let Some(ref tx) = notify_tx {
        let _ = tx.send(Response::SystemMsg(alert_msg));
    }

    // GC: kill window after a delay (keep failed windows open longer for inspection)
    let gc_delay = if exit_code == 0 { Duration::from_secs(5) } else { Duration::from_secs(60) };
    tokio::time::sleep(gc_delay).await;

    let reason = if exit_code == 0 { "done" } else if exit_code == 124 { "timeout" } else { "error" };
    log_event("gc_window", serde_json::json!({
        "session": session_id.as_deref().unwrap_or("-"),
        "win_name": win_name,
        "pane": pane_id,
        "reason": reason,
    }));
    let _ = tmux::kill_job_window(&session, &win_name);
}

/// A tool call collected during AI streaming, to be executed after `Done`.
pub enum PendingCall {
    Foreground { id: String, thought_signature: Option<String>, cmd: String, target: Option<String> },
    Background { id: String, thought_signature: Option<String>, cmd: String, _credential: Option<String> },
    ScheduleCommand {
        id: String,
        thought_signature: Option<String>,
        name: String,
        command: String,
        is_script: bool,
        run_at: Option<String>,
        interval: Option<String>,
        runbook: Option<String>,
    },
    ListSchedules { id: String, thought_signature: Option<String> },
    CancelSchedule { id: String, thought_signature: Option<String>, job_id: String },
    DeleteSchedule { id: String, thought_signature: Option<String>, job_id: String },
    WriteScript { id: String, thought_signature: Option<String>, script_name: String, content: String },
    ListScripts { id: String, thought_signature: Option<String> },
    ReadScript { id: String, thought_signature: Option<String>, script_name: String },
    WatchPane { id: String, thought_signature: Option<String>, pane_id: String },
}

impl PendingCall {
    pub fn to_tool_call(&self) -> ToolCall {
        match self {
            PendingCall::Foreground { id, thought_signature, cmd, target } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: serde_json::json!({
                    "command": cmd,
                    "background": false,
                    "target_pane": target
                }).to_string(),
            },
            PendingCall::Background { id, thought_signature, cmd, .. } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "run_terminal_command".to_string(),
                arguments: serde_json::json!({"command": cmd, "background": true}).to_string(),
            },
            PendingCall::ScheduleCommand { id, thought_signature, name, command, is_script, run_at, interval, runbook } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "schedule_command".to_string(),
                arguments: serde_json::json!({
                    "name": name, "command": command,
                    "is_script": is_script,
                    "run_at": run_at, "interval": interval, "runbook": runbook
                }).to_string(),
            },
            PendingCall::ListSchedules { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_schedules".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::CancelSchedule { id, thought_signature, job_id } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "cancel_schedule".to_string(),
                arguments: serde_json::json!({"id": job_id}).to_string(),
            },
            PendingCall::DeleteSchedule { id, thought_signature, job_id } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "delete_schedule".to_string(),
                arguments: serde_json::json!({"id": job_id}).to_string(),
            },
            PendingCall::WriteScript { id, thought_signature, script_name, content } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "write_script".to_string(),
                arguments: serde_json::json!({"script_name": script_name, "content": content}).to_string(),
            },
            PendingCall::ListScripts { id, thought_signature } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "list_scripts".to_string(),
                arguments: "{}".to_string(),
            },
            PendingCall::ReadScript { id, thought_signature, script_name } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "read_script".to_string(),
                arguments: serde_json::json!({"script_name": script_name}).to_string(),
            },
            PendingCall::WatchPane { id, thought_signature, pane_id } => ToolCall {
                id: id.clone(),
                thought_signature: thought_signature.clone(),
                name: "watch_pane".to_string(),
                arguments: serde_json::json!({"pane_id": pane_id}).to_string(),
            },
        }
    }

    pub fn id(&self) -> &str {
        match self {
            PendingCall::Foreground { id, .. } => id,
            PendingCall::Background { id, .. } => id,
            PendingCall::ScheduleCommand { id, .. } => id,
            PendingCall::ListSchedules { id, .. } => id,
            PendingCall::CancelSchedule { id, .. } => id,
            PendingCall::DeleteSchedule { id, .. } => id,
            PendingCall::WriteScript { id, .. } => id,
            PendingCall::ListScripts { id, .. } => id,
            PendingCall::ReadScript { id, .. } => id,
            PendingCall::WatchPane { id, .. } => id,
        }
    }
}

/// Run a single scheduled job in a dedicated tmux window.
///
/// - Success: window killed, job marked `Succeeded` (or rescheduled for `Every`).
/// - Failure: window left open for debugging, job marked `Failed`.
pub async fn run_scheduled_job(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    session: String,
    config: Config,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
    let id_short = &job.id[..job.id.len().min(8)];
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let win_name = format!("de-sched-{}-{}", now, id_short);
    let cmd = match &job.action {
        ActionOn::Alert => {
            // Pure alert: no command to run.
            store.mark_done(&job.id, true, None);
            let msg = format!("Watchdog alert: {}", job.name);
            if let Some(ref tx) = notify_tx {
                let _ = tx.send(Response::SystemMsg(msg.clone()));
            }
            fire_notification(&job.name, &msg, &config);
            return;
        }
        ActionOn::Command(c) => c.clone(),
        ActionOn::Script(s) => match scripts::resolve_script(s) {
            Ok(path) => path.to_string_lossy().to_string(),
            Err(e) => {
                let msg = format!("Scheduled job '{}' failed: {}", job.name, e);
                store.mark_done(&job.id, false, Some(msg.clone()));
                if let Some(ref tx) = notify_tx { let _ = tx.send(Response::SystemMsg(msg)); }
                return;
            }
        },
    };

    let wrapped = format!("{}; exit $?", cmd);

    let pane_id = match tmux::create_job_window(&session, &win_name) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Scheduled job '{}': failed to create window: {}", job.name, e);
            store.mark_done(&job.id, false, Some(e.to_string()));
            if let Some(ref tx) = notify_tx { let _ = tx.send(Response::SystemMsg(msg)); }
            return;
        }
    };
    
    // P7: keep the pane alive in a '<dead>' state so we can query pane_dead_status.
    let _ = tmux::set_remain_on_exit(&pane_id, true);

    if let Err(e) = tmux::send_keys(&pane_id, &wrapped) {
        let msg = format!("Scheduled job '{}': failed to send keys: {}", job.name, e);
        store.mark_done(&job.id, false, Some(e.to_string()));
        if let Some(ref tx) = notify_tx { let _ = tx.send(Response::SystemMsg(msg)); }
        return;
    }

    let mut rx = bg_done_subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    
    let exit_code = loop {
        if let Some(code) = tmux::pane_dead_status(&pane_id) {
            break code;
        }
        if tokio::time::Instant::now() >= deadline {
            break 124;
        }
        tokio::select! {
            result = rx.recv() => {
                if let Ok(notified_pane) = result {
                    if notified_pane == pane_id {
                        if let Some(code) = tmux::pane_dead_status(&pane_id) {
                            break code;
                        }
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break 124;
            }
        }
    };

    let raw = tmux::capture_pane(&pane_id, 5000).unwrap_or_default();
    let output = normalize_output(&raw);
    let success = exit_code == 0;

    // Runbook / watchdog AI analysis (scheduled-job specific; runs before GC so the pane is still alive)
    if let Some(ref rb_name) = job.runbook {
        if let Ok(rb) = runbook::load_runbook(rb_name) {
            let api_key = if !config.ai.api_key.is_empty() {
                config.ai.api_key.clone()
            } else {
                std::env::var(match config.ai.provider.as_str() {
                    "openai" => "OPENAI_API_KEY",
                    "gemini" => "GEMINI_API_KEY",
                    _ => "ANTHROPIC_API_KEY",
                }).unwrap_or_default()
            };
            let client = crate::ai::make_client(&config.ai.provider, api_key, config.ai.model.clone());
            let system = runbook::watchdog_system_prompt(&rb);
            let msgs = vec![Message {
                role: "user".to_string(),
                content: format!("Command output:\n```\n{}\n```", output),
                tool_calls: None,
                tool_results: None,
            }];
            let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
            let _ = client.chat(&system, msgs, ai_tx).await;
            let mut ai_response = String::new();
            while let Some(ev) = ai_rx.recv().await {
                if let AiEvent::Token(t) = ev { ai_response.push_str(&t); }
            }
            if ai_response.to_uppercase().contains("ALERT") {
                let msg = format!("[Watchdog] {}: {}", job.name, ai_response.trim());
                if let Some(ref tx) = notify_tx { let _ = tx.send(Response::SystemMsg(msg.clone())); }
                fire_notification(&job.name, &msg, &config);
            }
        }
    }

    store.mark_done(&job.id, success, if success { None } else {
        Some(format!("exit code {}", exit_code))
    });

    // Hand off to the shared notification + GC handler (non-blocking)
    let cmd_str = cmd.to_string();
    let started_at = tokio::time::Instant::now() - Duration::from_secs(60);
    tokio::spawn(notify_job_completion(pane_id, cmd_str, win_name, session, exit_code, None, Default::default(), notify_tx, started_at));
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
pub async fn handle_client(
    stream: UnixStream,
    cache: Arc<SessionCache>,
    sessions: SessionStore,
    schedule_store: Arc<ScheduleStore>,
    session_name: String,
) -> Result<()> {
    let mut config = Config::load().unwrap_or_else(|_| {
        log::warn!("Failed to load config, using defaults");
        Config::default()
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
        Request::NotifyActivity { pane_id, hook_index: _, session_name } => {
            if let Some(tx) = BG_DONE_TX.get() {
                let _ = tx.send(pane_id.clone());
            }

            // Passive monitoring check
            let mut notify_client = None;
            let mut alerted_sessions = Vec::new();

            if let Ok(mut store) = sessions.lock() {
                for (sid, entry) in store.iter_mut() {
                    // Alert any session that is watching this pane
                    if entry.watched_panes.contains(&pane_id) {
                        let msg = format!("Activity detected in monitored pane: {}", pane_id);
                        entry.messages.push(Message {
                            role: "user".to_string(), // Injected as user context for the next turn
                            content: format!("[System] Activity detected in monitored pane {}. Please analyze the new output and inform the user of any results.", pane_id),
                            tool_calls: None,
                            tool_results: None,
                        });
                        crate::daemon::session::write_session_file(sid, &entry.messages);

                        if let Some(ref cp) = entry.chat_pane {
                            notify_client = Some((cp.clone(), msg));
                        }
                        
                        // Remove from watched list so we don't alert on every single new line.
                        // The user/AI can re-engage watch_pane if they want to monitor for another cycle.
                        entry.watched_panes.remove(&pane_id);
                        let _ = crate::tmux::remove_passive_activity_hook(&pane_id);
                        alerted_sessions.push(sid.clone());
                        break; // assumed one session watching
                    }
                }
            }

            if let Some((_chat_pane, msg)) = notify_client {
                log::info!("Activity detected in monitored pane {}; alerting session(s): {:?}", pane_id, alerted_sessions);
                for sid in &alerted_sessions {
                    log_event("watch_alert", serde_json::json!({
                        "session": sid,
                        "pane_id": pane_id,
                        "status": "alerted"
                    }));
                }

                // Trigger external notification hook (e.g. notify-send)
                fire_notification(&format!("watch:{}", pane_id), &msg, &config);

                // Send alert to the tmux status bar. We target the session name from the request
                // so the message is visible even if the user is in a different window.
                let _ = std::process::Command::new("tmux")
                    .args(["display-message", "-d", "5000", "-t", &session_name, &msg])
                    .output();
            }

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
            let mem = sessions.lock().unwrap();
            mem.get(id).map(|e| e.messages.clone())
        })
        .or_else(|| {
            session_id.as_ref().map(|id| read_session_file(id))
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();

    // Preserve the chat_pane in the session store so we can send out-of-band alerts
    if let Some(ref id) = session_id {
        if let Ok(mut store) = sessions.lock() {
            if let Some(entry) = store.get_mut(id) {
                entry.chat_pane = chat_pane.clone();
            }
        }
    }

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
    let session_summary = cache.get_labeled_context(client_pane.as_deref(), chat_pane.as_deref());
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
        let mut pending_calls: Vec<PendingCall> = Vec::new();

        while let Some(event) = ai_rx.recv().await {
            match event {
                AiEvent::Token(t) => {
                    full_response.push_str(&t);
                    send_response_split(&mut tx, Response::Token(t)).await?;
                }
                AiEvent::ToolCall(id, cmd, bg, target, thought_signature) => {
                    if bg {
                        pending_calls.push(PendingCall::Background { id, cmd, _credential: None, thought_signature: thought_signature.clone() });
                    } else {
                        pending_calls.push(PendingCall::Foreground { id, cmd, target, thought_signature: thought_signature.clone() });
                    }
                }
                AiEvent::ScheduleCommand { id, name, command, is_script, run_at, interval, runbook, thought_signature } => {
                    pending_calls.push(PendingCall::ScheduleCommand { id, name, command, is_script, run_at, interval, runbook, thought_signature });
                }
                AiEvent::ListSchedules { id, thought_signature } => {
                    pending_calls.push(PendingCall::ListSchedules { id, thought_signature });
                }
                AiEvent::CancelSchedule { id, job_id, thought_signature } => {
                    pending_calls.push(PendingCall::CancelSchedule { id, job_id, thought_signature });
                }
                AiEvent::DeleteSchedule { id, job_id, thought_signature } => {
                    pending_calls.push(PendingCall::DeleteSchedule { id, job_id, thought_signature });
                }
                AiEvent::WriteScript { id, script_name, content, thought_signature } => {
                    pending_calls.push(PendingCall::WriteScript { id, script_name, content, thought_signature });
                }
                AiEvent::ListScripts { id, thought_signature } => {
                    pending_calls.push(PendingCall::ListScripts { id, thought_signature });
                }
                AiEvent::ReadScript { id, script_name, thought_signature } => {
                    pending_calls.push(PendingCall::ReadScript { id, script_name, thought_signature });
                }
                AiEvent::WatchPane { id, pane_id, thought_signature } => {
                    pending_calls.push(PendingCall::WatchPane { id, pane_id, thought_signature });
                }

                AiEvent::Error(e) => {
                    send_response_split(&mut tx, Response::Error(e)).await?;
                    return Ok(());
                }
                AiEvent::Done(usage) => {
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
                        
                        log_event("ai_turn", serde_json::json!({
                            "session": session_id.as_deref().unwrap_or("-"),
                            "model": config.ai.model,
                            "prompt_tokens": usage.prompt_tokens,
                            "completion_tokens": usage.completion_tokens,
                        }));
                        
                        // Persist the conversation for the next turn.
                        // In-memory: fast lookup within the same daemon run.
                        // On-disk: survives daemon restarts.
                        if let Some(ref id) = session_id {
                            if let Ok(mut store) = sessions.lock() {
                                let entry = store.entry(id.clone()).or_insert_with(|| SessionEntry {
                                    id: id.clone(),
                                    messages: Vec::new(),
                                    last_accessed: Instant::now(),
                                    chat_pane: chat_pane.clone(),
                                    info_pane: None,
                                    watched_panes: Default::default(),
                                });
                                entry.messages = messages.clone();
                                entry.last_accessed = Instant::now();
                                if chat_pane.is_some() {
                                    entry.chat_pane = chat_pane.clone();
                                }
                            }
                            write_session_file(id, &messages);
                        }
                        send_response_split(&mut tx, Response::Ok).await?;
                        return Ok(());
                    }

                    log_event("ai_turn", serde_json::json!({
                        "session": session_id.as_deref().unwrap_or("-"),
                        "model": config.ai.model,
                        "prompt_tokens": usage.prompt_tokens,
                        "completion_tokens": usage.completion_tokens,
                    }));

                    // Push one assistant message listing all tool calls.
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: full_response.clone(),
                        tool_calls: Some(pending_calls.iter().map(|c| c.to_tool_call()).collect()),
                        tool_results: None,
                    });

                    let mut tool_results = Vec::new();
                    for call in &pending_calls {
                        let call_id = call.id().to_string();
                        let result: String = match call {
                            PendingCall::Foreground { id, cmd, target, .. } => {
                                send_response_split(&mut tx, Response::ToolCallPrompt {
                                    id: id.clone(),
                                    command: cmd.clone(),
                                    background: false,
                                }).await?;

                                let mut line = String::new();
                                let read_result = tokio::time::timeout(
                                    Duration::from_secs(60),
                                    rx.read_line(&mut line),
                                ).await;

                                if matches!(read_result, Ok(Ok(0))) { return Ok(()); }

                                let timed_out = read_result.is_err();
                                let approved = match read_result {
                                    Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                                        Ok(Request::ToolCallResponse { id: resp_id, approved }) if resp_id == *id => approved,
                                        _ => false,
                                    },
                                    _ => false,
                                };

                                if !approved {
                                    let decision = if timed_out { "timeout" } else { "denied" };
                                    log::info!("Foreground command {}: {}", decision, cmd);
                                    log_event("command_approval", serde_json::json!({
                                        "session": session_id.as_deref().unwrap_or("-"),
                                        "mode": "foreground",
                                        "cmd": cmd,
                                        "decision": decision,
                                    }));
                                    log_command(session_id.as_deref(), "foreground", "", cmd, decision, "");
                                    if timed_out {
                                        "Approval timed out (60 s); command not executed.".to_string()
                                    } else {
                                        "User denied execution".to_string()
                                    }
                                } else {
                                    log::info!("Foreground command approved: {}", cmd);
                                    log_event("command_approval", serde_json::json!({
                                        "session": session_id.as_deref().unwrap_or("-"),
                                        "mode": "foreground",
                                        "cmd": cmd,
                                        "decision": "approved",
                                    }));
                                    let ai_target = target.as_deref().and_then(|tp: &str| {
                                        // Never inject into the chat pane itself.
                                        if chat_pane.as_deref() == Some(tp) { return None; }
                                        let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
                                        if panes.contains_key(tp) { Some(tp.to_string()) } else { None::<String> }
                                    });

                                    let target_owned: String = if let Some(tp) = ai_target {
                                        tp
                                    } else if let Some(cp) = client_pane.as_deref().filter(|cp| chat_pane.as_deref() != Some(cp)) {
                                        cp.to_string()
                                    } else {
                                        let pane_list: Vec<PaneInfo> = {
                                            let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
                                            let mut v: Vec<PaneInfo> = panes.iter()
                                                .map(|(pid, state)| PaneInfo {
                                                    id: pid.clone(),
                                                    current_cmd: state.current_cmd.clone(),
                                                    summary: state.summary.clone(),
                                                })
                                                .collect();
                                            v.sort_by(|a, b| a.id.cmp(&b.id));
                                            v
                                        };
                                        if pane_list.is_empty() {
                                            send_response_split(&mut tx, Response::Error(
                                                "No tmux panes available".to_string()
                                            )).await?;
                                            return Ok(());
                                        }
                                        let prompt_id = next_tool_id();
                                        send_response_split(&mut tx, Response::PaneSelectPrompt {
                                            id: prompt_id.clone(),
                                            panes: pane_list,
                                        }).await?;
                                        let mut pane_line = String::new();
                                        rx.read_line(&mut pane_line).await?;
                                        match serde_json::from_str::<Request>(pane_line.trim()) {
                                            Ok(Request::PaneSelectResponse { pane_id, .. }) => pane_id,
                                            _ => {
                                                send_response_split(&mut tx, Response::Error(
                                                    "Expected PaneSelectResponse".to_string()
                                                )).await?;
                                                return Ok(());
                                            }
                                        }
                                    };
                                    let target_str = target_owned.as_str();
                                    if target_str.is_empty() {
                                        "No active pane found.".to_string()
                                    } else {
                                        // R6: Reject commands targeting synchronized panes —
                                        // they would broadcast to ALL synced panes simultaneously.
                                        let is_synchronized = {
                                            let panes = cache.panes.read().unwrap_or_else(|e| e.into_inner());
                                            panes.get(target_str).map(|p| p.synchronized).unwrap_or(false)
                                        };
                                        if is_synchronized {
                                            let msg = format!(
                                                "Pane {} has synchronized input enabled — sending a command \
                                                 would broadcast to all synchronized panes simultaneously. \
                                                 Disable synchronization first:\n  \
                                                 tmux set-option -t {} synchronize-panes off",
                                                target_str, target_str
                                            );
                                            send_response_split(&mut tx, Response::SystemMsg(msg.clone())).await?;
                                            msg
                                        } else {
                                        let idle_cmd = tmux::pane_current_command(target_str)
                                            .unwrap_or_default();
                                        let is_remote_pane = get_pane_remote_host(target_str).is_some();

                                        let current_exe = std::env::current_exe()
                                            .unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
                                        let hook_idx = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 10000;
                                        let hook_name = format!("pane-title-changed[@de_fg_{}]", hook_idx);
                                        let notify_cmd = format!(
                                            "run-shell -b '{} notify activity {} 0 \"{}\"'",
                                            current_exe.display(), target_str, session_name
                                        );
                                        let _ = std::process::Command::new("tmux")
                                            .args(["set-hook", "-t", target_str, &hook_name, &notify_cmd])
                                            .output();

                                        let mut fg_rx = bg_done_subscribe();

                                        match tmux::send_keys(target_str, cmd) {
                                            Ok(()) => {
                                                let mut switched_to_working = false;

                                                if command_has_sudo(cmd) {
                                                    let poll = Duration::from_millis(100);
                                                    let mut waited = Duration::ZERO;
                                                    let prompt_timeout = Duration::from_secs(3);
                                                    let needs_password = loop {
                                                        tokio::time::sleep(poll).await;
                                                        waited += poll;
                                                        let cur = tmux::pane_current_command(target_str)
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
                                                        let _ = tmux::select_pane(target_str);
                                                        switched_to_working = true;
                                                    }
                                                }

                                                if is_remote_pane {
                                                    let mut prev_snap = String::new();
                                                    let mut stable_ticks = 0u32;
                                                    let poll = Duration::from_millis(500); // Slower fallback poll
                                                    let cmd_timeout = Duration::from_secs(30);
                                                    let deadline = tokio::time::Instant::now() + cmd_timeout;
                                                    
                                                    loop {
                                                        if tokio::time::Instant::now() >= deadline { break; }
                                                        tokio::select! {
                                                            result = fg_rx.recv() => {
                                                                if let Ok(notified_pane) = result {
                                                                    if notified_pane == target_str {
                                                                        // Title changed. Reset stability check to evaluate new state
                                                                        stable_ticks = 0;
                                                                    }
                                                                }
                                                            }
                                                            _ = tokio::time::sleep(poll) => {
                                                                let snap = tmux::capture_pane(target_str, 10).unwrap_or_default();
                                                                if snap == prev_snap && !snap.is_empty() {
                                                                    stable_ticks += 1;
                                                                    if stable_ticks >= 2 { break; }
                                                                } else {
                                                                    stable_ticks = 0;
                                                                    prev_snap = snap;
                                                                }
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    let fast_poll = Duration::from_millis(25);
                                                    let start_timeout = Duration::from_millis(300);
                                                    let cmd_timeout = Duration::from_secs(45);
                                                    let deadline = tokio::time::Instant::now() + cmd_timeout;

                                                    // Phase 1: Wait up to 300ms for child process to appear.
                                                    let saw_child = tokio::time::timeout(start_timeout, async {
                                                        loop {
                                                            tokio::time::sleep(fast_poll).await;
                                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                            if cur != idle_cmd { break; }
                                                        }
                                                    }).await.is_ok();

                                                    // Phase 2: If we saw a child, wait for it to finish using event-driven hook
                                                    if saw_child {
                                                        let slow_poll = Duration::from_millis(500);
                                                        loop {
                                                            if tokio::time::Instant::now() >= deadline { break; }
                                                            tokio::select! {
                                                                result = fg_rx.recv() => {
                                                                    if let Ok(notified_pane) = result {
                                                                        if notified_pane == target_str {
                                                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                                            if cur == idle_cmd { break; }
                                                                        }
                                                                    }
                                                                }
                                                                _ = tokio::time::sleep(slow_poll) => {
                                                                    let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                                    if cur == idle_cmd { break; }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                
                                                // Cleanup the hook
                                                let _ = std::process::Command::new("tmux")
                                                    .args(["set-hook", "-u", "-t", target_str, &hook_name])
                                                    .output();

                                                tokio::time::sleep(Duration::from_millis(50)).await;

                                                let output = match tmux::capture_pane(target_str, 200) {
                                                    Ok(snap) => {
                                                        let extracted = extract_command_output(&snap, cmd);
                                                        mask_sensitive(&normalize_output(&extracted))
                                                    }
                                                    Err(_) => "Command sent but could not capture output.".to_string(),
                                                };

                                                if switched_to_working {
                                                    if let Some(ref cp) = chat_pane {
                                                        let _ = tmux::select_pane(cp);
                                                    }
                                                }

                                                send_response_split(&mut tx, Response::ToolResult(output.clone())).await?;
                                                log_command(session_id.as_deref(), "foreground", target_str, cmd, "approved", &output);
                                                output
                                            }
                                            Err(e) => {
                                                let msg = format!("Failed to send command: {}", e);
                                                log_command(session_id.as_deref(), "foreground", target_str, cmd, "send-failed", &msg);
                                                msg
                                            }
                                        }

                                        } // end R6 else (not synchronized)
                                    }
                                }
                            }

                            PendingCall::Background { id, cmd, .. } => {
                                send_response_split(&mut tx, Response::ToolCallPrompt {
                                    id: id.clone(),
                                    command: cmd.clone(),
                                    background: true,
                                }).await?;

                                let mut line = String::new();
                                let read_result = tokio::time::timeout(
                                    Duration::from_secs(60),
                                    rx.read_line(&mut line),
                                ).await;

                                if matches!(read_result, Ok(Ok(0))) { return Ok(()); }

                                let timed_out = read_result.is_err();
                                let approved = match read_result {
                                    Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                                        Ok(Request::ToolCallResponse { id: resp_id, approved }) if resp_id == *id => approved,
                                        _ => false,
                                    },
                                    _ => false,
                                };

                                if !approved {
                                    let decision = if timed_out { "timeout" } else { "denied" };
                                    log::info!("Background command {}: {}", decision, cmd);
                                    log_event("command_approval", serde_json::json!({
                                        "session": session_id.as_deref().unwrap_or("-"),
                                        "mode": "background",
                                        "cmd": cmd,
                                        "decision": decision,
                                    }));
                                    log_command(session_id.as_deref(), "background", "", cmd, decision, "");
                                    if timed_out {
                                        "Approval timed out (60 s); command not executed.".to_string()
                                    } else {
                                        "User denied execution".to_string()
                                    }
                                } else {
                                    log::info!("Background command approved: {}", cmd);
                                    log_event("command_approval", serde_json::json!({
                                        "session": session_id.as_deref().unwrap_or("-"),
                                        "mode": "background",
                                        "cmd": cmd,
                                        "decision": "approved",
                                    }));
                                    let credential = if command_has_sudo(cmd) {
                                        send_response_split(&mut tx, Response::CredentialPrompt {
                                            id: id.clone(),
                                            prompt: format!("[sudo] password required for: {}", cmd),
                                        }).await?;
                                        let mut cred_line = String::new();
                                        match tokio::time::timeout(
                                            Duration::from_secs(120),
                                            rx.read_line(&mut cred_line),
                                        ).await {
                                            Ok(Ok(_)) => match serde_json::from_str::<Request>(cred_line.trim()) {
                                                Ok(Request::CredentialResponse { credential, .. }) => Some(credential),
                                                _ => None,
                                            },
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    };

                                    let output = run_background_in_window(
                                        &session_name,
                                        id,
                                        cmd,
                                        credential.as_deref(),
                                        session_id.clone(),
                                        sessions.clone(),
                                    ).await;
                                    send_response_split(&mut tx, Response::ToolResult(output.clone())).await?;
                                    log_command(session_id.as_deref(), "background", "", cmd, "approved", &output);
                                    output
                                }
                            }

                            PendingCall::ScheduleCommand { id: call_id, name, command, is_script, run_at, interval, runbook, .. } => {
                                let action = if *is_script {
                                    ActionOn::Script(command.clone())
                                } else {
                                    ActionOn::Command(command.clone())
                                };
                                let kind = if let Some(iso) = interval {
                                    let secs = crate::scheduler::parse_iso_duration(iso).unwrap_or(3600);
                                    let next = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
                                    ScheduleKind::Every { interval_secs: secs, next_run: next }
                                } else if let Some(at_str) = run_at {
                                    let at = chrono::DateTime::parse_from_rfc3339(at_str).map(|d| d.with_timezone(&chrono::Utc))
                                        .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::seconds(60));
                                    ScheduleKind::Once { at }
                                } else {
                                    ScheduleKind::Once { at: chrono::Utc::now() + chrono::Duration::seconds(60) }
                                };

                                send_response_split(&mut tx, Response::ScheduleWritePrompt {
                                    id: call_id.clone(),
                                    name: name.clone(),
                                    kind: kind.describe(),
                                    action: action.describe(),
                                }).await?;

                                let mut line = String::new();
                                let read_result = tokio::time::timeout(
                                    Duration::from_secs(120),
                                    rx.read_line(&mut line),
                                ).await;
                                if matches!(read_result, Ok(Ok(0))) { return Ok(()); }
                                let approved = match read_result {
                                    Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                                        Ok(Request::ScheduleWriteResponse { approved, .. }) => approved,
                                        _ => false,
                                    },
                                    _ => false,
                                };

                                if approved {
                                    let job = ScheduledJob::new(name.clone(), kind.clone(), action, runbook.clone());
                                    match schedule_store.add(job) {
                                        Ok(job_id) => {
                                            log::info!("Job scheduled: '{}' ({})", name, &job_id[..8]);
                                            log_event("job_scheduled", serde_json::json!({
                                                "session": session_id.as_deref().unwrap_or("-"),
                                                "job_id": &job_id,
                                                "job_name": name,
                                                "kind": kind.describe(),
                                            }));
                                            format!("Scheduled job '{}' created (id: {})", name, &job_id[..8])
                                        }
                                        Err(e) => format!("Failed to schedule job: {}", e),
                                    }
                                } else {
                                    log_event("command_approval", serde_json::json!({
                                        "session": session_id.as_deref().unwrap_or("-"),
                                        "mode": "schedule",
                                        "cmd": command,
                                        "decision": "denied",
                                    }));
                                    "Job scheduling denied by user".to_string()
                                }
                            }

                            PendingCall::ListSchedules { id: _, .. } => {
                                let jobs = schedule_store.list();
                                let items: Vec<ScheduleListItem> = jobs.iter().map(|j| ScheduleListItem {
                                    id: j.id.clone(),
                                    name: j.name.clone(),
                                    kind: j.kind.describe(),
                                    action: j.action.describe(),
                                    status: j.status.describe(),
                                    last_run: j.last_run.map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
                                    next_run: j.kind.next_run().map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
                                }).collect();
                                let count = items.len();
                                let _ = send_response_split(&mut tx, Response::ScheduleList { jobs: items }).await;
                                format!("{} scheduled job(s)", count)
                            }

                            PendingCall::CancelSchedule { id: _, job_id, .. } => {
                                match schedule_store.cancel(job_id) {
                                    Ok(true) => {
                                        log::info!("Job canceled: {}", &job_id[..job_id.len().min(8)]);
                                        log_event("job_canceled", serde_json::json!({
                                            "session": session_id.as_deref().unwrap_or("-"),
                                            "job_id": job_id,
                                        }));
                                        format!("Job {} cancelled", &job_id[..job_id.len().min(8)])
                                    }
                                    Ok(false) => format!("Job {} not found", job_id),
                                    Err(e)  => format!("Failed to cancel job: {}", e),
                                }
                            }

                            PendingCall::DeleteSchedule { id: _, job_id, .. } => {
                                match schedule_store.delete(job_id) {
                                    Ok(true) => {
                                        log::info!("Job deleted: {}", &job_id[..job_id.len().min(8)]);
                                        log_event("job_deleted", serde_json::json!({
                                            "session": session_id.as_deref().unwrap_or("-"),
                                            "job_id": job_id,
                                        }));
                                        format!("Job {} deleted permanently", &job_id[..job_id.len().min(8)])
                                    }
                                    Ok(false) => format!("Job {} not found", job_id),
                                    Err(e)  => format!("Failed to delete job: {}", e),
                                }
                            }

                            PendingCall::WriteScript { id, script_name, content, .. } => {
                                send_response_split(&mut tx, Response::ScriptWritePrompt {
                                    id: id.clone(),
                                    script_name: script_name.clone(),
                                    content: content.clone(),
                                }).await?;

                                let mut line = String::new();
                                let read_result = tokio::time::timeout(
                                    Duration::from_secs(120),
                                    rx.read_line(&mut line),
                                ).await;
                                if matches!(read_result, Ok(Ok(0))) { return Ok(()); }
                                let approved = match read_result {
                                    Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                                        Ok(Request::ScriptWriteResponse { approved, .. }) => approved,
                                        _ => false,
                                    },
                                    _ => false,
                                };

                                if approved {
                                    match scripts::write_script(script_name, content) {
                                        Ok(()) => format!("Script '{}' written successfully", script_name),
                                        Err(e) => format!("Failed to write script: {}", e),
                                    }
                                } else {
                                    "Script write denied by user".to_string()
                                }
                            }

                            PendingCall::ListScripts { id: _, .. } => {
                                let script_list = scripts::list_scripts().unwrap_or_default();
                                let items: Vec<ScriptListItem> = script_list.iter()
                                    .map(|s| ScriptListItem { name: s.name.clone(), size: s.size })
                                    .collect();
                                let count = items.len();
                                let _ = send_response_split(&mut tx, Response::ScriptList { scripts: items }).await;
                                format!("{} script(s) in ~/.daemoneye/scripts/", count)
                            }

                            PendingCall::ReadScript { id: _, script_name, .. } => {
                                match scripts::read_script(script_name) {
                                    Ok(content) => content,
                                    Err(e) => format!("Error reading script '{}': {}", script_name, e),
                                }
                            }

                            PendingCall::WatchPane { id: _, pane_id, .. } => {
                                let session_owned = session_name.clone();
                                match crate::tmux::install_passive_activity_hook(&pane_id, &session_owned) {
                                    Ok(_) => {
                                        if let Some(ref sid) = session_id {
                                            if let Ok(mut store) = sessions.lock() {
                                                if let Some(entry) = store.get_mut(sid) {
                                                    entry.watched_panes.insert(pane_id.clone());
                                                }
                                            }
                                        }
                                        log::info!("Watch placed on pane {} for session {}", pane_id, session_id.as_deref().unwrap_or("-"));
                                        log_event("watch_pane", serde_json::json!({
                                            "session": session_id.as_deref().unwrap_or("-"),
                                            "pane_id": pane_id,
                                            "status": "active"
                                        }));
                                        format!("Pane {} has been flagged for passive monitoring. You will be notified out-of-band via a [System] message when it produces output.", pane_id)
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to monitor pane {}: {}", pane_id, e);
                                        format!("Failed to monitor pane {}: {}", pane_id, e)
                                    }
                                }
                            }

                        };
                        tool_results.push(ToolResult { tool_call_id: call_id, content: result });
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

pub async fn send_response(stream: &mut UnixStream, response: Response) -> Result<()> {
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    stream.write_all(&data).await?;
    Ok(())
}

pub async fn send_response_split(tx: &mut tokio::net::unix::OwnedWriteHalf, response: Response) -> Result<()> {
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

