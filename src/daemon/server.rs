use crate::ai::filter::mask_sensitive;
use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::default_socket_path;
use crate::config::{Config, load_named_prompt};
use crate::daemon::background::notify_job_completion;
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use crate::runbook;
use crate::scheduler::{ActionOn, ScheduleStore, ScheduledJob};
use crate::scripts;
use crate::sys_context::get_or_init_sys_context;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use libc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;

/// Build the N15 catch-up brief from messages injected while the client was away.
///
/// `new_msgs` is the slice of messages added after detach.
/// `away_secs` is how long the client was gone.
/// Returns `None` when the absence was too short or no relevant events occurred.
/// Validate that a pane_id received from an external hook matches the tmux
/// format `%<digits>` (e.g. `%0`, `%23`).  Rejects anything else so that
/// crafted hook payloads cannot inject escape sequences or unexpected strings
/// into the cache or broadcast channels.
fn is_valid_pane_id(id: &str) -> bool {
    id.starts_with('%') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn build_catchup_brief(
    new_msgs: &[crate::ai::Message],
    away_secs: u64,
) -> Option<String> {
    // Skip if the user was away less than 30 s — too brief to be useful.
    if away_secs < 30 {
        return None;
    }
    if new_msgs.is_empty() {
        return None;
    }

    // Scan for injected event messages the AI adds to session history.
    let events: Vec<String> = new_msgs
        .iter()
        .filter_map(|m| {
            let c = &m.content;
            if c.contains("[Background Task Completed")
                || c.contains("[Webhook Alert]")
                || c.contains("[Watchdog]")
                || c.contains("[Watch Pane")
            {
                // Extract just the first line as a terse summary.
                Some(c.lines().next().unwrap_or(c.as_str()).trim().to_string())
            } else {
                None
            }
        })
        .collect();

    if events.is_empty() {
        return None;
    }

    let away_str = if away_secs < 60 {
        format!("{}s", away_secs)
    } else if away_secs < 3600 {
        format!("{}m", away_secs / 60)
    } else {
        format!("{}h{}m", away_secs / 3600, (away_secs % 3600) / 60)
    };
    let count = events.len();
    let lines = events
        .iter()
        .map(|e| format!("  • {}", e))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "[Catch-up] {} event{} while you were away ({}):\n{}",
        count,
        if count == 1 { "" } else { "s" },
        away_str,
        lines,
    ))
}

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
    crate::daemon::stats::inc_schedules_executed();
    if matches!(job.action, ActionOn::Script(_)) {
        crate::daemon::stats::inc_scripts_executed();
    }

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
                if let Some(ref tx) = notify_tx {
                    let _ = tx.send(Response::SystemMsg(msg));
                }
                return;
            }
        },
    };

    let wrapped = format!("{}; exit $?", cmd);

    let pane_id = match tmux::create_job_window(&session, &win_name) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!(
                "Scheduled job '{}': failed to create window: {}",
                job.name, e
            );
            store.mark_done(&job.id, false, Some(e.to_string()));
            if let Some(ref tx) = notify_tx {
                let _ = tx.send(Response::SystemMsg(msg));
            }
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
        if let Some(ref tx) = notify_tx {
            let _ = tx.send(Response::SystemMsg(msg));
        }
        return;
    }

    let cmd_id = crate::daemon::stats::start_command(&cmd, "scheduled");

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

    crate::daemon::stats::finish_command(cmd_id, exit_code);

    let raw = tmux::capture_pane(&pane_id, 5000).unwrap_or_default();
    let output = normalize_output(&raw);
    let success = exit_code == 0;

    // Runbook / watchdog AI analysis (scheduled-job specific; runs before GC so the pane is still alive)
    if let Some(ref rb_name) = job.runbook {
        if let Ok(rb) = runbook::load_runbook(rb_name) {
            let api_key = config.ai.resolve_api_key();
            let client = crate::ai::make_client(
                &config.ai.provider,
                api_key,
                config.ai.model.clone(),
                config.ai.effective_base_url(),
            );
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
                if let AiEvent::Token(t) = ev {
                    ai_response.push_str(&t);
                }
            }
            if ai_response.to_uppercase().contains("ALERT") {
                let msg = format!("[Watchdog] {}: {}", job.name, ai_response.trim());
                if let Some(ref tx) = notify_tx {
                    let _ = tx.send(Response::SystemMsg(msg.clone()));
                }
                fire_notification(&job.name, &msg, &config);
            }
        }
    }

    store.mark_done(
        &job.id,
        success,
        if success {
            None
        } else {
            Some(format!("exit code {}", exit_code))
        },
    );

    // Hand off to the shared notification + GC handler (non-blocking)
    let cmd_str = cmd.to_string();
    let started_at = tokio::time::Instant::now() - Duration::from_secs(60);
    tokio::spawn(notify_job_completion(
        pane_id, cmd_str, win_name, session, exit_code, None, sessions, notify_tx, started_at,
    ));
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

    /// Maximum size of a single incoming IPC message (1 MiB).
    /// Prevents a malicious or buggy client from exhausting daemon memory by
    /// sending an arbitrarily large JSON payload without a newline.
    const MAX_IPC_MESSAGE_BYTES: usize = 1 << 20;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }
    if line.len() > MAX_IPC_MESSAGE_BYTES {
        let mut stream = reader.into_inner();
        send_response(
            &mut stream,
            Response::Error(format!(
                "Request too large ({} bytes; limit {} bytes)",
                line.len(),
                MAX_IPC_MESSAGE_BYTES
            )),
        )
        .await?;
        return Ok(());
    }

    let request: Request = match serde_json::from_str(line.trim()) {
        Ok(req) => req,
        Err(e) => {
            let mut stream = reader.into_inner();
            send_response(
                &mut stream,
                Response::Error(format!("Invalid request: {}", e)),
            )
            .await?;
            return Ok(());
        }
    };

    let (rx_half, mut tx) = reader.into_inner().into_split();
    let mut rx = BufReader::new(rx_half);

    let (
        initial_query,
        client_pane,
        session_id,
        chat_pane,
        prompt_override,
        chat_width,
        client_tmux_session,
        client_target_pane,
    ) = match request {
        Request::Ping => {
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        Request::Shutdown => {
            send_response_split(&mut tx, Response::Ok).await?;
            // Send SIGTERM to ourselves so the main loop's signal handler runs
            // the full graceful shutdown sequence (hook uninstall, session cleanup,
            // socket removal) rather than exiting here and bypassing it.
            unsafe {
                libc::kill(libc::getpid(), libc::SIGTERM);
            }
            return Ok(());
        }
        Request::Ask {
            query,
            tmux_pane,
            session_id,
            chat_pane,
            prompt,
            chat_width,
            tmux_session,
            target_pane,
        } => (
            query,
            tmux_pane,
            session_id,
            chat_pane,
            prompt,
            chat_width,
            tmux_session,
            target_pane,
        ),
        Request::Refresh => {
            crate::sys_context::refresh_sys_context();
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // F1: return a live status snapshot to `daemoneye status`.
        Request::Status => {
            let uptime_secs = crate::daemon::daemon_uptime_secs();
            let pid = std::process::id();
            let mut active_sessions = 0;
            let mut active_prompt_tokens = 0;
            if let Ok(sess_map) = sessions.lock() {
                active_sessions = sess_map.len();
                active_prompt_tokens = sess_map.values().map(|s| s.last_prompt_tokens).sum();
            }
            let schedule_count = schedule_store.list().len();
            let circuit_state = crate::ai::circuit_state_str().to_string();
            let circuit_failures = crate::ai::circuit_failure_count();

            let commands_fg_succeeded = crate::daemon::stats::get_commands_fg_succeeded();
            let commands_fg_failed = crate::daemon::stats::get_commands_fg_failed();
            let commands_fg_approved = crate::daemon::stats::get_commands_fg_approved();
            let commands_fg_denied = crate::daemon::stats::get_commands_fg_denied();
            let commands_bg_succeeded = crate::daemon::stats::get_commands_bg_succeeded();
            let commands_bg_failed = crate::daemon::stats::get_commands_bg_failed();
            let commands_bg_approved = crate::daemon::stats::get_commands_bg_approved();
            let commands_bg_denied = crate::daemon::stats::get_commands_bg_denied();
            let commands_sched_succeeded = crate::daemon::stats::get_commands_sched_succeeded();
            let commands_sched_failed = crate::daemon::stats::get_commands_sched_failed();
            let webhooks_received = crate::daemon::stats::get_webhooks_received();
            let webhooks_rejected = crate::daemon::stats::get_webhooks_rejected();
            let webhook_url = format!(
                "http://{}:{}/webhook",
                config.webhook.bind_addr, config.webhook.port
            );
            let recent_commands = crate::daemon::stats::get_recent_commands();

            let runbook_count = crate::runbook::list_runbooks().map(|v| v.len()).unwrap_or(0);
            let runbooks_created = crate::daemon::stats::get_runbooks_created();
            let runbooks_executed = crate::daemon::stats::get_runbooks_executed();
            let runbooks_deleted = crate::daemon::stats::get_runbooks_deleted();
            let script_count = crate::scripts::list_scripts().map(|v| v.len()).unwrap_or(0);
            let scripts_created = crate::daemon::stats::get_scripts_created();
            let scripts_executed = crate::daemon::stats::get_scripts_executed();
            let scripts_deleted = crate::daemon::stats::get_scripts_deleted();
            let memories_created = crate::daemon::stats::get_memories_created();
            let memories_recalled = crate::daemon::stats::get_memories_recalled();
            let memories_deleted = crate::daemon::stats::get_memories_deleted();
            let schedules_created = crate::daemon::stats::get_schedules_created();
            let schedules_executed = crate::daemon::stats::get_schedules_executed();
            let schedules_deleted = crate::daemon::stats::get_schedules_deleted();
            let mut memory_breakdown = std::collections::HashMap::new();
            if let Ok(memories) = crate::memory::list_memories(None) {
                for (cat, _) in memories {
                    *memory_breakdown.entry(cat).or_insert(0) += 1;
                }
            }

            let context_window_tokens = config.ai.context_window_tokens.unwrap_or(128000);

            send_response_split(
                &mut tx,
                Response::DaemonStatus {
                    uptime_secs,
                    pid,
                    active_sessions,
                    provider: config.ai.provider.clone(),
                    model: config.ai.model.clone(),
                    socket_path: default_socket_path().display().to_string(),
                    schedule_count,
                    circuit_state,
                    circuit_failures,
                    commands_fg_succeeded,
                    commands_fg_failed,
                    commands_fg_approved,
                    commands_fg_denied,
                    commands_bg_succeeded,
                    commands_bg_failed,
                    commands_bg_approved,
                    commands_bg_denied,
                    commands_sched_succeeded,
                    commands_sched_failed,
                    webhooks_received,
                    webhooks_rejected,
                    webhook_url,
                    runbook_count,
                    runbooks_created,
                    runbooks_executed,
                    runbooks_deleted,
                    script_count,
                    scripts_created,
                    scripts_executed,
                    scripts_deleted,
                    memories_created,
                    memories_recalled,
                    memories_deleted,
                    schedules_created,
                    schedules_executed,
                    schedules_deleted,
                    active_prompt_tokens,
                    context_window_tokens,
                    recent_commands,
                    memory_breakdown,
                    redaction_counts: crate::ai::filter::get_redaction_counts(),
                },
            )
            .await?;
            return Ok(());
        }
        Request::NotifyActivity {
            pane_id,
            hook_index: _,
            session_name: _,
        } => {
            if is_valid_pane_id(&pane_id) {
                if let Some(tx) = BG_DONE_TX.get() {
                    let _ = tx.send(pane_id.clone());
                }
            } else {
                log::warn!("NotifyActivity: rejected invalid pane_id {:?}", pane_id);
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        Request::NotifyComplete {
            pane_id,
            exit_code,
            session_name: _,
        } => {
            if is_valid_pane_id(&pane_id) {
                if let Ok(mut map) = crate::daemon::background::BG_COMMAND_MAP
                    .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
                    .lock()
                {
                    if let Some(cmd_id) = map.remove(&pane_id) {
                        crate::daemon::stats::finish_command(cmd_id, exit_code);
                    }
                }
                // Fix C: use get_or_init so the channel always exists, even if
                // NotifyComplete arrives before any completion monitor has subscribed.
                let tx = crate::daemon::session::COMPLETE_TX.get_or_init(|| {
                    let (tx, _) = tokio::sync::broadcast::channel(32);
                    tx
                });
                let _ = tx.send((pane_id, exit_code));
            } else {
                log::warn!("NotifyComplete: rejected invalid pane_id {:?}", pane_id);
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // N1: pane-focus-in hook — update active pane instantly, no 2 s poll lag.
        Request::NotifyFocus {
            pane_id,
            session_name: _,
        } => {
            if is_valid_pane_id(&pane_id) {
                cache.set_active_pane(&pane_id);
            } else {
                log::warn!("NotifyFocus: rejected invalid pane_id {:?}", pane_id);
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // N2: session-window-changed hook — refresh window topology immediately.
        Request::NotifyWindowChanged { session_name: _ } => {
            cache.refresh_windows();
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // A6: session-closed hook — clean up daemon state when a tmux session is destroyed.
        Request::NotifySessionClosed { session_name } => {
            if let Ok(mut store) = sessions.lock() {
                store.retain(|_, entry| {
                    if entry.tmux_session == session_name {
                        entry.cleanup_bg_windows();
                        log::info!(
                            "Cleaned up session '{}' on tmux session-closed.",
                            session_name
                        );
                        false
                    } else {
                        true
                    }
                });
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // N14: after-new-session hook — auto-install per-session hooks for new sessions.
        Request::NotifySessionCreated { session_name } => {
            let hook_exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "daemoneye".to_string());
            crate::daemon::install_session_hooks(&session_name, &hook_exe);
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // N15: client-detached hook — record detach time on all sessions for this tmux session.
        Request::NotifyClientDetached { session_name } => {
            let now = Instant::now();
            if let Ok(mut store) = sessions.lock() {
                for entry in store.values_mut() {
                    if entry.tmux_session == session_name {
                        entry.last_detach = Some(now);
                        entry.messages_at_detach = entry.messages.len();
                    }
                }
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // N15: client-attached hook — clear pending detach state so no catch-up brief fires.
        Request::NotifyClientAttached { session_name } => {
            if let Ok(mut store) = sessions.lock() {
                for entry in store.values_mut() {
                    if entry.tmux_session == session_name {
                        entry.last_detach = None;
                    }
                }
            }
            send_response_split(&mut tx, Response::Ok).await?;
            return Ok(());
        }
        // N8: client-resized hook — update cached viewport dimensions immediately.
        Request::NotifyResize {
            width,
            height,
            session_name: _,
        } => {
            cache.set_client_size(width, height);
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
        .unwrap_or_else(|| bg_session.lock().unwrap_or_log().clone());

    // Adopt the session if the daemon doesn't have one yet (systemd startup case).
    if !session_name.is_empty() {
        let mut current = bg_session.lock().unwrap_or_log();
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
            session_id
                .as_ref()
                .map(|id| read_session_file(id))
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();

    // Upsert the session entry: create it with the client-resolved target pane if
    // new, or refresh chat_pane and adopt client_target_pane if not yet set.
    // Also capture any pending catch-up brief (N15) to send after SessionInfo.
    let catchup_brief: Option<String> = if let Some(ref id) = session_id {
        if let Ok(mut store) = sessions.lock() {
            let entry = store.entry(id.clone()).or_insert_with(|| SessionEntry {
                messages: Vec::new(),
                last_accessed: Instant::now(),
                chat_pane: chat_pane.clone(),
                default_target_pane: client_target_pane.clone(),
                bg_windows: Vec::new(),
                last_prompt_tokens: 0,
                tmux_session: session_name.clone(),
                last_detach: None,
                messages_at_detach: 0,
                pipe_source_pane: None,
            });
            entry.chat_pane = chat_pane.clone();
            entry.tmux_session = session_name.clone();
            if entry.default_target_pane.is_none() {
                entry.default_target_pane = client_target_pane.clone();
            }

            // R1: start pipe-pane for the source pane on the first Ask so we can
            // capture full terminal output history (including content that has scrolled
            // past the tmux scrollback buffer).  Best-effort — falls back to
            // capture-pane silently if pipe-pane is unavailable.
            if entry.pipe_source_pane.is_none() {
                if let Some(ref pane_id) = client_pane {
                    match crate::tmux::start_pipe_pane(pane_id) {
                        Ok(_) => {
                            entry.pipe_source_pane = Some(pane_id.clone());
                        }
                        Err(e) => {
                            log::warn!("R1: could not start pipe-pane for {}: {}", pane_id, e);
                        }
                    }
                }
            }

            // N15: generate a catch-up brief if the client was detached and new
            // messages arrived while no terminal was attached (background jobs,
            // webhook alerts, watchdog results, etc.).
            let brief = entry.last_detach.and_then(|detach_time| {
                let away_secs = detach_time.elapsed().as_secs();
                let new_msgs =
                    &entry.messages[entry.messages_at_detach.min(entry.messages.len())..];
                build_catchup_brief(new_msgs, away_secs)
            });

            // Clear detach state regardless of whether we generated a brief.
            entry.last_detach = None;

            brief
        } else {
            None
        }
    } else {
        None
    };

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
    let last_prompt_tokens = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.last_prompt_tokens))
        .unwrap_or(0);

    let safe_query = mask_sensitive(&initial_query);

    // First turn: include full host context + terminal snapshot.
    // Subsequent turns: budget note + query only. The AI calls get_terminal_context
    // when it needs a fresh snapshot, keeping mid-turn messages lean.
    let prompt = if is_first_turn {
        let session_summary =
            cache.get_labeled_context(client_pane.as_deref(), chat_pane.as_deref());
        let sys_ctx = get_or_init_sys_context().format_for_ai();
        let daemon_host = daemon_hostname();
        let environment = &config.context.environment;
        let pane_location = client_pane
            .as_deref()
            .and_then(get_pane_remote_host)
            .map(|h| format!("REMOTE — {}", h))
            .unwrap_or_else(|| format!("LOCAL — same host as daemon ({})", daemon_host));
        let width_hint = chat_width
            .map(|w| format!("\n- Chat display width: {w} columns (write prose as continuous paragraphs; the terminal word-wraps automatically — do not insert hard line breaks within paragraphs)"))
            .unwrap_or_default();
        let memory_block = crate::memory::load_session_memory_block();
        let manifest_block = crate::manifest::build_knowledge_manifest();
        let auto_search_block = crate::manifest::auto_search_context(&safe_query, &session_summary);
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
             {manifest_block}\
             {auto_search_block}\
             ## Terminal Session\n```\n{session_summary}\n```\n\n\
             User: {safe_query}"
        )
    } else {
        // Inject a context-budget line so the AI knows how much context it has consumed.
        // Use percentage thresholds so the signal is meaningful regardless of model.
        let context_window = config.ai.context_window();
        let pct_used = if context_window > 0 {
            (last_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
        } else {
            0
        };
        let budget_note = if pct_used >= 75 {
            format!(
                "[Token Budget] Context at {}k / {}k tokens ({}% used) — NEAR LIMIT. Be very concise. Suggest `/clear` if the task is complete.\n\n",
                last_prompt_tokens / 1000,
                context_window / 1000,
                pct_used
            )
        } else if pct_used >= 50 {
            format!(
                "[Token Budget] Context at {}k / {}k tokens ({}% used) — prefer concise responses, avoid redundant tool calls.\n\n",
                last_prompt_tokens / 1000,
                context_window / 1000,
                pct_used
            )
        } else if last_prompt_tokens > 0 {
            format!(
                "[Token Budget] Context at {}k / {}k tokens ({}% used).\n\n",
                last_prompt_tokens / 1000,
                context_window / 1000,
                pct_used
            )
        } else {
            String::new()
        };
        format!("{budget_note}User: {safe_query}")
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

    send_response_split(
        &mut tx,
        Response::SessionInfo {
            message_count: history_count,
        },
    )
    .await?;

    // N15: send catch-up brief as a SystemMsg immediately after SessionInfo so
    // it appears before any streaming tokens from the AI.
    if let Some(ref brief) = catchup_brief {
        send_response_split(&mut tx, Response::SystemMsg(brief.clone())).await?;
    }

    loop {
        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        let client_instance = make_client(
            &config.ai.provider,
            config.ai.resolve_api_key(),
            config.ai.model.clone(),
            config.ai.effective_base_url(),
        );
        let sys_prompt_turn = sys_prompt.clone();
        let messages_clone = messages.clone();

        tokio::spawn(async move {
            if let Err(e) = client_instance
                .chat(&sys_prompt_turn, messages_clone, ai_tx.clone())
                .await
            {
                let _ = ai_tx.send(AiEvent::Error(e.to_string()));
            }
        });

        let mut full_response = String::new();
        let mut pending_calls: Vec<PendingCall> = Vec::new();

        loop {
            let event = match tokio::time::timeout(std::time::Duration::from_secs(30), ai_rx.recv())
                .await
            {
                Ok(Some(ev)) => ev,
                Ok(None) => break,
                Err(_timeout) => {
                    // No token in 30 s — send a keep-alive so the client doesn't
                    // hit its per-token deadline (slow local LLMs can stall longer).
                    send_response_split(&mut tx, Response::KeepAlive).await?;
                    continue;
                }
            };
            match event {
                AiEvent::Token(t) => {
                    full_response.push_str(&t);
                    send_response_split(&mut tx, Response::Token(t)).await?;
                }
                AiEvent::ToolCall(id, cmd, bg, target, retry, thought_signature) => {
                    if bg {
                        pending_calls.push(PendingCall::Background {
                            id,
                            cmd,
                            _credential: None,
                            retry_pane: retry,
                            thought_signature: thought_signature.clone(),
                        });
                    } else {
                        pending_calls.push(PendingCall::Foreground {
                            id,
                            cmd,
                            target,
                            thought_signature: thought_signature.clone(),
                        });
                    }
                }
                AiEvent::ScheduleCommand {
                    id,
                    name,
                    command,
                    is_script,
                    run_at,
                    interval,
                    runbook,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ScheduleCommand {
                        id,
                        name,
                        command,
                        is_script,
                        run_at,
                        interval,
                        runbook,
                        thought_signature,
                    });
                }
                AiEvent::ListSchedules {
                    id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ListSchedules {
                        id,
                        thought_signature,
                    });
                }
                AiEvent::CancelSchedule {
                    id,
                    job_id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::CancelSchedule {
                        id,
                        job_id,
                        thought_signature,
                    });
                }
                AiEvent::DeleteSchedule {
                    id,
                    job_id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::DeleteSchedule {
                        id,
                        job_id,
                        thought_signature,
                    });
                }
                AiEvent::WriteScript {
                    id,
                    script_name,
                    content,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::WriteScript {
                        id,
                        script_name,
                        content,
                        thought_signature,
                    });
                }
                AiEvent::ListScripts {
                    id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ListScripts {
                        id,
                        thought_signature,
                    });
                }
                AiEvent::ReadScript {
                    id,
                    script_name,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ReadScript {
                        id,
                        script_name,
                        thought_signature,
                    });
                }
                AiEvent::DeleteScript {
                    id,
                    script_name,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::DeleteScript {
                        id,
                        script_name,
                        thought_signature,
                    });
                }
                AiEvent::WatchPane {
                    id,
                    pane_id,
                    timeout_secs,
                    pattern,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::WatchPane {
                        id,
                        pane_id,
                        timeout_secs,
                        pattern,
                        thought_signature,
                    });
                }
                AiEvent::ReadFile {
                    id,
                    path,
                    offset,
                    limit,
                    pattern,
                    target_pane,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ReadFile {
                        id,
                        thought_signature,
                        path,
                        offset,
                        limit,
                        pattern,
                        target_pane,
                    });
                }
                AiEvent::EditFile {
                    id,
                    path,
                    old_string,
                    new_string,
                    target_pane,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::EditFile {
                        id,
                        thought_signature,
                        path,
                        old_string,
                        new_string,
                        target_pane,
                    });
                }
                AiEvent::WriteRunbook {
                    id,
                    name,
                    content,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::WriteRunbook {
                        id,
                        thought_signature,
                        name,
                        content,
                    });
                }
                AiEvent::DeleteRunbook {
                    id,
                    name,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::DeleteRunbook {
                        id,
                        thought_signature,
                        name,
                    });
                }
                AiEvent::ReadRunbook {
                    id,
                    name,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ReadRunbook {
                        id,
                        thought_signature,
                        name,
                    });
                }
                AiEvent::ListRunbooks {
                    id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ListRunbooks {
                        id,
                        thought_signature,
                    });
                }
                AiEvent::AddMemory {
                    id,
                    key,
                    value,
                    category,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::AddMemory {
                        id,
                        thought_signature,
                        key,
                        value,
                        category,
                    });
                }
                AiEvent::DeleteMemory {
                    id,
                    key,
                    category,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::DeleteMemory {
                        id,
                        thought_signature,
                        key,
                        category,
                    });
                }
                AiEvent::ReadMemory {
                    id,
                    key,
                    category,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ReadMemory {
                        id,
                        thought_signature,
                        key,
                        category,
                    });
                }
                AiEvent::ListMemories {
                    id,
                    category,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ListMemories {
                        id,
                        thought_signature,
                        category,
                    });
                }
                AiEvent::SearchRepository {
                    id,
                    query,
                    kind,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::SearchRepository {
                        id,
                        thought_signature,
                        query,
                        kind,
                    });
                }
                AiEvent::GetTerminalContext {
                    id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::GetTerminalContext {
                        id,
                        thought_signature,
                    });
                }
                AiEvent::ListPanes {
                    id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ListPanes {
                        id,
                        thought_signature,
                    });
                }
                AiEvent::CloseBackgroundWindow {
                    id,
                    pane_id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::CloseBackgroundWindow {
                        id,
                        thought_signature,
                        pane_id,
                    });
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

                        log_event(
                            "ai_turn",
                            serde_json::json!({
                                "session": session_id.as_deref().unwrap_or("-"),
                                "model": config.ai.model,
                                "prompt_tokens": usage.prompt_tokens,
                                "completion_tokens": usage.completion_tokens,
                            }),
                        );

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
                        send_response_split(
                            &mut tx,
                            Response::UsageUpdate {
                                prompt_tokens: usage.prompt_tokens,
                            },
                        )
                        .await?;
                        send_response_split(&mut tx, Response::Ok).await?;
                        return Ok(());
                    }

                    log_event(
                        "ai_turn",
                        serde_json::json!({
                            "session": session_id.as_deref().unwrap_or("-"),
                            "model": config.ai.model,
                            "prompt_tokens": usage.prompt_tokens,
                            "completion_tokens": usage.completion_tokens,
                        }),
                    );

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
                    let mut tool_call_counts: std::collections::HashMap<&'static str, u32> =
                        std::collections::HashMap::new();

                    let mut tool_results = Vec::new();
                    let mut user_message_redirect: Option<String> = None;
                    for (call_idx, call) in pending_calls.iter().enumerate() {
                        let call_id = call.id().to_string();

                        // Hard total cap: block all calls beyond the limit.
                        if call_idx as u32 >= MAX_TOTAL_CALLS {
                            log::warn!(
                                "Turn tool-call total limit ({MAX_TOTAL_CALLS}) reached; blocking call {}",
                                call_idx + 1
                            );
                            tool_results.push(ToolResult {
                                tool_call_id: call_id,
                                tool_name: call.tool_name().to_string(),
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
                            log::warn!(
                                "Per-tool limit for `{tool_name}` reached ({MAX_SAME_TOOL}); blocking call"
                            );
                            tool_results.push(ToolResult {
                                tool_call_id: call_id,
                                tool_name: tool_name.to_string(),
                                content: format!(
                                    "Error: `{tool_name}` has been called {MAX_SAME_TOOL} times \
                                     this turn. This call was not executed. Proceed with the \
                                     information already gathered and do not call this tool again."
                                ),
                            });
                            continue;
                        }

                        let outcome = match crate::daemon::executor::execute_tool_call(
                            call,
                            &mut tx,
                            &mut rx,
                            session_id.as_deref(),
                            &session_name,
                            chat_pane.as_deref(),
                            &cache,
                            &sessions,
                            &schedule_store,
                        )
                        .await
                        {
                            Ok(res) => res,
                            Err(_) => return Ok(()),
                        };

                        match outcome {
                            crate::daemon::executor::ToolCallOutcome::UserMessage(text) => {
                                // User typed a corrective message at the approval prompt.
                                // Abort the tool chain: pop the assistant message we just pushed
                                // (it referenced tool calls that will never complete), then inject
                                // the user's text as a plain user turn so the AI can course-correct.
                                user_message_redirect = Some(text);
                                break;
                            }
                            crate::daemon::executor::ToolCallOutcome::Result(result) => {
                                tool_results.push(ToolResult {
                                    tool_call_id: call_id,
                                    tool_name: tool_name.to_string(),
                                    content: result,
                                });
                            }
                        }
                    }

                    // If the user interrupted with a message, discard the tool chain and inject
                    // the message as a new user turn instead.
                    if let Some(user_msg) = user_message_redirect {
                        messages.pop(); // remove the assistant message that listed the tool calls
                        messages.push(Message {
                            role: "user".to_string(),
                            content: user_msg,
                            tool_calls: None,
                            tool_results: None,
                        });
                        break; // restart outer loop — AI will see the user message next
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
                                tool_name: r.tool_name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;

    fn msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
        }
    }

    // ── build_catchup_brief ───────────────────────────────────────────────────

    #[test]
    fn catchup_brief_none_when_away_less_than_30s() {
        let msgs = vec![msg("[Background Task Completed] deploy finished")];
        assert!(build_catchup_brief(&msgs, 29).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_new_messages() {
        assert!(build_catchup_brief(&[], 120).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_matching_events() {
        let msgs = vec![
            msg("User: what is load avg?"),
            msg("The load average is 0.5"),
        ];
        assert!(build_catchup_brief(&msgs, 120).is_none());
    }

    #[test]
    fn catchup_brief_detects_background_task() {
        let msgs = vec![msg(
            "[Background Task Completed] apt upgrade finished (exit 0)",
        )];
        let brief = build_catchup_brief(&msgs, 60).expect("should produce a brief");
        assert!(brief.contains("[Catch-up]"), "missing header: {brief}");
        assert!(
            brief.contains("[Background Task Completed]"),
            "missing event: {brief}"
        );
        assert!(brief.contains("1m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_webhook_alert() {
        let msgs = vec![msg("[Webhook Alert] Disk usage at 92% on web01")];
        let brief = build_catchup_brief(&msgs, 3600).expect("should produce a brief");
        assert!(brief.contains("[Webhook Alert]"), "missing event: {brief}");
        assert!(brief.contains("1h0m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watchdog() {
        let msgs = vec![msg("[Watchdog] nginx: 5xx rate above threshold")];
        let brief = build_catchup_brief(&msgs, 90).expect("should produce a brief");
        assert!(brief.contains("[Watchdog]"), "missing event: {brief}");
        assert!(brief.contains("1m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watch_pane() {
        let msgs = vec![msg("[Watch Pane %3] pattern 'ready' matched after 45s")];
        let brief = build_catchup_brief(&msgs, 120).expect("should produce a brief");
        assert!(brief.contains("[Watch Pane"), "missing event: {brief}");
    }

    #[test]
    fn catchup_brief_counts_events_correctly() {
        let msgs = vec![
            msg("[Background Task Completed] job1 (exit 0)"),
            msg("User: check this"),
            msg("[Webhook Alert] CPU spike on prod"),
            msg("[Background Task Completed] job2 (exit 1)"),
        ];
        let brief = build_catchup_brief(&msgs, 200).expect("should produce a brief");
        assert!(brief.contains("3 events"), "expected count 3: {brief}");
    }

    #[test]
    fn catchup_brief_singular_event_label() {
        let msgs = vec![msg("[Webhook Alert] single alert")];
        let brief = build_catchup_brief(&msgs, 60).expect("should produce a brief");
        assert!(brief.contains("1 event "), "expected singular: {brief}");
        assert!(!brief.contains("1 events"), "should be singular: {brief}");
    }

    #[test]
    fn catchup_brief_extracts_first_line_only() {
        let msgs = vec![msg(
            "[Background Task Completed] job done\nFull output:\nline 1\nline 2",
        )];
        let brief = build_catchup_brief(&msgs, 60).expect("should produce a brief");
        // Only the first line should appear as the bullet
        assert!(
            brief.contains("[Background Task Completed] job done"),
            "missing first line: {brief}"
        );
        assert!(
            !brief.contains("Full output:"),
            "should not include subsequent lines: {brief}"
        );
    }

    #[test]
    fn catchup_brief_away_time_hours_minutes() {
        let msgs = vec![msg("[Watchdog] alert")];
        let brief = build_catchup_brief(&msgs, 7260).expect("should produce a brief");
        // 7260 s = 2h1m
        assert!(brief.contains("2h1m"), "expected 2h1m: {brief}");
    }

    // ── is_valid_pane_id ──────────────────────────────────────────────────────

    #[test]
    fn valid_pane_ids_accepted() {
        assert!(is_valid_pane_id("%0"));
        assert!(is_valid_pane_id("%1"));
        assert!(is_valid_pane_id("%23"));
        assert!(is_valid_pane_id("%999"));
    }

    #[test]
    fn invalid_pane_ids_rejected() {
        assert!(!is_valid_pane_id(""));
        assert!(!is_valid_pane_id("%")); // no digits
        assert!(!is_valid_pane_id("0")); // no leading %
        assert!(!is_valid_pane_id("%0a")); // non-digit character
        assert!(!is_valid_pane_id("%23\x1b[31m")); // ANSI escape injection
        assert!(!is_valid_pane_id("%;rm -rf /")); // shell injection attempt
    }
}
