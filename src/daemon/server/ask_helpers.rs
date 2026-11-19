//! Helper routines for `handle_ask`: session upsert and token-scale loading,
//! the three-tier token-pressure compaction ladder (emergency / compact /
//! elide), activity-based pane snapshot detection, prompt assembly, and
//! pre-turn `SessionInfo` / compaction notice emission.

use super::*;

pub(super) struct AskSessionState {
    pub messages: Vec<Message>,
    pub session_cost_usd: f64,
    pub has_untracked_cost: bool,
    pub session_active_model: Option<String>,
    pub effective_prompt_tokens: u32,
    pub session_token_scale: f64,
}

pub(super) fn upsert_ask_session_and_load_state(
    session_id: Option<&String>,
    session_name: &str,
    chat_pane: &Option<String>,
    client_target_pane: &Option<String>,
    sessions: &SessionStore,
) -> AskSessionState {
    let messages: Vec<Message> = session_id
        .and_then(|id| take_turn_snapshot(sessions, id))
        .or_else(|| {
            session_id
                .map(|id| read_session_file(id, None))
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();

    let (session_cost_usd, has_untracked_cost): (f64, bool) = if let Some(id) = session_id {
        let absent = !with_sessions(sessions, |store| store.contains_key(id.as_str()));
        let built = if absent {
            Some(SessionEntry::from_disk(id))
        } else {
            None
        };
        with_sessions(sessions, |store| {
            if let Some(built) = built {
                store.entry(id.clone()).or_insert(built);
            }
            let Some(entry) = store.get_mut(id.as_str()) else {
                return (0.0, false);
            };
            if let Some(i) = entry.as_interactive_mut() {
                i.chat_pane = chat_pane.clone();
                i.tmux_session = session_name.to_string();
                match (&i.default_target_pane, client_target_pane) {
                    (Some(old), Some(new)) if old != new => {
                        i.default_target_pane = Some(new.clone());
                    }
                    (None, Some(new)) => {
                        i.default_target_pane = Some(new.clone());
                    }
                    _ => {}
                };
            }
            (entry.ledger.cost_usd, entry.ledger.has_untracked_cost)
        })
    } else {
        (0.0, false)
    };

    let session_active_model: Option<String> = session_id.and_then(|id| {
        with_sessions(sessions, |store| {
            store.get(id.as_str()).and_then(|e| e.active_model.clone())
        })
    });

    let (effective_prompt_tokens, session_token_scale): (u32, f64) = if let Some(id) = session_id {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get(id.as_str()) {
                let lpt = entry.ledger.last_prompt_tokens;
                let scale = entry.ledger.token_scale;
                if lpt > 0 {
                    (lpt, scale)
                } else {
                    let est = crate::daemon::context::estimate::estimate_history_tokens(&messages);
                    ((est as f64 * scale).min(u32::MAX as f64) as u32, scale)
                }
            } else {
                (0, 1.5)
            }
        })
    } else {
        (0, 1.5)
    };

    AskSessionState {
        messages,
        session_cost_usd,
        has_untracked_cost,
        session_active_model,
        effective_prompt_tokens,
        session_token_scale,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_emergency_compaction(
    messages: &mut Vec<Message>,
    session_id: Option<&String>,
    sessions: &SessionStore,
    config: &Config,
    context_window: u32,
    session_token_scale: f64,
    token_pct: u32,
    pre_trim_len: usize,
) {
    let elided = crate::daemon::digest::elide_old_tool_results(messages, true);
    let started_at = session_id.and_then(|id| {
        with_sessions(sessions, |store| {
            store.get(id).map(|e| e.persistence.started_at)
        })
    });
    let Some(since) = started_at else {
        log::info!(
            "Emergency compaction (elide): tokens {}% — elided {} chars, no session start recorded; keeping {} messages",
            token_pct,
            elided,
            messages.len()
        );
        return;
    };

    let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;
    let tail_start =
        crate::daemon::digest::planned_tail_start_by_budget(messages, budget, session_token_scale)
            .or_else(|| {
                crate::daemon::digest::synthesized_tail_start(messages, budget, session_token_scale)
            });

    match tail_start {
        Some(ts) => {
            let id = session_id.map(String::as_str).unwrap_or("-");
            let prior = crate::daemon::context::epochs::read_epochs(id);
            let span_start = prior.last().map(|e| e.ts_end).unwrap_or(since);
            let span_end = chrono::Utc::now();
            let dropped = &messages[..ts];
            let record = crate::daemon::context::epochs::EpochRecord {
                seq: prior.last().map(|e| e.seq + 1).unwrap_or(1),
                kind: "epoch".into(),
                turn_start: crate::daemon::context::epochs::first_turn_of(dropped),
                turn_end: crate::daemon::context::epochs::last_turn_of(dropped),
                ts_start: span_start,
                ts_end: span_end,
                msg_count: dropped.len() as u32,
                narrative: None,
                tally: crate::daemon::context::epochs::tally_span(id, span_start, span_end),
                artifacts: crate::daemon::context::epochs::scan_artifacts_span(
                    span_start, span_end,
                ),
                covers: None,
            };
            crate::daemon::context::epochs::append_epoch(id, &record);
            log_event(
                "epoch_created",
                serde_json::json!({
                    "session": id,
                    "seq": record.seq,
                    "turns": [record.turn_start, record.turn_end],
                    "msgs": record.msg_count,
                }),
            );
            let _chain = crate::daemon::context::epochs::read_epochs(id);
            let _ = crate::daemon::context::epochs::maybe_rollup(id, config).await;
            let chain = crate::daemon::context::epochs::read_epochs(id);
            let env = config.context.environment.clone();
            let host = crate::daemon::utils::daemon_hostname();
            let rendered = crate::daemon::context::epochs::render_context_block(&chain);
            let tail_first_turn = messages.get(ts).and_then(|m| m.turn).unwrap_or(0);
            *messages = crate::daemon::context::epochs::compact_with_epochs(
                std::mem::take(messages),
                &rendered,
                &env,
                &host,
                record.turn_end as usize,
                tail_first_turn,
                ts,
            );
            if 2 < messages.len() {
                let tail = &mut messages[2..];
                crate::daemon::digest::repair_tail_head(tail);
            }
            log::info!(
                "Emergency compaction (epoch {}): tokens {}% — compacted {} → {} messages",
                record.seq,
                token_pct,
                pre_trim_len,
                messages.len()
            );
        }
        None => {
            log::info!(
                "Emergency compaction: tokens {}% — no viable tail start found, keeping history as-is",
                token_pct
            );
        }
    }
}

pub(super) struct CompactionOutcome {
    pub pre_trim_len: usize,
    pub post_trim_len: usize,
    pub needs_compaction: bool,
    pub wants_background_compaction: bool,
}

pub(super) async fn apply_turn_compaction_ladder(
    messages: &mut Vec<Message>,
    session_id: Option<&String>,
    sessions: &SessionStore,
    config: &Config,
    session_active_model: Option<&str>,
    effective_prompt_tokens: u32,
    session_token_scale: f64,
) -> CompactionOutcome {
    let context_window = config.resolve_model(session_active_model).context_window();
    let token_pct = if context_window > 0 {
        (effective_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
    } else {
        0
    };
    let pre_trim_len = messages.len();
    use crate::daemon::digest::DIGEST_THRESHOLD;
    let above_floor = messages.len() >= DIGEST_THRESHOLD;

    let is_emergency = token_pct >= config.compaction.emergency_pct;
    let is_compact = token_pct >= config.compaction.compact_at_pct;
    let is_elide = token_pct >= config.compaction.elide_at_pct;
    let mut wants_background_compaction = false;

    if is_emergency && above_floor {
        run_emergency_compaction(
            messages,
            session_id,
            sessions,
            config,
            context_window,
            session_token_scale,
            token_pct,
            pre_trim_len,
        )
        .await;
    } else if is_compact && above_floor {
        let elided = crate::daemon::digest::elide_old_tool_results(messages, true);
        wants_background_compaction = true;
        if elided > 0 {
            log::info!(
                "Compaction (compact/elide): tokens {}% — elided {} chars; epoch build deferred to background",
                token_pct,
                elided
            );
        }
    } else if is_elide && above_floor {
        let elided = crate::daemon::digest::elide_old_tool_results(messages, false);
        if elided > 0 {
            log::info!(
                "Compaction (elide only): tokens {}% — elided {} chars from old tool results",
                token_pct,
                elided
            );
        }
    }

    let did_inline_elide = (is_compact || is_elide) && above_floor && !is_emergency;
    let needs_compaction = messages.len() < pre_trim_len || did_inline_elide;
    let post_trim_len = messages.len();

    CompactionOutcome {
        pre_trim_len,
        post_trim_len,
        needs_compaction,
        wants_background_compaction,
    }
}

pub(super) fn compute_inject_snapshot(
    session_id: Option<&String>,
    sessions: &SessionStore,
    cache: &Arc<SessionCache>,
    is_first_turn: bool,
) -> bool {
    let default_target_pane: Option<String> = session_id.and_then(|id| {
        with_sessions(sessions, |store| {
            store.get(id)?.default_target_pane().map(str::to_string)
        })
    });
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
        .and_then(|id| {
            with_sessions(sessions, |store| {
                store
                    .get(id)
                    .and_then(|e| e.as_interactive())
                    .map(|i| i.last_snapshot_activity)
            })
        })
        .unwrap_or(0);
    let inject_snapshot =
        is_first_turn || (pane_activity > 0 && pane_activity > last_snapshot_activity);

    if inject_snapshot
        && pane_activity > 0
        && let Some(id) = session_id
    {
        with_sessions(sessions, |store| {
            if let Some(i) = store.get_mut(id).and_then(|e| e.as_interactive_mut()) {
                i.last_snapshot_activity = pane_activity;
            }
        });
    }
    inject_snapshot
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn send_pre_turn_session_notices<W: AsyncWrite + Unpin>(
    tx: &mut W,
    session_id: Option<&String>,
    sessions: &SessionStore,
    history_count: usize,
    this_turn_count: usize,
    session_cost_usd: f64,
    has_untracked_cost: bool,
    compaction: &CompactionOutcome,
) -> Result<()> {
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

    if compaction.needs_compaction {
        let ratio = compaction.pre_trim_len as f64 / compaction.post_trim_len.max(1) as f64;
        log::info!(
            "Session {} history compacted: {} → {} messages ({:.1}:1)",
            session_id.map(String::as_str).unwrap_or("-"),
            compaction.pre_trim_len,
            compaction.post_trim_len,
            ratio,
        );
        log_event(
            "compaction",
            serde_json::json!({
                "session": session_id.map(String::as_str).unwrap_or("-"),
                "msgs_before": compaction.pre_trim_len,
                "msgs_after": compaction.post_trim_len,
                "ratio": (ratio * 10.0).round() / 10.0,
            }),
        );
        crate::daemon::stats::record_compaction(compaction.pre_trim_len, compaction.post_trim_len);
        send_response_split(
            tx,
            Response::SystemMsg(format!(
                "↩ Session history compacted ({} → {} messages) — older turns in the session archive",
                compaction.pre_trim_len, compaction.post_trim_len
            )),
        )
        .await?;
    }

    let pending_notice: Option<String> = session_id.and_then(|id| {
        with_sessions(sessions, |store| {
            store
                .get_mut(id)
                .and_then(|e| e.history.pending_compaction_notice.take())
        })
    });
    if let Some(notice) = pending_notice {
        send_response_split(tx, Response::SystemMsg(notice)).await?;
    }
    Ok(())
}

pub(super) struct AssembledAskPrompts {
    pub user_prompt: String,
    pub sys_prompt: String,
    pub is_ghost_session: bool,
    pub cost_attribution: CostAttribution,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn assemble_ask_prompts(
    req: &AskRequest,
    sessions: &SessionStore,
    cache: &Arc<SessionCache>,
    config: &Config,
    is_first_turn: bool,
    is_ghost_session_initial: bool,
    effective_prompt_tokens: u32,
    history_count: usize,
    this_turn_count: usize,
) -> AssembledAskPrompts {
    let safe_query = mask_sensitive(&req.query);
    let inject_snapshot =
        compute_inject_snapshot(req.session_id.as_ref(), sessions, cache, is_first_turn);

    let ghost_turn_limit: Option<usize> = req.session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
            let g = store.get(id)?.as_ghost()?;
            let ceiling = config.ghost.max_ghost_turns;
            let limit = if g.config.max_ghost_turns > 0 {
                g.config.max_ghost_turns.min(ceiling)
            } else {
                ceiling
            };
            Some(limit)
        })
    });

    let memory_namespaces_owned = crate::daemon::executor::build_memory_namespaces(
        req.session_id.as_deref(),
        sessions,
        is_ghost_session_initial,
    );
    let memory_namespaces: Vec<&str> = memory_namespaces_owned.iter().map(|s| s.as_str()).collect();
    let (tool_policy_owned, agent_name_owned, is_ghost_session, parent_job_id_owned) = req
        .session_id
        .as_ref()
        .and_then(|id| {
            with_sessions(sessions, |store| {
                let entry = store.get(id)?;
                if let Some(g) = entry.as_ghost() {
                    Some((
                        g.config.tool_policy.clone(),
                        g.config.agent.clone(),
                        true,
                        g.config.parent_job_id.clone(),
                    ))
                } else {
                    Some((None, None, false, None))
                }
            })
        })
        .unwrap_or((None, None, false, None));
    let cost_attribution = CostAttribution {
        agent_name: agent_name_owned
            .clone()
            .unwrap_or_else(|| "chat".to_string()),
        is_ghost: is_ghost_session,
        parent_job_id: parent_job_id_owned,
    };
    let prompt_ctx = PromptCtx {
        client_pane: req.client_pane.as_deref(),
        chat_pane: req.chat_pane.as_deref(),
        cache,
        config,
        chat_width: req.chat_width,
        safe_query: &safe_query,
        last_prompt_tokens: effective_prompt_tokens,
        history_count,
        this_turn_count,
        ghost_turn_limit,
        inject_snapshot,
        memory_namespaces: &memory_namespaces,
        session_id: req.session_id.as_deref(),
        tool_policy: tool_policy_owned.as_ref(),
        agent_name: agent_name_owned.as_deref(),
    };

    let user_prompt = if is_first_turn {
        build_first_turn_prompt(&prompt_ctx)
    } else {
        build_subsequent_turn_prompt(&prompt_ctx)
    };
    let prompt_name = req.prompt_override.as_deref().unwrap_or(&config.ai.prompt);
    let sys_prompt = load_named_prompt(prompt_name).system;

    AssembledAskPrompts {
        user_prompt,
        sys_prompt,
        is_ghost_session,
        cost_attribution,
    }
}
