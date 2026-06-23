use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use crate::config::default_socket_path;
use crate::config::{Config, load_named_prompt};
use crate::cost::CostAttribution;
use crate::daemon::prompt::{PromptCtx, build_first_turn_prompt, build_subsequent_turn_prompt};
use crate::daemon::session::*;
use crate::daemon::stream;
use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use libc;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::BufReader;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite};
use tokio::net::UnixStream;

/// Build the N15 catch-up brief from messages injected while the client was away.
///
/// `new_msgs` is the slice of messages added after detach.
/// `away_secs` is how long the client was gone.
/// `detach_time_utc` is the UTC wall-clock time of the detach, used to query
/// cost events from `events.jsonl` (Phase 7).
/// Returns `None` when the absence was too short or no relevant events occurred.
/// Validate that a pane_id received from an external hook matches the tmux
/// format `%<digits>` (e.g. `%0`, `%23`).  Rejects anything else so that
/// crafted hook payloads cannot inject escape sequences or unexpected strings
/// into the cache or broadcast channels.
pub(crate) fn is_valid_pane_id(id: &str) -> bool {
    id.starts_with('%') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Build the N15 catch-up brief from messages injected while the client was away.
///
/// `new_msgs` is the slice of messages added after detach.
/// `away_secs` is how long the client was gone.
/// `detach_time_utc` is the UTC wall-clock time of the detach, used to query
/// cost events from `events.jsonl` (Phase 7).
/// Returns `None` when the absence was too short or no relevant events occurred.
pub(crate) fn build_catchup_brief(
    new_msgs: &[crate::ai::Message],
    away_secs: u64,
    detach_time_utc: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<String> {
    // Skip if the user was away less than 30 s — too brief to be useful.
    if away_secs < 30 {
        return None;
    }

    let away_str = if away_secs < 60 {
        format!("{}s", away_secs)
    } else if away_secs < 3600 {
        format!("{}m", away_secs / 60)
    } else {
        format!("{}h{}m", away_secs / 3600, (away_secs % 3600) / 60)
    };

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

    // Compute cost during the detach window (Phase 7).
    let cost_line = detach_time_utc.and_then(|detach_time| {
        let now = chrono::Utc::now();
        let summary = crate::daemon::utils::sum_cost_between(detach_time, now);
        if summary.call_count == 0 {
            // No AI calls during detach — omit cost line entirely.
            return None;
        }
        let total = summary.total_cost_usd;
        let marker = if summary.has_untracked { "+" } else { "" };
        let agent_detail = if total < 0.001 {
            // All costs are zero (local providers only).
            "local providers only".to_string()
        } else {
            summary
                .by_agent
                .iter()
                .map(|(name, cost)| format!("{} ${:.2}", name, cost))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        Some(format!(
            "Cost during detach: ${:.2}{} ({})",
            total, marker, agent_detail
        ))
    });

    let has_events = !events.is_empty();
    let has_cost = cost_line.is_some();

    if !has_events && !has_cost {
        return None;
    }

    let count = events.len();
    let mut parts = Vec::new();

    if has_events {
        let lines = events
            .iter()
            .map(|e| format!("  • {}", e))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!(
            "[Catch-up] {} event{} while you were away ({}):\n{}",
            count,
            if count == 1 { "" } else { "s" },
            away_str,
            lines,
        ));
        if let Some(cost) = cost_line {
            parts.push(format!("  • {}", cost));
        }
    } else if let Some(cost) = cost_line {
        parts.push(format!(
            "[Catch-up] AI activity while you were away ({}):\n  • {}",
            away_str, cost
        ));
    }

    Some(parts.join("\n"))
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

    match request {
        Request::Ping => {
            handle_ping(&mut tx).await?;
        }
        Request::Shutdown => {
            handle_shutdown(&mut tx).await?;
            return Ok(());
        }
        Request::Refresh => {
            handle_refresh(&mut tx).await?;
        }
        Request::SetModel {
            session_id,
            model: model_name,
        } => {
            handle_set_model(&mut tx, &sessions, &config, session_id, model_name).await?;
        }
        Request::ListModels { session_id } => {
            handle_list_models(&mut tx, &sessions, &config, session_id).await?;
        }
        Request::SetPane {
            session_id,
            pane_id,
        } => {
            handle_set_pane(&mut tx, &sessions, &cache, session_id, pane_id).await?;
        }
        Request::ListPanesForSession { session_id } => {
            handle_list_panes(&mut tx, &sessions, &cache, session_id).await?;
        }
        Request::Status => {
            handle_status(&mut tx, &sessions, &schedule_store, &config).await?;
        }
        Request::QueryLimits { session_id: sid } => {
            handle_query_limits(&mut tx, &sessions, &config, sid).await?;
        }
        Request::ResetSessionToolCount { session_id: sid } => {
            handle_reset_tool_count(&mut tx, &sessions, sid).await?;
        }
        Request::SaveSession {
            session_id: sid,
            name,
            description,
            force,
        } => {
            handle_save_session(&mut tx, &sessions, sid, name, description, force).await?;
        }
        Request::LoadSession {
            session_id: sid,
            name,
            force,
        } => {
            handle_load_session(&mut tx, &sessions, &config, sid, name, force).await?;
        }
        Request::ListSavedSessions => {
            handle_list_saved_sessions(&mut tx).await?;
        }
        Request::DeleteSavedSession { name } => {
            handle_delete_saved_session(&mut tx, name).await?;
        }
        Request::RenameSavedSession { old_name, new_name } => {
            handle_rename_saved_session(&mut tx, &sessions, old_name, new_name).await?;
        }

        Request::NotifyActivity { pane_id, .. } => {
            crate::daemon::hook::handle_notify_activity(&mut tx, &pane_id).await?;
        }
        Request::NotifyComplete {
            pane_id, exit_code, ..
        } => {
            crate::daemon::hook::handle_notify_complete(&mut tx, &pane_id, exit_code).await?;
        }
        Request::NotifyFocus { pane_id, .. } => {
            crate::daemon::hook::handle_notify_focus(&cache, &mut tx, &pane_id).await?;
        }
        Request::NotifyWindowChanged { .. } => {
            crate::daemon::hook::handle_notify_window_changed(&cache, &mut tx).await?;
        }
        Request::NotifySessionClosed { session_name } => {
            crate::daemon::hook::handle_notify_session_closed(
                Arc::clone(&sessions),
                Arc::clone(&cache),
                Arc::clone(&managed_session),
                Arc::clone(&bg_session),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifySessionCreated { session_name } => {
            crate::daemon::hook::handle_notify_session_created(&mut tx, session_name).await?;
        }
        Request::NotifyClientDetached { session_name } => {
            crate::daemon::hook::handle_notify_client_detached(
                Arc::clone(&sessions),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifyClientAttached { session_name } => {
            crate::daemon::hook::handle_notify_client_attached(
                Arc::clone(&sessions),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifyResize { width, height, .. } => {
            crate::daemon::hook::handle_notify_resize(&cache, &mut tx, width, height).await?;
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
        } => {
            handle_ask(
                query,
                tmux_pane,
                session_id,
                chat_pane,
                prompt,
                chat_width,
                tmux_session,
                target_pane,
                &mut tx,
                &mut rx,
                cache,
                &sessions,
                schedule_store,
                bg_session,
                &config,
            )
            .await?;
            return Ok(());
        }
        _ => {}
    }

    Ok(())
}

// ── Quick-return request handlers ─────────────────────────────────────────────

async fn handle_ping<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

async fn handle_shutdown<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    send_response_split(tx, Response::Ok).await?;
    // SAFETY: Graceful self-signal to trigger the tokio signal handler. No safe
    // wrapper exists in the Rust stdlib for sending a signal to self.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGTERM);
    }
    Ok(())
}

async fn handle_refresh<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    crate::sys_context::refresh_sys_context();
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

// ── Model management handlers ────────────────────────────────────────────────

async fn handle_set_model<W>(
    tx: &mut W,
    sessions: &SessionStore,
    config: &Config,
    session_id: String,
    model_name: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let available = config.available_models();
    if available.contains(&model_name.as_str()) {
        if let Ok(mut store) = sessions.lock()
            && let Some(entry) = store.get_mut(&session_id)
        {
            entry.active_model = Some(model_name.clone());
        }
        send_response_split(tx, Response::ModelChanged { model: model_name }).await?;
    } else {
        let list = available.join(", ");
        send_response_split(
            tx,
            Response::Error(format!(
                "Unknown model '{model_name}'. Configured models: {list}"
            )),
        )
        .await?;
    }
    Ok(())
}

async fn handle_list_models<W>(
    tx: &mut W,
    sessions: &SessionStore,
    config: &Config,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
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
    send_response_split(tx, Response::ModelList { models, active }).await?;
    Ok(())
}

// ── Pane management handlers ─────────────────────────────────────────────────

async fn handle_set_pane<W>(
    tx: &mut W,
    sessions: &SessionStore,
    cache: &SessionCache,
    session_id: String,
    pane_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if !is_valid_pane_id(&pane_id) {
        send_response_split(
            tx,
            Response::Error(format!(
                "Invalid pane ID '{}'. Use the format %N (e.g. %3).",
                pane_id
            )),
        )
        .await?;
        return Ok(());
    }
    if let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(&session_id)
    {
        entry.default_target_pane = Some(pane_id.clone());
        crate::pane_prefs::save(&entry.tmux_session, &pane_id);
    }
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
        tx,
        Response::PaneChanged {
            pane_id,
            description,
        },
    )
    .await?;
    Ok(())
}

async fn handle_list_panes<W>(
    tx: &mut W,
    sessions: &SessionStore,
    cache: &SessionCache,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
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
        tx,
        Response::PaneList {
            panes: panes_snapshot,
        },
    )
    .await?;
    Ok(())
}

// ── Status / limits handlers ─────────────────────────────────────────────────

async fn handle_status<W>(
    tx: &mut W,
    sessions: &SessionStore,
    schedule_store: &ScheduleStore,
    config: &Config,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let uptime_secs = crate::daemon::daemon_uptime_secs();
    let pid = std::process::id();
    let mut active_sessions = 0;
    let mut active_prompt_tokens = 0;
    let mut total_turns = 0;
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
    if let Ok(memories) = crate::memory::list_memories(None, &["global"]) {
        for (_, cat, _) in memories {
            *memory_breakdown.entry(cat).or_insert(0) += 1;
        }
    }

    let active_entry = config.resolve_model(status_active_model.as_deref());
    let context_window_tokens = active_entry.context_window();
    let compactions = crate::daemon::stats::get_compactions();
    let compaction_ratio = crate::daemon::stats::get_compaction_ratio();

    // Compute today's cost aggregation (cached for 5s).
    let cost_today = crate::daemon::stats::compute_cost_today();

    // Collect per-session cost totals from active sessions.
    let session_costs: Vec<(String, f64)> = if let Ok(sess_map) = sessions.lock() {
        sess_map
            .iter()
            .map(|(id, entry)| (id.clone(), entry.cost_usd))
            .collect()
    } else {
        Vec::new()
    };

    send_response_split(
        tx,
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
            active_agents: {
                let mut agents: Vec<(String, String)> = Vec::new();
                if let Ok(sess_map) = sessions.lock() {
                    for entry in sess_map.values() {
                        if let Some(ref gc) = entry.ghost_config
                            && let Some(ref agent_name) = gc.agent
                        {
                            let job_id = entry
                                .ghost_task_message
                                .as_deref()
                                .unwrap_or("unknown")
                                .chars()
                                .take(40)
                                .collect();
                            agents.push((agent_name.clone(), job_id));
                        }
                    }
                }
                agents.sort_by(|a, b| a.0.cmp(&b.0));
                agents
            },
            daemon_session_costs: session_costs,
            daemon_total_cost_today_usd: cost_today.total_cost_usd,
            daemon_cost_by_provider: cost_today.cost_by_provider,
            daemon_cost_by_agent: cost_today.cost_by_agent,
        },
    )
    .await?;
    Ok(())
}

async fn handle_query_limits<W>(
    tx: &mut W,
    sessions: &SessionStore,
    config: &Config,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let (turn_count, tool_calls_this_session, history_len) = if let Ok(store) = sessions.lock()
        && let Some(entry) = store.get(&session_id)
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
        tx,
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
    Ok(())
}

async fn handle_reset_tool_count<W>(
    tx: &mut W,
    sessions: &SessionStore,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(&session_id)
    {
        entry.tool_calls_this_session = 0;
        log::info!(
            "Session {}: per-session tool call counter reset",
            session_id
        );
    }
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

// ── Named session CRUD handlers ──────────────────────────────────────────────

async fn handle_save_session<W>(
    tx: &mut W,
    sessions: &SessionStore,
    session_id: String,
    name: String,
    description: String,
    force: bool,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let (msgs, turn_count, current_saved, model_name, artifacts) = if let Ok(store) =
        sessions.lock()
        && let Some(entry) = store.get(&session_id)
    {
        (
            entry.messages.clone(),
            entry.turn_count,
            entry.saved_name.clone(),
            entry
                .active_model
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            entry.artifacts_created.clone(),
        )
    } else {
        (Vec::new(), 0, None, "default".to_string(), Vec::new())
    };

    match crate::session_store::save_session(
        &name,
        current_saved.as_deref(),
        &description,
        &msgs,
        turn_count,
        &model_name,
        &artifacts,
        force,
    ) {
        Ok(()) => {
            if current_saved.is_none() && !artifacts.is_empty() {
                let errs = crate::session_store::backfill_session_origin(&artifacts, &name);
                if !errs.is_empty() {
                    log::warn!(
                        "Session {}: backfill_session_origin failed for: {}",
                        session_id,
                        errs.join(", ")
                    );
                }
            }
            if let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(&session_id)
            {
                entry.saved_name = Some(name.clone());
                entry.dirty = false;
            }
            log::info!("Session {}: saved as '{}'", session_id, name);
            send_response_split(tx, Response::SessionSaved { name }).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

async fn handle_load_session<W>(
    tx: &mut W,
    sessions: &SessionStore,
    config: &Config,
    session_id: String,
    name: String,
    force: bool,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let is_dirty = if let Ok(store) = sessions.lock()
        && let Some(entry) = store.get(&session_id)
    {
        entry.dirty
    } else {
        false
    };
    if is_dirty && !force {
        send_response_split(
            tx,
            Response::Error(format!(
                "session has unsaved changes; run `/session save <name>` first, \
                 or use `/session load {} --force` to discard them",
                name
            )),
        )
        .await?;
        return Ok(());
    }

    if !crate::session_store::session_exists(&name) {
        send_response_split(
            tx,
            Response::Error(format!("no saved session named '{}'", name)),
        )
        .await?;
        return Ok(());
    }

    let load_count = config.sessions.load_recent_turns;
    match (
        crate::session_store::load_session_meta(&name),
        crate::session_store::load_session_messages(&name, load_count),
    ) {
        (Ok(meta), Ok(loaded_msgs)) => {
            let banner = crate::session_store::build_resumed_banner(&meta, loaded_msgs.len());
            let loaded_count = loaded_msgs.len();
            let turn_count = meta.turn_count;

            if let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(&session_id)
            {
                entry.messages = loaded_msgs;
                entry.saved_name = Some(name.clone());
                entry.dirty = false;
            }
            log::info!(
                "Session {}: loaded '{}' ({} messages)",
                session_id,
                name,
                loaded_count
            );
            send_response_split(
                tx,
                Response::SessionLoaded {
                    name,
                    message_count: loaded_count,
                    turn_count,
                    banner,
                },
            )
            .await?;
        }
        (Err(e), _) | (_, Err(e)) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

async fn handle_list_saved_sessions<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use crate::session_store::{list_sessions, load_session_meta};
    let sessions_list: Vec<crate::ipc::SessionSummary> = list_sessions()
        .into_iter()
        .map(|(name, idx)| {
            let (description, turn_count, message_count, artifact_count) = load_session_meta(&name)
                .map(|m| {
                    (
                        m.description,
                        m.turn_count,
                        m.message_count,
                        m.artifacts_created.len(),
                    )
                })
                .unwrap_or_default();
            crate::ipc::SessionSummary {
                name,
                description,
                created_at: idx.created_at,
                last_updated: idx.last_updated,
                turn_count,
                message_count,
                artifact_count,
            }
        })
        .collect();
    send_response_split(
        tx,
        Response::SavedSessionList {
            sessions: sessions_list,
        },
    )
    .await?;
    Ok(())
}

async fn handle_delete_saved_session<W>(tx: &mut W, name: String) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match crate::session_store::delete_session(&name) {
        Ok(()) => {
            log::info!("Saved session '{}' deleted", name);
            send_response_split(tx, Response::Ok).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

async fn handle_rename_saved_session<W>(
    tx: &mut W,
    sessions: &SessionStore,
    old_name: String,
    new_name: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match crate::session_store::rename_session(&old_name, &new_name) {
        Ok(()) => {
            if let Ok(mut store) = sessions.lock() {
                for entry in store.values_mut() {
                    if entry.saved_name.as_deref() == Some(old_name.as_str()) {
                        entry.saved_name = Some(new_name.clone());
                    }
                }
            }
            log::info!("Saved session '{}' renamed to '{}'", old_name, new_name);
            send_response_split(tx, Response::Ok).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

// ── Ask handler ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
// TODO(M2): consolidate params into a struct
async fn handle_ask<W, R>(
    initial_query: String,
    client_pane: Option<String>,
    session_id: Option<String>,
    chat_pane: Option<String>,
    prompt_override: Option<String>,
    chat_width: Option<usize>,
    client_tmux_session: Option<String>,
    client_target_pane: Option<String>,
    tx: &mut W,
    rx: &mut R,
    cache: Arc<SessionCache>,
    sessions: &SessionStore,
    schedule_store: Arc<ScheduleStore>,
    bg_session: Arc<std::sync::Mutex<String>>,
    config: &Config,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
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
            let mem = sessions.lock().unwrap_or_log();
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
    let (catchup_brief, pane_drift_msg, session_cost_usd, has_untracked_cost): (
        Option<String>,
        Option<String>,
        f64,
        bool,
    ) = if let Some(ref id) = session_id {
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
                detach_time_utc: None,
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
                saved_name: None,
                dirty: false,
                artifacts_created: Vec::new(),
                auto_name_suggested: false,
                ghost_task_message: None,
                loaded_tools: std::collections::HashSet::new(),
                cost_usd: 0.0,
                cost_by_agent: std::collections::HashMap::new(),
                has_untracked_cost: false,
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
                build_catchup_brief(new_msgs, away_secs, entry.detach_time_utc)
            });

            // Clear detach state regardless of whether we generated a brief.
            entry.last_detach = None;

            let cost_usd = entry.cost_usd;
            let has_untracked = entry.has_untracked_cost;
            (brief, drift_msg, cost_usd, has_untracked)
        } else {
            (None, None, 0.0, false)
        }
    } else {
        (None, None, 0.0, false)
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
        && let Some(turn_limit) = crate::config::LimitsConfig::cap_usize(config.limits.max_turns)
        && this_turn_count > turn_limit
    {
        send_response_split(
            tx,
            Response::Error(format!(
                "Session turn limit ({turn_limit}) reached. \
                 Start a new session to continue."
            )),
        )
        .await?;
        return Ok(());
    }

    let safe_query = mask_sensitive(&initial_query);

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

    // Build the prompt using the prompt module.
    let memory_namespaces_owned = crate::daemon::executor::build_memory_namespaces(
        session_id.as_deref(),
        sessions,
        is_ghost_session,
    );
    let memory_namespaces: Vec<&str> = memory_namespaces_owned.iter().map(|s| s.as_str()).collect();
    let tool_policy_owned: Option<crate::agents::ToolPolicy> = session_id.as_ref().and_then(|id| {
        let store = sessions.lock().ok()?;
        let entry = store.get(id)?;
        if !entry.is_ghost {
            return None;
        }
        entry
            .ghost_config
            .as_ref()
            .and_then(|gc| gc.tool_policy.clone())
    });
    let agent_name_owned: Option<String> = session_id.as_ref().and_then(|id| {
        let store = sessions.lock().ok()?;
        let entry = store.get(id)?;
        if !entry.is_ghost {
            return None;
        }
        entry.ghost_config.as_ref().and_then(|gc| gc.agent.clone())
    });
    let (is_ghost_session, parent_job_id_owned): (bool, Option<String>) = session_id
        .as_ref()
        .and_then(|id| {
            let store = sessions.lock().ok()?;
            let entry = store.get(id)?;
            Some((
                entry.is_ghost,
                entry
                    .ghost_config
                    .as_ref()
                    .and_then(|gc| gc.parent_job_id.clone()),
            ))
        })
        .unwrap_or((false, None));
    let cost_attribution = CostAttribution {
        agent_name: agent_name_owned
            .clone()
            .unwrap_or_else(|| "chat".to_string()),
        is_ghost: is_ghost_session,
        parent_job_id: parent_job_id_owned,
    };
    let prompt_ctx = PromptCtx {
        client_pane: client_pane.as_deref(),
        chat_pane: chat_pane.as_deref(),
        default_target_pane: default_target_pane.as_deref(),
        cache: &cache,
        config,
        chat_width,
        safe_query: &safe_query,
        last_prompt_tokens,
        history_count: messages.len(),
        this_turn_count,
        ghost_turn_limit,
        inject_snapshot,
        memory_namespaces: &memory_namespaces,
        tool_policy: tool_policy_owned.as_ref(),
        agent_name: agent_name_owned.as_deref(),
    };

    let prompt = if is_first_turn {
        build_first_turn_prompt(&prompt_ctx)
    } else {
        build_subsequent_turn_prompt(&prompt_ctx)
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
        tx,
        Response::SessionInfo {
            message_count: history_count,
            turn_count: this_turn_count,
            session_cost_usd,
            has_untracked_cost,
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
            tx,
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
        send_response_split(tx, Response::SystemMsg(brief.clone())).await?;
    }

    // Pane drift: notify the model when the foreground target changed since
    // the last turn so it doesn't keep using the stale pane ID.
    if let Some(ref msg) = pane_drift_msg {
        send_response_split(tx, Response::SystemMsg(msg.clone())).await?;
    }

    // ── Conversation loop ─────────────────────────────────────────────────────
    stream::run_conversation_loop(
        tx,
        rx,
        session_id,
        &session_name,
        chat_pane,
        messages,
        sys_prompt,
        session_active_model,
        is_ghost_session,
        this_turn_count,
        post_trim_len,
        needs_compaction,
        config,
        cache,
        Arc::clone(sessions),
        schedule_store,
        cost_attribution,
    )
    .await
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
        assert!(build_catchup_brief(&msgs, 29, None).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_new_messages() {
        assert!(build_catchup_brief(&[], 120, None).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_matching_events() {
        let msgs = vec![
            msg("User: what is load avg?"),
            msg("The load average is 0.5"),
        ];
        assert!(build_catchup_brief(&msgs, 120, None).is_none());
    }

    #[test]
    fn catchup_brief_detects_background_task() {
        let msgs = vec![msg(
            "[Background Task Completed] apt upgrade finished (exit 0)",
        )];
        let brief = build_catchup_brief(&msgs, 60, None).expect("should produce a brief");
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
        let brief = build_catchup_brief(&msgs, 3600, None).expect("should produce a brief");
        assert!(brief.contains("[Webhook Alert]"), "missing event: {brief}");
        assert!(brief.contains("1h0m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watchdog() {
        let msgs = vec![msg("[Watchdog] nginx: 5xx rate above threshold")];
        let brief = build_catchup_brief(&msgs, 90, None).expect("should produce a brief");
        assert!(brief.contains("[Watchdog]"), "missing event: {brief}");
        assert!(brief.contains("1m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watch_pane() {
        let msgs = vec![msg("[Watch Pane %3] pattern 'ready' matched after 45s")];
        let brief = build_catchup_brief(&msgs, 120, None).expect("should produce a brief");
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
        let brief = build_catchup_brief(&msgs, 200, None).expect("should produce a brief");
        assert!(brief.contains("3 events"), "expected count 3: {brief}");
    }

    #[test]
    fn catchup_brief_singular_event_label() {
        let msgs = vec![msg("[Webhook Alert] single alert")];
        let brief = build_catchup_brief(&msgs, 60, None).expect("should produce a brief");
        assert!(brief.contains("1 event "), "expected singular: {brief}");
        assert!(!brief.contains("1 events"), "should be singular: {brief}");
    }

    #[test]
    fn catchup_brief_extracts_first_line_only() {
        let msgs = vec![msg(
            "[Background Task Completed] job done\nFull output:\nline 1\nline 2",
        )];
        let brief = build_catchup_brief(&msgs, 60, None).expect("should produce a brief");
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
        let brief = build_catchup_brief(&msgs, 7260, None).expect("should produce a brief");
        // 7260 s = 2h1m
        assert!(brief.contains("2h1m"), "expected 2h1m: {brief}");
    }

    // ── Phase 7: catch-up brief cost integration ──────────────────────────────

    #[test]
    fn catchup_brief_includes_cost_when_ghosts_ran() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        let one_hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let thirty_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line1 = format!(
            r#"{{"event":"ai_cost","ts":"{one_hour_ago}","session_id":"gs-1","agent_name":"architect","cost":{{"total_cost_usd":0.20}}}}"#
        );
        let line2 = format!(
            r#"{{"event":"ai_cost","ts":"{thirty_min_ago}","session_id":"gs-2","agent_name":"ghost-anonymous","cost":{{"total_cost_usd":0.14}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n{}\n", line1, line2)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(2);
        let msgs = vec![msg("[Ghost Shell Completed] architect finished")];
        let brief =
            build_catchup_brief(&msgs, 7200, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("Cost during detach:"),
            "missing cost line: {brief}"
        );
        assert!(brief.contains("$0.34"), "wrong total: {brief}");
        assert!(brief.contains("architect"), "missing agent: {brief}");
        assert!(
            brief.contains("ghost-anonymous"),
            "missing ghost agent: {brief}"
        );
    }

    #[test]
    fn catchup_brief_omits_cost_line_when_no_ai_calls() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        // Write a non-ai_cost event — should not trigger cost line.
        let ts = chrono::Utc::now().to_rfc3339();
        let line = format!(r#"{{"event":"command","ts":"{ts}","session":"s1","cmd":"ls"}}"#);
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs = vec![msg("[Background Task Completed] job done")];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            !brief.contains("Cost during detach:"),
            "should omit cost line when no ai_cost events: {brief}"
        );
    }

    #[test]
    fn catchup_brief_local_only_shows_zero_explicitly() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        // Local provider call with zero cost.
        let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line = format!(
            r#"{{"event":"ai_cost","ts":"{ts}","session_id":"gs-local","agent_name":"chat","cost":{{"total_cost_usd":0.0}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs = vec![msg("[Ghost Shell Completed] local job done")];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("Cost during detach:"),
            "should show cost line: {brief}"
        );
        assert!(brief.contains("$0.00"), "should show zero cost: {brief}");
        assert!(
            brief.contains("local providers only"),
            "should indicate local providers: {brief}"
        );
    }

    #[test]
    fn catchup_brief_marks_untracked_spend() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        // Unknown pricing source — cost is zero but should be flagged.
        let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line = format!(
            r#"{{"event":"ai_cost","ts":"{ts}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":0.0}},"pricing_source":"Unknown"}}"#
        );
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs = vec![msg("[Background Task Completed] job done")];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("$0.00+"),
            "should have + marker for untracked: {brief}"
        );
    }

    #[test]
    fn catchup_brief_cost_only_has_header_when_no_events() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        // AI cost event during the detach window.
        let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line = format!(
            r#"{{"event":"ai_cost","ts":"{ts}","session_id":"gs-1","agent_name":"architect","cost":{{"total_cost_usd":0.34}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        // No injected event messages — only cost.
        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs: Vec<crate::ai::Message> = vec![];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("[Catch-up] AI activity while you were away"),
            "should have header when cost-only: {brief}"
        );
        assert!(
            brief.contains("Cost during detach:"),
            "should have cost line: {brief}"
        );
        assert!(
            brief.contains("architect"),
            "should show agent name: {brief}"
        );
    }

    #[test]
    fn sum_cost_between_excludes_events_outside_window() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        let now = chrono::Utc::now();
        let outside_before = (now - chrono::Duration::hours(3)).to_rfc3339();
        let inside = (now - chrono::Duration::hours(1)).to_rfc3339();
        let outside_after = (now + chrono::Duration::hours(1)).to_rfc3339();

        let line_before = format!(
            r#"{{"event":"ai_cost","ts":"{outside_before}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":9.99}}}}"#
        );
        let line_inside = format!(
            r#"{{"event":"ai_cost","ts":"{inside}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":0.50}}}}"#
        );
        let line_after = format!(
            r#"{{"event":"ai_cost","ts":"{outside_after}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":8.88}}}}"#
        );
        std::fs::write(
            &events_path,
            format!("{}\n{}\n{}\n", line_before, line_inside, line_after),
        )
        .unwrap();

        let from = now - chrono::Duration::hours(2);
        let to = now;
        let summary = crate::daemon::utils::sum_cost_between(from, to);

        assert!(
            (summary.total_cost_usd - 0.50).abs() < 1e-10,
            "should only include inside-window event, got {}",
            summary.total_cost_usd
        );
        assert_eq!(summary.call_count, 1, "should have exactly 1 call");
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
