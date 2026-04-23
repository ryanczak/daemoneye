use crate::ai::filter::mask_sensitive;
use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::default_socket_path;
use crate::config::{Config, load_named_prompt};
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use crate::scheduler::ScheduleStore;
use crate::sys_context::get_or_init_sys_context;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use libc;
use std::sync::Arc;
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

/// Prepend a `[FOREGROUND TARGET]` line to the session context block.
///
/// This pins the model to a specific pane ID for foreground execution so it
/// never has to infer the target from topology.  If `target_pane` is `None`
/// or the pane is not in the cache, returns the context unchanged.
fn prepend_foreground_target(
    ctx: &str,
    target_pane: Option<&str>,
    cache: &crate::tmux::cache::SessionCache,
) -> String {
    let Some(pane_id) = target_pane else {
        return ctx.to_string();
    };
    let (cmd, window) = {
        let panes = cache.panes.read().unwrap_or_log();
        if let Some(p) = panes.get(pane_id) {
            (p.current_cmd.clone(), p.window_name.clone())
        } else {
            (String::new(), String::new())
        }
    };
    let detail = if !cmd.is_empty() && !window.is_empty() {
        format!(" ({}, '{}')", cmd, window)
    } else if !cmd.is_empty() {
        format!(" ({})", cmd)
    } else {
        String::new()
    };
    format!(
        "[FOREGROUND TARGET] {}{} — for run_terminal_command(background=false), always pass target_pane=\"{}\"\n{}",
        pane_id, detail, pane_id, ctx
    )
}

