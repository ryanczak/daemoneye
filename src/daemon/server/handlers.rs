use super::catchup::is_valid_pane_id;
use crate::config::Config;
use crate::config::default_socket_path;
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::{PaneInfo, Response};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use libc;
// ── Quick-return request handlers ─────────────────────────────────────────────

pub(super) async fn handle_ping<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

pub(super) async fn handle_shutdown<W>(tx: &mut W) -> Result<()>
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

pub(super) async fn handle_refresh<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    crate::sys_context::refresh_sys_context();
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

// ── Model management handlers ────────────────────────────────────────────────

pub(super) async fn handle_set_model<W>(
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
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(&session_id) {
                entry.active_model = Some(model_name.clone());
            }
        });
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

pub(super) async fn handle_list_models<W>(
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
    let active = with_sessions(sessions, |store| {
        if let Some(entry) = store.get(&session_id)
            && let Some(ref m) = entry.active_model
        {
            m.clone()
        } else {
            "default".to_string()
        }
    });
    send_response_split(tx, Response::ModelList { models, active }).await?;
    Ok(())
}

// ── Pane management handlers ─────────────────────────────────────────────────

pub(super) async fn handle_set_pane<W>(
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
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(&session_id) {
            entry.default_target_pane = Some(pane_id.clone());
            crate::pane_prefs::save(&entry.tmux_session, &pane_id);
        }
    });
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

pub(super) async fn handle_list_panes<W>(
    tx: &mut W,
    sessions: &SessionStore,
    cache: &SessionCache,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let (current_target, chat_pane_id) = with_sessions(sessions, |store| {
        let entry = store.get(&session_id);
        (
            entry.and_then(|e| e.default_target_pane.clone()),
            entry.and_then(|e| e.chat_pane.clone()),
        )
    });
    // Phase 1 (locked): snapshot the candidates. No blocking work under the
    // read guard — `pane_exists` spawns a tmux subprocess per pane and used to
    // run here, holding the cache lock for the whole sweep.
    let panes_snapshot: Vec<PaneInfo> = {
        // Read session_name before the panes guard (M12 lock ordering).
        let home = cache.session_name.read().unwrap_or_log().clone();
        let panes = cache.panes.read().unwrap_or_log();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        panes
            .iter()
            .filter(|(_, s)| s.session_name == home)
            .filter(|(id, s)| {
                crate::daemon::is_targetable_pane(&s.window_name, id, chat_pane_id.as_deref())
            })
            .map(|(id, s)| {
                let is_target = current_target.as_deref() == Some(id.as_str());
                let activity_age_secs = if s.last_activity > 0 {
                    Some(now.saturating_sub(s.last_activity))
                } else {
                    None
                };
                let title = if !s.pane_title.is_empty() && s.pane_title != s.current_cmd {
                    crate::ai::filter::mask_sensitive(&s.pane_title)
                } else {
                    String::new()
                };
                let preview = crate::ai::filter::mask_sensitive(&crate::tmux::status::summarize(
                    s.status, &s.buffer,
                ));
                PaneInfo {
                    id: id.clone(),
                    idx: s.pane_index,
                    window: s.window_name.clone(),
                    session: s.session_name.clone(),
                    cmd: s.current_cmd.clone(),
                    cwd: s.current_path.clone(),
                    title,
                    status: s.status.to_string(),
                    activity_age_secs,
                    is_target,
                    preview,
                }
            })
            .collect()
    };

    // Phase 2 (unlocked): the liveness probe, one bounded tmux call per
    // candidate, off the runtime.
    let mut panes_snapshot = {
        let mut live = Vec::with_capacity(panes_snapshot.len());
        for candidate in panes_snapshot {
            let id = candidate.id.clone();
            let exists =
                crate::tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&id))
                    .await
                    .unwrap_or(false);
            if exists {
                live.push(candidate);
            }
        }
        live
    };
    panes_snapshot.sort_by_key(|p: &PaneInfo| (p.window.clone(), p.idx));
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

