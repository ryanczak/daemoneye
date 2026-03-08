
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
    bg_session: Arc<std::sync::Mutex<String>>,
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

    let (initial_query, client_pane, session_id, chat_pane, prompt_override, chat_width, client_tmux_session, client_target_pane) = match request {
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
        Request::Ask { query, tmux_pane, session_id, chat_pane, prompt, chat_width, tmux_session, target_pane } =>
            (query, tmux_pane, session_id, chat_pane, prompt, chat_width, tmux_session, target_pane),
        Request::Refresh => {
            crate::sys_context::refresh_sys_context();
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        Request::NotifyActivity { pane_id, hook_index: _, session_name: _ } => {
            if let Some(tx) = BG_DONE_TX.get() {
                let _ = tx.send(pane_id.clone());
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        Request::NotifyComplete { pane_id, exit_code, session_name: _ } => {
            if let Some(tx) = crate::daemon::session::COMPLETE_TX.get() {
                let _ = tx.send((pane_id, exit_code));
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        _ => return Ok(()),
    };

    // Derive the tmux session name: prefer what the client told us, fall back
    // to whatever is already stored in bg_session (e.g. detected at startup).
    let session_name: String = client_tmux_session
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| bg_session.lock().unwrap_or_else(|e| e.into_inner()).clone());

    // Adopt the session if the daemon doesn't have one yet (systemd startup case).
    if !session_name.is_empty() {
        let mut current = bg_session.lock().unwrap_or_else(|e| e.into_inner());
        if current.is_empty() {
            *current = session_name.clone();
            drop(current);
            cache.set_session(&session_name);
            let hook_exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "daemoneye".to_string());
            crate::daemon::install_session_hooks(&session_name, &hook_exe);
            log::info!("Adopted tmux session from client: {}", session_name);
        }
    }

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

    // Upsert the session entry: create it with the client-resolved target pane if
    // new, or refresh chat_pane and adopt client_target_pane if not yet set.
    if let Some(ref id) = session_id {
        if let Ok(mut store) = sessions.lock() {
            let entry = store.entry(id.clone()).or_insert_with(|| SessionEntry {
                messages: Vec::new(),
                last_accessed: Instant::now(),
                chat_pane: chat_pane.clone(),
                default_target_pane: client_target_pane.clone(),
                bg_windows: Vec::new(),
                last_prompt_tokens: 0,
            });
            entry.chat_pane = chat_pane.clone();
            if entry.default_target_pane.is_none() {
                entry.default_target_pane = client_target_pane.clone();
            }
            // Ensure the field exists on entries created before this was added.
            // (No-op for new entries; safe for loaded-from-disk sessions.)
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
    let pre_trim_len = messages.len();
    messages = trim_history(messages);
    // If trim dropped messages the on-disk file must be fully rewritten to remove
    // the stale entries.  Otherwise we can append-only at the end of each turn.
    let needs_compaction = messages.len() < pre_trim_len;
    let post_trim_len = messages.len();

    let is_first_turn = messages.is_empty();

    // Read last prompt token count for context-budget injection and client display.
    let last_prompt_tokens = session_id.as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.last_prompt_tokens))
        .unwrap_or(0);

    let safe_query = mask_sensitive(&initial_query);

    // First turn: include full host context + terminal snapshot.
    // Subsequent turns: budget note + query only. The AI calls get_terminal_context
    // when it needs a fresh snapshot, keeping mid-turn messages lean.
    let prompt = if is_first_turn {
        let session_summary = cache.get_labeled_context(client_pane.as_deref(), chat_pane.as_deref());
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
        let memory_block = crate::memory::load_session_memory_block();
        format!(
            "## Host Context\n```\n{sys_ctx}\n```\n\n\
             ## Execution Context\n\
             - Environment: {environment}\n\
             - Daemon host: {daemon_host}\n\
             - User's terminal pane: {pane_location}\
             {width_hint}\n\
             - background=true  → runs on DAEMON HOST ({daemon_host})\n\
             - background=false → runs in USER'S PANE ({pane_location})\n\n\
             {memory_block}\
             ## Terminal Session\n```\n{session_summary}\n```\n\n\
             User: {safe_query}"
        )
    } else {
        // Inject a context-budget line so the AI knows how much context it has consumed.
        // Use percentage thresholds so the signal is meaningful regardless of model.
        let context_window = config.ai.context_window();
        let pct_used = if context_window > 0 {
            (last_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
        } else { 0 };
        let budget_note = if pct_used >= 75 {
            format!("[Token Budget] Context at {}k / {}k tokens ({}% used) — NEAR LIMIT. Be very concise. Suggest `/clear` if the task is complete.\n\n",
                last_prompt_tokens / 1000, context_window / 1000, pct_used)
        } else if pct_used >= 50 {
            format!("[Token Budget] Context at {}k / {}k tokens ({}% used) — prefer concise responses, avoid redundant tool calls.\n\n",
                last_prompt_tokens / 1000, context_window / 1000, pct_used)
        } else if last_prompt_tokens > 0 {
            format!("[Token Budget] Context at {}k / {}k tokens ({}% used).\n\n",
                last_prompt_tokens / 1000, context_window / 1000, pct_used)
        } else {
            String::new()
        };
        format!(
            "{budget_note}User: {safe_query}"
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
                AiEvent::WatchPane { id, pane_id, timeout_secs, thought_signature } => {
                    pending_calls.push(PendingCall::WatchPane { id, pane_id, timeout_secs, thought_signature });
                }
                AiEvent::WriteRunbook { id, name, content, thought_signature } => {
                    pending_calls.push(PendingCall::WriteRunbook { id, thought_signature, name, content });
                }
                AiEvent::DeleteRunbook { id, name, thought_signature } => {
                    pending_calls.push(PendingCall::DeleteRunbook { id, thought_signature, name });
                }
                AiEvent::ReadRunbook { id, name, thought_signature } => {
                    pending_calls.push(PendingCall::ReadRunbook { id, thought_signature, name });
                }
                AiEvent::ListRunbooks { id, thought_signature } => {
                    pending_calls.push(PendingCall::ListRunbooks { id, thought_signature });
                }
                AiEvent::AddMemory { id, key, value, category, thought_signature } => {
                    pending_calls.push(PendingCall::AddMemory { id, thought_signature, key, value, category });
                }
                AiEvent::DeleteMemory { id, key, category, thought_signature } => {
                    pending_calls.push(PendingCall::DeleteMemory { id, thought_signature, key, category });
                }
                AiEvent::ReadMemory { id, key, category, thought_signature } => {
                    pending_calls.push(PendingCall::ReadMemory { id, thought_signature, key, category });
                }
                AiEvent::ListMemories { id, category, thought_signature } => {
                    pending_calls.push(PendingCall::ListMemories { id, thought_signature, category });
                }
                AiEvent::SearchRepository { id, query, kind, thought_signature } => {
                    pending_calls.push(PendingCall::SearchRepository { id, thought_signature, query, kind });
                }
                AiEvent::GetTerminalContext { id, thought_signature } => {
                    pending_calls.push(PendingCall::GetTerminalContext { id, thought_signature });
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
                                if let Some(entry) = store.get_mut(id) {
                                    entry.messages = messages.clone();
                                    entry.last_accessed = Instant::now();
                                    entry.last_prompt_tokens = usage.prompt_tokens;
                                    if chat_pane.is_some() {
                                        entry.chat_pane = chat_pane.clone();
                                    }
                                }
                            }
                            if needs_compaction {
                                write_session_file(id, &messages);
                            } else {
                                for msg in &messages[post_trim_len..] {
                                    crate::daemon::session::append_session_message(id, msg);
                                }
                            }
                        }
                        send_response_split(&mut tx, Response::UsageUpdate {
                            prompt_tokens: usage.prompt_tokens,
                        }).await?;
                        send_response_split(&mut tx, Response::Ok).await?;
                        return Ok(());
                    }

                    log_event("ai_turn", serde_json::json!({
                        "session": session_id.as_deref().unwrap_or("-"),
                        "model": config.ai.model,
                        "prompt_tokens": usage.prompt_tokens,
                        "completion_tokens": usage.completion_tokens,
                    }));

                    // Update session token tracking so the budget line in the next
                    // prompt reflects the actual context size of this turn.
                    if let Some(ref id) = session_id {
                        if let Ok(mut store) = sessions.lock() {
                            if let Some(entry) = store.get_mut(id) {
                                entry.last_prompt_tokens = usage.prompt_tokens;
                            }
                        }
                    }

                    // Push one assistant message listing all tool calls.
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: full_response.clone(),
                        tool_calls: Some(pending_calls.iter().map(|c| c.to_tool_call()).collect()),
                        tool_results: None,
                    });

                    // Per-turn tool-call loop guard.
                    // Prevents the AI from looping endlessly through the same tools.
                    const MAX_SAME_TOOL: u32 = 2;
                    const MAX_TOTAL_CALLS: u32 = 12;
                    let mut tool_call_counts: std::collections::HashMap<&'static str, u32> = std::collections::HashMap::new();

                    let mut tool_results = Vec::new();
                    for (call_idx, call) in pending_calls.iter().enumerate() {
                        let call_id = call.id().to_string();

                        // Hard total cap: block all calls beyond the limit.
                        if call_idx as u32 >= MAX_TOTAL_CALLS {
                            log::warn!("Turn tool-call total limit ({MAX_TOTAL_CALLS}) reached; blocking call {}", call_idx + 1);
                            tool_results.push(ToolResult {
                                tool_call_id: call_id,
                                content: format!(
                                    "Error: turn tool-call limit ({MAX_TOTAL_CALLS}) reached. \
                                     This call was not executed. Stop calling tools and \
                                     respond to the user with what you have."
                                ),
                            });
                            continue;
                        }

                        // Per-tool cap: block repeated calls to the same tool.
                        let tool_name = call.tool_name();
                        let count = tool_call_counts.entry(tool_name).or_insert(0);
                        *count += 1;
                        if *count > MAX_SAME_TOOL {
                            log::warn!("Per-tool limit for `{tool_name}` reached ({MAX_SAME_TOOL}); blocking call");
                            tool_results.push(ToolResult {
                                tool_call_id: call_id,
                                content: format!(
                                    "Error: `{tool_name}` has been called {MAX_SAME_TOOL} times \
                                     this turn. This call was not executed. Proceed with the \
                                     information already gathered and do not call this tool again."
                                ),
                            });
                            continue;
                        }

                        let result = match crate::daemon::executor::execute_tool_call(
                            call, &mut tx, &mut rx, session_id.as_deref(), &session_name,
                            chat_pane.as_deref(), &cache, &sessions, &schedule_store
                        ).await {
                            Ok(res) => res,
                            Err(_) => return Ok(()),
                        };
                        tool_results.push(ToolResult { tool_call_id: call_id, content: result });
                    }

                    // Truncate tool results before storing in history.
                    // The full output was already delivered to the AI as the live result;
                    // only the history copy needs to be capped to prevent context bloat.
                    const MAX_TOOL_RESULT_CHARS: usize = 4_000;
                    let history_results: Vec<ToolResult> = tool_results.into_iter().map(|r| {
                        if r.content.len() <= MAX_TOOL_RESULT_CHARS {
                            r
                        } else {
                            // Snap to a valid UTF-8 char boundary.
                            let mut end = MAX_TOOL_RESULT_CHARS;
                            while !r.content.is_char_boundary(end) { end -= 1; }
                            ToolResult {
                                tool_call_id: r.tool_call_id,
                                content: format!(
                                    "{}\n[truncated — {} chars total; full output archived in pane log]",
                                    &r.content[..end], r.content.len()
                                ),
                            }
                        }
                    }).collect();

                    // Push one message with all results so message history is valid.
                    messages.push(Message {
                        role: "user".to_string(),
                        content: String::new(),
                        tool_calls: None,
                        tool_results: Some(history_results),
                    });
                    break; // break inner loop; outer loop makes the next AI call
                }
            }
        }
        
    }
}