/// Build the N15 catch-up brief from messages injected while the client was away.
///
/// `new_msgs` is the slice of messages added after detach.
/// `away_secs` is how long the client was gone.
/// Returns `None` when the absence was too short or no relevant events occurred.
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
                || c.contains("[Ghost Shell Started]")
                || c.contains("[Ghost Shell Completed]")
                || c.contains("[Ghost Shell Failed]")
            {
                // Extract just the first line as a terse summary.
                let first_line = c.lines().next().unwrap_or(c.as_str()).trim();
                Some(first_line.to_string())
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
    managed_session: Arc<Option<String>>,
) -> Result<()> {
    let config = Config::load().unwrap_or_else(|_| {
        log::warn!("Failed to load config, using defaults");
        Config::default()
    });

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
            model: _ask_model,
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
        Request::SetModel {
            session_id,
            model: model_name,
        } => {
            let available = config.available_models();
            if available.contains(&model_name.as_str()) {
                if let Ok(mut store) = sessions.lock()
                    && let Some(entry) = store.get_mut(&session_id)
                {
                    entry.active_model = Some(model_name.clone());
                }
                send_response_split(&mut tx, Response::ModelChanged { model: model_name }).await?;
            } else {
                let list = available.join(", ");
                send_response_split(
                    &mut tx,
                    Response::Error(format!(
                        "Unknown model '{model_name}'. Configured models: {list}"
                    )),
                )
                .await?;
            }
            return Ok(());
        }
        Request::ListModels { session_id } => {
            let models: Vec<(String, String)> = config
                .available_models()
                .into_iter()
                .map(|key| {
                    let model_id = config.resolve_model(Some(key)).model.clone();
                    (key.to_string(), model_id)
                })
                .collect();
            let active = if let Ok(store) = sessions.lock()
                && let Some(entry) = store.get(&session_id)
                && let Some(ref m) = entry.active_model
            {
                m.clone()
            } else {
                "default".to_string()
            };
            send_response_split(&mut tx, Response::ModelList { models, active }).await?;
            return Ok(());
        }
        Request::SetPane {
            session_id,
            pane_id,
        } => {
            // Validate it looks like a tmux pane ID.
            if !is_valid_pane_id(&pane_id) {
                send_response_split(
                    &mut tx,
                    Response::Error(format!(
                        "Invalid pane ID '{}'. Use the format %N (e.g. %3).",
                        pane_id
                    )),
                )
                .await?;
                return Ok(());
            }
            // Update the session entry.
            if let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(&session_id)
            {
                entry.default_target_pane = Some(pane_id.clone());
                // Persist so the preference survives daemon restarts.
                crate::pane_prefs::save(&entry.tmux_session, &pane_id);
            }
            // Build a human-readable description from the cache.
            let (cmd, window) = {
                let panes = cache.panes.read().unwrap_or_log();
                panes
                    .get(&pane_id)
                    .map(|p| (p.current_cmd.clone(), p.window_name.clone()))
                    .unwrap_or_default()
            };
            let description = if !cmd.is_empty() && !window.is_empty() {
                format!("{} ({})", pane_id, cmd)
            } else {
                pane_id.clone()
            };
            send_response_split(
                &mut tx,
                Response::PaneChanged {
                    pane_id,
                    description,
                },
            )
            .await?;
            return Ok(());
        }
        Request::ListPanesForSession { session_id } => {
            let current_target = if let Ok(store) = sessions.lock() {
                store
                    .get(&session_id)
                    .and_then(|e| e.default_target_pane.clone())
            } else {
                None
            };
            let chat_pane_id: Option<String> = if let Ok(store) = sessions.lock() {
                store.get(&session_id).and_then(|e| e.chat_pane.clone())
            } else {
                None
            };
            let panes_snapshot = {
                let panes = cache.panes.read().unwrap_or_log();
                let mut entries: Vec<_> = panes
                    .iter()
                    .filter(|(id, _)| chat_pane_id.as_deref() != Some(id.as_str()))
                    .filter(|(_, s)| {
                        !s.window_name.starts_with("de-bg-")
                            && !s.window_name.starts_with("de-sj-")
                            && !s.window_name.starts_with("de-gs-bg-")
                            && !s.window_name.starts_with("de-gs-sj-")
                            && !s.window_name.starts_with("de-gs-ir-")
                    })
                    .filter(|(id, _)| crate::tmux::pane_exists(id))
                    .map(|(id, s)| {
                        let is_target = current_target.as_deref() == Some(id.as_str());
                        (
                            id.clone(),
                            s.current_cmd.clone(),
                            s.window_name.clone(),
                            s.pane_index,
                            is_target,
                        )
                    })
                    .collect();
                entries.sort_by_key(|(_, _, win, idx, _)| (win.clone(), *idx));
                entries
            };
            send_response_split(
                &mut tx,
                Response::PaneList {
                    panes: panes_snapshot,
                },
            )
            .await?;
            return Ok(());
        }
        // F1: return a live status snapshot to `daemoneye status`.
        Request::Status => {
            let uptime_secs = crate::daemon::daemon_uptime_secs();
            let pid = std::process::id();
            let mut active_sessions = 0;
            let mut active_prompt_tokens = 0;
            let mut total_turns = 0;
            // Model name from the most recently accessed non-ghost session, if any.
            let mut status_active_model: Option<String> = None;
            if let Ok(sess_map) = sessions.lock() {
                active_sessions = sess_map.len();
                active_prompt_tokens = sess_map.values().map(|s| s.last_prompt_tokens).sum();
                total_turns = sess_map.values().map(|s| s.turn_count).sum();
                status_active_model = sess_map
                    .values()
                    .filter(|s| !s.is_ghost)
                    .max_by_key(|s| s.last_accessed)
                    .and_then(|s| s.active_model.clone());
            }
            let schedule_count = schedule_store.list().len();

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
            let ghosts_launched = crate::daemon::stats::get_ghosts_launched();
            let ghosts_active = crate::daemon::stats::get_ghosts_active();
            let ghosts_completed = crate::daemon::stats::get_ghosts_completed();
            let ghosts_failed = crate::daemon::stats::get_ghosts_failed();
            let webhooks_received = crate::daemon::stats::get_webhooks_received();

            let webhooks_rejected = crate::daemon::stats::get_webhooks_rejected();
            let webhook_url = format!(
                "http://{}:{}/webhook",
                config.webhook.bind_addr, config.webhook.port
            );
            let recent_commands = crate::daemon::stats::get_recent_commands();

            let runbook_count = crate::runbook::list_runbooks()
                .map(|v| v.len())
                .unwrap_or(0);
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

            let active_entry = config.resolve_model(status_active_model.as_deref());
            let context_window_tokens = active_entry.context_window();
            let compactions = crate::daemon::stats::get_compactions();
            let compaction_ratio = crate::daemon::stats::get_compaction_ratio();

            send_response_split(
                &mut tx,
                Response::DaemonStatus {
                    uptime_secs,
                    pid,
                    active_sessions,
                    total_turns,
                    provider: active_entry.provider.clone(),
                    model: active_entry.model.clone(),
                    available_models: config
                        .available_models()
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect(),
                    socket_path: default_socket_path().display().to_string(),
                    schedule_count,
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
                    ghosts_launched,
                    ghosts_active,
                    ghosts_completed,
                    ghosts_failed,
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
                    compactions,
                    compaction_ratio,
                    scripts_approved: crate::daemon::stats::get_scripts_approved(),
                    scripts_denied: crate::daemon::stats::get_scripts_denied(),
                    runbooks_approved: crate::daemon::stats::get_runbooks_approved(),
                    runbooks_denied: crate::daemon::stats::get_runbooks_denied(),
                    file_edits_approved: crate::daemon::stats::get_file_edits_approved(),
                    file_edits_denied: crate::daemon::stats::get_file_edits_denied(),
                    limits: {
                        let mut overrides: Vec<(String, u32)> = config
                            .limits
                            .per_tool
                            .iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect();
                        overrides.sort_by(|a, b| a.0.cmp(&b.0));
                        crate::ipc::LimitsSummary {
                            per_tool_batch: config.limits.per_tool_batch,
                            total_tool_calls_per_turn: config.limits.total_tool_calls_per_turn,
                            tool_result_chars: config.limits.tool_result_chars,
                            max_history: config.limits.max_history,
                            max_turns: config.limits.max_turns,
                            max_tool_calls_per_session: config.limits.max_tool_calls_per_session,
                            per_tool_overrides: overrides,
                        }
                    },
                },
            )
            .await?;
            return Ok(());
        }
        Request::QueryLimits { session_id: sid } => {
            let (turn_count, tool_calls_this_session, history_len) = if let Ok(store) =
                sessions.lock()
                && let Some(entry) = store.get(&sid)
            {
                (
                    entry.turn_count,
                    entry.tool_calls_this_session,
                    entry.messages.len(),
                )
            } else {
                (0, 0, 0)
            };
            let mut overrides: Vec<(String, u32)> = config
                .limits
                .per_tool
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            overrides.sort_by(|a, b| a.0.cmp(&b.0));
            send_response_split(
                &mut tx,
                Response::LimitsInfo {
                    limits: crate::ipc::LimitsSummary {
                        per_tool_batch: config.limits.per_tool_batch,
                        total_tool_calls_per_turn: config.limits.total_tool_calls_per_turn,
                        tool_result_chars: config.limits.tool_result_chars,
                        max_history: config.limits.max_history,
                        max_turns: config.limits.max_turns,
                        max_tool_calls_per_session: config.limits.max_tool_calls_per_session,
                        per_tool_overrides: overrides,
                    },
                    turn_count,
                    tool_calls_this_session,
                    history_len,
                },
            )
            .await?;
            return Ok(());
        }
        Request::ResetSessionToolCount { session_id: sid } => {
            if let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(&sid)
            {
                entry.tool_calls_this_session = 0;
                log::info!("Session {}: per-session tool call counter reset", sid);
            }
            send_response_split(&mut tx, Response::Ok).await?;
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
                    && let Some(cmd_id) = map.remove(&pane_id)
                {
                    crate::daemon::stats::finish_command(cmd_id, exit_code);
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
            // If this was the daemon-managed session, recreate it so ghost shells
            // and the scheduler can resume without a daemon restart.
            // Per-session hooks are installed automatically via the NotifySessionCreated
            // path once tmux fires the after-new-session global hook.
            if managed_session.as_deref() == Some(session_name.as_str()) {
                match std::process::Command::new("tmux")
                    .args(["new-session", "-d", "-s", &session_name])
                    .output()
                {
                    Ok(o) if o.status.success() => {
                        log::info!(
                            "Recreated managed tmux session '{}' after close.",
                            session_name
                        );
                        *bg_session.lock().unwrap_or_log() = session_name.clone();
                        cache.set_session(&session_name);
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        log::warn!(
                            "Failed to recreate managed session '{}': {}",
                            session_name,
                            stderr
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "tmux new-session for managed session '{}' failed: {}",
                            session_name,
                            e
                        );
                    }
                }
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
    // to whatever the daemon adopted at startup.
    let session_name: String = client_tmux_session
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| bg_session.lock().unwrap_or_log().clone());

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
                .map(|id| {
                    read_session_file(
                        id,
                        crate::config::LimitsConfig::cap_usize(config.limits.max_history),
                    )
                })
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();

    // Upsert the session entry: create it with the client-resolved target pane if
    // new, or refresh chat_pane and adopt client_target_pane if not yet set.
    // Also capture any pending catch-up brief (N15) and pane-drift notice to
    // send after SessionInfo.
    let (catchup_brief, pane_drift_msg): (Option<String>, Option<String>) =
        if let Some(ref id) = session_id {
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
                    is_ghost: false,
                    ghost_config: None,
                    ghost_bg_prefix: crate::daemon::GS_BG_WINDOW_PREFIX,
                    started_at: chrono::Utc::now(),
                    turn_count: 0,
                    tool_calls_this_session: 0,
                    active_model: None,
                    last_snapshot_activity: 0,
                });
                entry.chat_pane = chat_pane.clone();
                entry.tmux_session = session_name.clone();

                // Detect pane drift: the client resolved a different target pane than
                // what was stored.  Announce the change to the model as a SystemMsg so
                // it doesn't keep using the old pane ID.  Always adopt the new value —
                // resolve_target_pane() on the client already respects pane_prefs.json,
                // so if the user pinned a pane via /pane it will persist correctly.
                let drift_msg = match (&entry.default_target_pane, &client_target_pane) {
                    (Some(old), Some(new)) if old != new => {
                        let old_clone = old.clone();
                        entry.default_target_pane = Some(new.clone());
                        Some(format!(
                            "[Pane target changed] Foreground target is now {} (was {}). \
                             Use target_pane=\"{}\" for run_terminal_command(background=false).",
                            new, old_clone, new
                        ))
                    }
                    (None, Some(new)) => {
                        entry.default_target_pane = Some(new.clone());
                        None // first assignment — no drift to announce
                    }
                    _ => None,
                };

                // R1: start pipe-pane for the source pane on the first Ask so we can
                // capture full terminal output history (including content that has scrolled
                // past the tmux scrollback buffer).  Best-effort — falls back to
                // capture-pane silently if pipe-pane is unavailable.
                //
                // `pipe_source_pane = Some("")` is used as a "don't retry" sentinel:
                // it means we attempted and failed (or deliberately skipped), so we
                // fall back to capture-pane for all subsequent turns without retrying.
                if entry.pipe_source_pane.is_none()
                    && let Some(ref pane_id) = client_pane
                {
                    // Skip if client_pane == chat_pane: the chat pane runs the
                    // daemoneye UI, not the user's work.  Piping it is useless and
                    // can transiently fail immediately after split-window creates the
                    // pane (pty not yet fully initialized) causing repeated log noise.
                    let is_chat_pane = chat_pane.as_deref() == Some(pane_id.as_str());
                    if is_chat_pane {
                        log::debug!("R1: skipping pipe-pane for {} — same as chat pane", pane_id);
                        entry.pipe_source_pane = Some(String::new()); // don't retry
                    } else if crate::tmux::pane_exists(pane_id) {
                        match crate::tmux::start_pipe_pane(pane_id) {
                            Ok(_) => {
                                entry.pipe_source_pane = Some(pane_id.clone());
                            }
                            Err(e) => {
                                // Pane existed at check time but was gone by the time
                                // pipe-pane ran (TOCTOU race) — don't retry this session.
                                log::debug!("R1: could not start pipe-pane for {}: {}", pane_id, e);
                                entry.pipe_source_pane = Some(String::new()); // don't retry
                            }
                        }
                    } else {
                        log::debug!(
                            "R1: skipping pipe-pane for {} — pane no longer exists",
                            pane_id
                        );
                        entry.pipe_source_pane = Some(String::new()); // don't retry
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

                (brief, drift_msg)
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    // Read the session's active model override once so it stays consistent for
    // the whole turn (including the budget line and every AI loop iteration).
    let session_active_model: Option<String> = if let Some(ref id) = session_id
        && let Ok(store) = sessions.lock()
    {
        store.get(id.as_str()).and_then(|e| e.active_model.clone())
    } else {
        None
    };

    // Read last prompt token count up front — it drives the compaction decision
    // below and is also used later for the [BUDGET] line.
    let last_prompt_tokens = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.last_prompt_tokens))
        .unwrap_or(0);

    // Token-pressure-driven compaction.
    //
    // ELISION_PCT (50%) — elide oversized tool_results in old messages; cheap,
    //   preserves turn structure.
    // DIGEST_PCT  (60%) — build a structured digest and drop old messages.
    // Safety net — if we hit MAX_HISTORY regardless of token info, still digest.
    //
    // All paths require `messages.len() >= DIGEST_THRESHOLD` so a token-heavy
    // first turn (huge system context + memory) doesn't trigger compaction
    // before any real history exists.
    const ELISION_PCT: u32 = 50;
    const DIGEST_PCT: u32 = 60;
    let context_window = config
        .resolve_model(session_active_model.as_deref())
        .context_window();
    let token_pct = if context_window > 0 {
        (last_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
    } else {
        0
    };
    let pre_trim_len = messages.len();
    let history_cap = crate::config::LimitsConfig::cap_usize(config.limits.max_history);
    let at_safety_cap = history_cap.is_some_and(|cap| messages.len() >= cap);
    use crate::daemon::digest::DIGEST_THRESHOLD;
    let above_floor = messages.len() >= DIGEST_THRESHOLD;
    let should_digest = above_floor && (token_pct >= DIGEST_PCT || at_safety_cap);
    let should_elide_only = !should_digest && above_floor && token_pct >= ELISION_PCT;

    if should_digest {
        // Elide first — it's cheap and gives the digest pass smaller tool
        // outputs to reason about.
        let elided = crate::daemon::digest::elide_old_tool_results(&mut messages);
        let started_at = session_id
            .as_ref()
            .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.started_at));
        if let Some(since) = started_at {
            // Hybrid digest (task #4): when enabled in config, ask a cheap
            // model to turn the about-to-be-dropped turns into a short
            // narrative before we replace them with the structured tally.
            // Uses the `digest` model entry if configured, otherwise falls
            // back to `default`.  Best-effort — if the call fails or times
            // out, the structured digest still fires.  Disabled by default
            // because it costs one extra API call per compaction pass.
            let narrative = if config.digest.narrative_enabled
                && let Some(tail_start) = crate::daemon::digest::planned_tail_start(&messages)
                && tail_start > 1
            {
                let slice = &messages[1..tail_start];
                let model_entry = config.resolve_model(Some("digest"));
                crate::daemon::digest::build_narrative_summary(slice, model_entry).await
            } else {
                None
            };
            let has_narrative = narrative.is_some();
            let digest = crate::daemon::digest::build_session_digest(
                session_id.as_deref().unwrap_or("-"),
                since,
                messages.len(),
                narrative.as_deref(),
            );
            messages = crate::daemon::digest::compact_with_digest(messages, &digest);
            log::info!(
                "Compaction (digest): tokens {}% — elided {} chars, narrative={}, compacted {} → {} messages",
                token_pct,
                elided,
                if has_narrative { "yes" } else { "no" },
                pre_trim_len,
                messages.len()
            );
        } else {
            messages = trim_history(messages, history_cap);
            log::info!(
                "Compaction (trim): tokens {}% — elided {} chars, no started_at, trimmed {} → {} messages",
                token_pct,
                elided,
                pre_trim_len,
                messages.len()
            );
        }
    } else if should_elide_only {
        let elided = crate::daemon::digest::elide_old_tool_results(&mut messages);
        if elided > 0 {
            log::info!(
                "Compaction (elide only): tokens {}% — elided {} chars from old tool results",
                token_pct,
                elided
            );
        }
    } else if history_cap.is_some_and(|cap| messages.len() > cap) {
        // Final safety trim — should be unreachable given the digest path above
        // also fires at the cap, but keep it as a guard.
        messages = trim_history(messages, history_cap);
    }
    // If the message vec shrank the on-disk file must be fully rewritten to
    // remove the stale entries.  Otherwise we can append-only at the end of
    // each turn.
    let needs_compaction = messages.len() < pre_trim_len || should_elide_only;
    let post_trim_len = messages.len();

    let is_first_turn = messages.is_empty();

    // Read the current turn count and increment it for this turn.  Never reset
    // by compaction — this gives the client a stable, ever-increasing indicator.
    let this_turn_count = session_id
        .as_ref()
        .and_then(|id| {
            sessions.lock().ok().map(|mut store| {
                if let Some(entry) = store.get_mut(id) {
                    entry.turn_count += 1;
                    entry.turn_count
                } else {
                    1
                }
            })
        })
        .unwrap_or(1);

    // Chat-session max_turns gate.  Ghost sessions have their own turn budget
    // enforced in ghost.rs via max_ghost_turns — this check is skipped for them.
    let is_ghost_session = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.is_ghost))
        .unwrap_or(false);
    if !is_ghost_session
        && let Some(turn_limit) =
            crate::config::LimitsConfig::cap_usize(config.limits.max_turns)
        && this_turn_count > turn_limit
    {
        send_response_split(
            &mut tx,
            Response::Error(format!(
                "Session turn limit ({turn_limit}) reached. \
                 Start a new session to continue."
            )),
        )
        .await?;
        return Ok(());
    }

    let safe_query = mask_sensitive(&initial_query);

    // Current time — injected on every turn so the AI always has ground truth
    // for scheduling and time-relative reasoning.
    let current_time_line = {
        use chrono::Local;
        let now_local = Local::now();
        let now_utc = now_local.to_utc();
        let tz_name = now_local.format("%Z").to_string();
        format!(
            "[Current time: {} UTC ({}: {})]\n",
            now_utc.format("%Y-%m-%d %H:%M:%S"),
            tz_name,
            now_local.format("%Y-%m-%d %H:%M:%S"),
        )
    };

    // First turn: include full host context + terminal snapshot.
    // Subsequent turns: budget note + query only, unless the foreground pane has
    // had new activity since the last snapshot — in which case a fresh snapshot is
    // auto-injected so the AI sees current output without needing get_terminal_context.
    // C1: per-turn pane map — always generated, available in all branches.
    let pane_map = cache.pane_map_summary(chat_pane.as_deref());

    // Read the default foreground target pane for this session so we can inject
    // an explicit [FOREGROUND TARGET] line into the context block.  This tells the
    // model exactly which pane ID to pass to run_terminal_command(background=false)
    // without needing to infer it from the topology.
    let default_target_pane: Option<String> = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id)?.default_target_pane.clone());

    // Activity-based snapshot refresh: compare the foreground pane's current
    // last_activity timestamp against the value recorded when we last injected a
    // snapshot.  If it has advanced, the pane received new output since then and
    // we inject a fresh snapshot automatically.
    let pane_activity: u64 = default_target_pane
        .as_deref()
        .and_then(|tp| {
            cache
                .panes
                .read()
                .unwrap_or_log()
                .get(tp)
                .map(|s| s.last_activity)
        })
        .unwrap_or(0);
    let last_snapshot_activity: u64 = session_id
        .as_ref()
        .and_then(|id| {
            sessions
                .lock()
                .ok()?
                .get(id)
                .map(|e| e.last_snapshot_activity)
        })
        .unwrap_or(0);
    let inject_snapshot =
        is_first_turn || (pane_activity > 0 && pane_activity > last_snapshot_activity);

    // Record the activity timestamp so the next turn can detect further changes.
    if inject_snapshot
        && pane_activity > 0
        && let Some(ref id) = session_id
        && let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(id)
    {
        entry.last_snapshot_activity = pane_activity;
    }

    let prompt = if is_first_turn {
        let session_summary =
            cache.get_labeled_context(client_pane.as_deref(), chat_pane.as_deref());
        let session_summary =
            prepend_foreground_target(&session_summary, default_target_pane.as_deref(), &cache);
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
             {pane_map}{current_time_line}User: {safe_query}"
        )
    } else {
        // Inject a unified [BUDGET] line covering every dimension the AI could be
        // constrained by this turn: turn count (ghost sessions only), message
        // history position, and prompt-token pressure.  Attached chat sessions
        // have no hard turn cap — the turn slot is omitted for them.
        let context_window = config
            .resolve_model(session_active_model.as_deref())
            .context_window();
        let token_pct = if context_window > 0 {
            (last_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
        } else {
            0
        };
        let history_count = messages.len() + 1; // + the user turn about to be pushed
        let history_cap_budget = crate::config::LimitsConfig::cap_usize(config.limits.max_history);
        let history_pct = history_cap_budget
            .map(|cap| (history_count as f64 / cap as f64 * 100.0) as u32)
            .unwrap_or(0);

        // Ghost sessions: resolve the effective turn cap (runbook value clamped
        // to daemon ceiling; 0 = use the ceiling).  Returns None for regular chat.
        let ghost_turn_limit: Option<usize> = session_id.as_ref().and_then(|id| {
            let store = sessions.lock().ok()?;
            let entry = store.get(id)?;
            if !entry.is_ghost {
                return None;
            }
            let ceiling = config.ghost.max_ghost_turns;
            let limit = entry
                .ghost_config
                .as_ref()
                .map(|gc| {
                    if gc.max_ghost_turns > 0 {
                        gc.max_ghost_turns.min(ceiling)
                    } else {
                        ceiling
                    }
                })
                .unwrap_or(ceiling);
            Some(limit)
        });
        let turn_pct = ghost_turn_limit
            .filter(|l| *l > 0)
            .map(|l| (this_turn_count as f64 / l as f64 * 100.0) as u32)
            .unwrap_or(0);

        let mut parts: Vec<String> = Vec::new();
        if let Some(limit) = ghost_turn_limit {
            parts.push(format!("turn {}/{}", this_turn_count, limit));
        }
        match history_cap_budget {
            Some(cap) => parts.push(format!("history {}/{}", history_count, cap)),
            None => parts.push(format!("history {} (no cap)", history_count)),
        };
        if context_window > 0 && last_prompt_tokens > 0 {
            parts.push(format!(
                "prompt {}k/{}k ({}%)",
                last_prompt_tokens / 1000,
                context_window / 1000,
                token_pct
            ));
        }

        let max_pct = turn_pct.max(history_pct).max(token_pct);
        let warning = if max_pct >= 75 {
            " — NEAR LIMIT. Summarize progress, persist critical state to memory, and wrap up."
        } else if max_pct >= 50 {
            " — approaching budget; prefer concise responses and avoid redundant tool calls."
        } else {
            ""
        };
        let budget_note = format!("[BUDGET] {}{}\n\n", parts.join(" · "), warning);
        let fg_target_line = default_target_pane
            .as_deref()
            .map(|tp| format!("[FOREGROUND TARGET] {} — target_pane=\"{}\" for run_terminal_command(background=false)\n", tp, tp))
            .unwrap_or_default();

        if inject_snapshot {
            // Pane had new activity since the last snapshot — auto-inject a fresh one.
            let session_summary =
                cache.get_labeled_context(client_pane.as_deref(), chat_pane.as_deref());
            let session_summary =
                prepend_foreground_target(&session_summary, default_target_pane.as_deref(), &cache);
            format!(
                "{budget_note}{fg_target_line}{pane_map}{current_time_line}\
                 [Terminal snapshot — auto-refreshed (pane activity detected)]\n\
                 ```\n{session_summary}\n```\n\nUser: {safe_query}"
            )
        } else {
            format!("{budget_note}{fg_target_line}{pane_map}{current_time_line}User: {safe_query}")
        }
    };

    let prompt_name = prompt_override.as_deref().unwrap_or(&config.ai.prompt);
    let sys_prompt = load_named_prompt(prompt_name).system;

    let history_count = messages.len();
    messages.push(Message {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_results: None,
        turn: Some(this_turn_count),
    });

    send_response_split(
        &mut tx,
        Response::SessionInfo {
            message_count: history_count,
            turn_count: this_turn_count,
        },
    )
    .await?;

    // Notify the user when compaction occurred so the turn counter reset is
    // not mysterious.  Sent before the catch-up brief so it appears first.
    if needs_compaction {
        let ratio = pre_trim_len as f64 / post_trim_len.max(1) as f64;
        log::info!(
            "Session {} history compacted: {} → {} messages ({:.1}:1)",
            session_id.as_deref().unwrap_or("-"),
            pre_trim_len,
            post_trim_len,
            ratio,
        );
        log_event(
            "compaction",
            serde_json::json!({
                "session": session_id.as_deref().unwrap_or("-"),
                "msgs_before": pre_trim_len,
                "msgs_after": post_trim_len,
                "ratio": (ratio * 10.0).round() / 10.0,
            }),
        );
        crate::daemon::stats::record_compaction(pre_trim_len, post_trim_len);
        send_response_split(
            &mut tx,
            Response::SystemMsg(format!(
                "↩ Session history compacted ({} messages → {}) — full context preserved in digest",
                pre_trim_len, post_trim_len
            )),
        )
        .await?;
    }

    // N15: send catch-up brief as a SystemMsg immediately after SessionInfo so
    // it appears before any streaming tokens from the AI.
    if let Some(ref brief) = catchup_brief {
        send_response_split(&mut tx, Response::SystemMsg(brief.clone())).await?;
    }

    // Pane drift: notify the model when the foreground target changed since
    // the last turn so it doesn't keep using the stale pane ID.
    if let Some(ref msg) = pane_drift_msg {
        send_response_split(&mut tx, Response::SystemMsg(msg.clone())).await?;
    }

    loop {
        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        let active_entry = config.resolve_model(session_active_model.as_deref());
        let client_instance = make_client(
            &active_entry.provider,
            active_entry.resolve_api_key(),
            active_entry.model.clone(),
            active_entry.effective_base_url(),
        );
        let sys_prompt_turn = sys_prompt.clone();
        let messages_clone = messages.clone();

        tokio::spawn(async move {
            if let Err(e) = client_instance
                .chat(&sys_prompt_turn, messages_clone, ai_tx.clone(), true)
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
                    ghost_runbook,
                    cron,
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
                        ghost_runbook,
                        cron,
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
                    operation,
                    old_string,
                    new_string,
                    content,
                    dest_path,
                    target_pane,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::EditFile {
                        id,
                        thought_signature,
                        path,
                        operation,
                        old_string,
                        new_string,
                        content,
                        dest_path,
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
                AiEvent::UpdateMemory {
                    id,
                    key,
                    category,
                    body,
                    append,
                    tags,
                    summary,
                    relates_to,
                    expires,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::UpdateMemory {
                        id,
                        thought_signature,
                        key,
                        category,
                        body,
                        append,
                        tags,
                        summary,
                        relates_to,
                        expires,
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

                AiEvent::SpawnGhost {
                    id,
                    runbook,
                    message,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::SpawnGhost {
                        id,
                        thought_signature,
                        runbook,
                        message,
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
                                turn: Some(this_turn_count),
                            });
                        }

                        log_event(
                            "ai_turn",
                            serde_json::json!({
                                "session": session_id.as_deref().unwrap_or("-"),
                                "model": config.resolve_model(session_active_model.as_deref()).model,
                                "prompt_tokens": usage.prompt_tokens,
                                "completion_tokens": usage.completion_tokens,
                            }),
                        );

                        // Persist the conversation for the next turn.
                        // In-memory: fast lookup within the same daemon run.
                        // On-disk: survives daemon restarts.
                        if let Some(ref id) = session_id {
                            if let Ok(mut store) = sessions.lock()
                                && let Some(entry) = store.get_mut(id)
                            {
                                entry.messages = messages.clone();
                                entry.last_accessed = Instant::now();
                                entry.last_prompt_tokens = usage.prompt_tokens;
                                if chat_pane.is_some() {
                                    entry.chat_pane = chat_pane.clone();
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
                            "model": config.resolve_model(None).model,
                            "prompt_tokens": usage.prompt_tokens,
                            "completion_tokens": usage.completion_tokens,
                        }),
                    );

                    // Update session token tracking so the budget line in the next
                    // prompt reflects the actual context size of this turn.
                    if let Some(ref id) = session_id
                        && let Ok(mut store) = sessions.lock()
                        && let Some(entry) = store.get_mut(id)
                    {
                        entry.last_prompt_tokens = usage.prompt_tokens;
                    }

                    // Push one assistant message listing all tool calls.
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: full_response.clone(),
                        tool_calls: Some(pending_calls.iter().map(|c| c.to_tool_call()).collect()),
                        tool_results: None,
                        turn: Some(this_turn_count),
                    });

                    // Per-turn tool-call loop guard.
                    // Approval-gated tools (run_terminal_command, edit_file, write_script, etc.)
                    // are always exempt — the user's per-call approval prompt is the gate.
                    // Keep in sync with LimitsConfig::validate()'s APPROVAL_GATED list in config.rs.
                    const APPROVAL_GATED: &[&str] = &[
                        "run_terminal_command",
                        "edit_file",
                        "write_script",
                        "write_runbook",
                        "schedule_command",
                        "spawn_ghost_shell",
                        "delete_script",
                        "delete_runbook",
                        "delete_schedule",
                    ];

                    let mut tool_call_counts: std::collections::HashMap<&'static str, u32> =
                        std::collections::HashMap::new();
                    let mut total_turn_call_count: u32 = 0;

                    let mut tool_results = Vec::new();
                    let mut user_message_redirect: Option<String> = None;
                    for call in pending_calls.iter() {
                        let call_id = call.id().to_string();

                        // Per-tool and per-turn total caps: only applied to no-approval batch tools.
                        // tool_budget_hint carries a one-line suffix appended to the result
                        // when the running count is close to the cap, so the AI sees the
                        // approach and can slow down before the hard block triggers.
                        let tool_name = call.tool_name();
                        let mut tool_budget_hint: Option<String> = None;
                        if !APPROVAL_GATED.contains(&tool_name) {
                            total_turn_call_count += 1;

                            // Per-session cumulative cap across all non-approval-gated tools.
                            if let Some(session_limit) = crate::config::LimitsConfig::cap_usize(
                                config.limits.max_tool_calls_per_session,
                            ) {
                                let session_tool_count = session_id
                                    .as_ref()
                                    .and_then(|id| {
                                        sessions
                                            .lock()
                                            .ok()?
                                            .get(id)
                                            .map(|e| e.tool_calls_this_session)
                                    })
                                    .unwrap_or(0);
                                if session_tool_count >= session_limit {
                                    log::warn!(
                                        "Per-session tool cap ({session_limit}) reached; \
                                         blocking `{tool_name}`"
                                    );
                                    tool_results.push(ToolResult {
                                        tool_call_id: call_id,
                                        tool_name: tool_name.to_string(),
                                        content: format!(
                                            "Error: the per-session tool call limit \
                                             ({session_limit}) has been reached. This call was \
                                             not executed. No further tool calls can be made in \
                                             this session."
                                        ),
                                    });
                                    continue;
                                }
                            }

                            // Per-turn total cap across all non-approval-gated tools.
                            if let Some(total_limit) = crate::config::LimitsConfig::cap_u32(
                                config.limits.total_tool_calls_per_turn,
                            ) && total_turn_call_count > total_limit
                            {
                                log::warn!(
                                    "Per-turn total tool cap ({total_limit}) reached; \
                                     blocking `{tool_name}`"
                                );
                                tool_results.push(ToolResult {
                                    tool_call_id: call_id,
                                    tool_name: tool_name.to_string(),
                                    content: format!(
                                        "Error: the per-turn total tool call limit \
                                         ({total_limit}) has been reached. This call was \
                                         not executed. Summarise what you have gathered \
                                         so far and stop calling tools this turn."
                                    ),
                                });
                                continue;
                            }

                            // Per-tool batch cap (same tool repeated within one turn).
                            if let Some(limit) = config.limits.per_tool_cap(tool_name) {
                                let count = tool_call_counts.entry(tool_name).or_insert(0);
                                *count += 1;
                                if *count > limit {
                                    log::warn!(
                                        "Per-tool limit for `{tool_name}` reached ({limit}); \
                                         blocking call"
                                    );
                                    tool_results.push(ToolResult {
                                        tool_call_id: call_id,
                                        tool_name: tool_name.to_string(),
                                        content: format!(
                                            "Error: `{tool_name}` has been called {limit} times \
                                             this turn. This call was not executed. Proceed with \
                                             the information already gathered and do not call this \
                                             tool again."
                                        ),
                                    });
                                    continue;
                                }
                                if *count * 2 >= limit {
                                    tool_budget_hint = Some(format!(
                                        "\n\n[note: {}/{} `{}` calls this turn — approaching per-turn cap]",
                                        *count, limit, tool_name
                                    ));
                                }
                            }
                        }

                        let outcome = match crate::daemon::executor::execute_tool_call(
                            call,
                            &mut tx,
                            &mut rx,
                            crate::daemon::executor::SessionCtx {
                                session_id: session_id.as_deref(),
                                session_name: &session_name,
                                chat_pane: chat_pane.as_deref(),
                                sessions: &sessions,
                            },
                            &cache,
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
                                let content = match tool_budget_hint {
                                    Some(hint) => format!("{}{}", result, hint),
                                    None => result,
                                };
                                tool_results.push(ToolResult {
                                    tool_call_id: call_id,
                                    tool_name: tool_name.to_string(),
                                    content,
                                });
                                // Bump per-session counter for non-approval-gated tools.
                                if !APPROVAL_GATED.contains(&tool_name)
                                    && let Some(id) = &session_id
                                    && let Ok(mut store) = sessions.lock()
                                    && let Some(entry) = store.get_mut(id)
                                {
                                    entry.tool_calls_this_session += 1;
                                }
                            }
                            crate::daemon::executor::ToolCallOutcome::SpawnGhostSession {
                                session_id: ghost_sid,
                                runbook_name: ghost_rb,
                                tool_result,
                            } => {
                                // Spawn the ghost turn loop in a background task from this
                                // Send-safe context, then return the tool result to the AI.
                                let ghost_sessions = Arc::clone(&sessions);
                                let ghost_cache = Arc::clone(&cache);
                                let ghost_store = Arc::clone(&schedule_store);
                                let ghost_config = config.clone();
                                let ghost_sid2 = ghost_sid.clone();
                                let ghost_rb2 = ghost_rb.clone();
                                tokio::spawn(async move {
                                    let session_log =
                                        crate::daemon::session::session_file(&ghost_sid2)
                                            .display()
                                            .to_string();
                                    match crate::daemon::ghost::trigger_ghost_turn(
                                        &ghost_sid2,
                                        &ghost_sessions,
                                        &ghost_config,
                                        &ghost_cache,
                                        &ghost_store,
                                    )
                                    .await
                                    {
                                        Ok(()) => {
                                            crate::webhook::inject_ghost_event(
                                                &ghost_sessions,
                                                &format!(
                                                    "[Ghost Shell Completed] AI-requested ghost shell finished for runbook: {} — session log: {}",
                                                    ghost_rb2, session_log
                                                ),
                                            );
                                        }
                                        Err(e) => {
                                            log::error!(
                                                "SpawnGhost: ghost turn failed for {}: {}",
                                                ghost_sid2,
                                                e
                                            );
                                            crate::daemon::stats::inc_ghosts_failed();
                                            crate::webhook::inject_ghost_event(
                                                &ghost_sessions,
                                                &format!(
                                                    "[Ghost Shell Failed] AI-requested ghost shell error for runbook: {} — {} — session log: {}",
                                                    ghost_rb2, e, session_log
                                                ),
                                            );
                                        }
                                    }
                                });
                                tool_results.push(ToolResult {
                                    tool_call_id: call_id,
                                    tool_name: tool_name.to_string(),
                                    content: tool_result,
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
                            turn: Some(this_turn_count),
                        });
                        break; // restart outer loop — AI will see the user message next
                    }

                    // Truncate tool results before storing in history.
                    // The full output was already delivered to the AI as the live result;
                    // only the history copy needs to be capped to prevent context bloat.
                    // Limit comes from config.limits.tool_result_chars (0 = no cap).
                    let result_char_cap =
                        crate::config::LimitsConfig::cap_usize(config.limits.tool_result_chars);
                    let history_results: Vec<ToolResult> = tool_results.into_iter().map(|r| {
                        match result_char_cap {
                            Some(cap) if r.content.len() > cap => {
                                // Snap to a valid UTF-8 char boundary.
                                let mut end = cap;
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
                            _ => r,
                        }
                    }).collect();

                    // Push one message with all results so message history is valid.
                    messages.push(Message {
                        role: "user".to_string(),
                        content: String::new(),
                        tool_calls: None,
                        tool_results: Some(history_results),
                        turn: Some(this_turn_count),
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
            turn: None,
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