/// Sandbox runtime health and the containers this daemon owns. Collected off
/// the async runtime because it spawns the container runtime.
pub(super) async fn handle_container_status<W>(tx: &mut W, config: &Config) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let sandbox = config.sandbox.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::collect_container_status(&sandbox)
    })
    .await
    .unwrap_or_else(|e| {
        log::warn!("container status task failed: {e}");
        crate::ipc::ContainerStatusReport {
            enabled: config.sandbox.enabled,
            runtime_ok: false,
            runtime_detail: format!("status collection failed: {e}"),
            image_detail: String::new(),
            containers: Vec::new(),
        }
    });
    send_response_split(tx, Response::ContainerStatus { report }).await?;
    Ok(())
}

pub(super) async fn handle_status<W>(
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
    let (active_sessions, active_prompt_tokens, total_turns, status_active_model) =
        with_sessions(sessions, |store| {
            (
                store.len(),
                store.values().map(|s| s.last_prompt_tokens).sum::<u32>(),
                store.values().map(|s| s.turn_count).sum::<usize>(),
                store
                    .values()
                    .filter(|s| !s.is_ghost)
                    .max_by_key(|s| s.last_accessed)
                    .and_then(|s| s.active_model.clone()),
            )
        });
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
    let session_costs: Vec<(String, f64)> = with_sessions(sessions, |store| {
        store
            .iter()
            .map(|(id, entry)| (id.clone(), entry.cost_usd))
            .collect()
    });

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
                    max_turns: config.limits.max_turns,
                    max_tool_calls_per_session: config.limits.max_tool_calls_per_session,
                    per_tool_overrides: overrides,
                }
            },
            active_agents: {
                let mut agents: Vec<(String, String)> = with_sessions(sessions, |store| {
                    store
                        .values()
                        .filter_map(|entry| {
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
                                Some((agent_name.clone(), job_id))
                            } else {
                                None
                            }
                        })
                        .collect()
                });
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

pub(super) async fn handle_query_limits<W>(
    tx: &mut W,
    sessions: &SessionStore,
    config: &Config,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let (turn_count, tool_calls_this_session, history_len) = with_sessions(sessions, |store| {
        if let Some(entry) = store.get(&session_id) {
            (
                entry.turn_count,
                entry.tool_calls_this_session,
                entry.messages.len(),
            )
        } else {
            (0, 0, 0)
        }
    });
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

pub(super) async fn handle_reset_tool_count<W>(
    tx: &mut W,
    sessions: &SessionStore,
    session_id: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(&session_id) {
            entry.tool_calls_this_session = 0;
            log::info!(
                "Session {}: per-session tool call counter reset",
                session_id
            );
        }
    });
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

// ── Named session CRUD handlers ──────────────────────────────────────────────

pub(super) async fn handle_save_session<W>(
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
    let (msgs, turn_count, current_saved, model_name, artifacts) =
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get(&session_id) {
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
            }
        });

    match crate::session_store::save_session(crate::session_store::SaveSessionArgs {
        name: &name,
        current_saved_name: current_saved.as_deref(),
        description: &description,
        messages: &msgs,
        turn_count,
        model: &model_name,
        artifacts: &artifacts,
        force,
    }) {
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
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(&session_id) {
                    entry.saved_name = Some(name.clone());
                    entry.dirty = false;
                }
            });
            log::info!("Session {}: saved as '{}'", session_id, name);
            send_response_split(tx, Response::SessionSaved { name }).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

pub(super) async fn handle_load_session<W>(
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
    let is_dirty = with_sessions(sessions, |store| {
        store.get(&session_id).is_some_and(|e| e.dirty)
    });
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

            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(&session_id) {
                    entry.messages = loaded_msgs;
                    entry.saved_name = Some(name.clone());
                    entry.dirty = false;
                }
            });
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

pub(super) async fn handle_list_saved_sessions<W>(tx: &mut W) -> Result<()>
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

pub(super) async fn handle_delete_saved_session<W>(tx: &mut W, name: String) -> Result<()>
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

pub(super) async fn handle_rename_saved_session<W>(
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
            with_sessions(sessions, |store| {
                for entry in store.values_mut() {
                    if entry.saved_name.as_deref() == Some(old_name.as_str()) {
                        entry.saved_name = Some(new_name.clone());
                    }
                }
            });
            log::info!("Saved session '{}' renamed to '{}'", old_name, new_name);
            send_response_split(tx, Response::Ok).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}
