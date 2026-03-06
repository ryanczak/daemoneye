
use crate::daemon::session::*;
use crate::daemon::utils::*;
use tokio::io::AsyncBufReadExt;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use std::time::Duration;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::{make_client, AiEvent, Message, ToolResult, PendingCall};
use crate::ai::filter::mask_sensitive;
use crate::config::{Config, load_named_prompt};
use crate::sys_context::get_or_init_sys_context;
use crate::scheduler::{ActionOn, ScheduledJob, ScheduleStore};
use crate::runbook;
use crate::scripts;
use crate::daemon::background::notify_job_completion;

/// Run a single scheduled job in a dedicated tmux window.
///
/// - Success: window killed, job marked `Succeeded` (or rescheduled for `Every`).
/// - Failure: window left open for debugging, job marked `Failed`.
pub async fn run_scheduled_job(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    session: String,
    sessions: SessionStore,
    config: Config,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
    let id_short = &job.id[..job.id.len().min(8)];
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let win_name = format!("{}{}-{}", crate::daemon::SCHED_WINDOW_PREFIX, now, id_short);
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
    if let Err(e) = tmux::set_remain_on_exit(&pane_id, true) {
        log::warn!("Failed to set remain-on-exit for {}: {}", win_name, e);
    }

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
            let api_key = config.ai.resolve_api_key();
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
    tokio::spawn(notify_job_completion(pane_id, cmd_str, win_name, session, exit_code, None, sessions, notify_tx, started_at));
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
    // Ensure API key is resolved from env if missing in config file
    if config.ai.api_key.is_empty() {
        config.ai.api_key = config.ai.resolve_api_key();
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
                                    messages: Vec::new(),
                                    last_accessed: Instant::now(),
                                    chat_pane: chat_pane.clone(),
                                    default_target_pane: None,
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
                        let result = match crate::daemon::executor::execute_tool_call(
                            call, &mut tx, &mut rx, session_id.as_deref(), &session_name,
                            chat_pane.as_deref(), &cache, &sessions, &schedule_store
                        ).await {
                            Ok(res) => res,
                            Err(_) => return Ok(()),
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
