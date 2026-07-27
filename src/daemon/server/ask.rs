use super::catchup::build_catchup_brief;
use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use crate::config::{Config, load_named_prompt};
use crate::cost::CostAttribution;
use crate::daemon::prompt::{PromptCtx, build_first_turn_prompt, build_subsequent_turn_prompt};
use crate::daemon::session::*;
use crate::daemon::stream;
use crate::daemon::utils::*;
use crate::ipc::Response;
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncWrite};
// ── Ask handler ──────────────────────────────────────────────────────────────

pub(super) struct AskRequest {
    pub query: String,
    pub client_pane: Option<String>,
    pub session_id: Option<String>,
    pub chat_pane: Option<String>,
    pub prompt_override: Option<String>,
    pub chat_width: Option<usize>,
    pub client_tmux_session: Option<String>,
    pub client_target_pane: Option<String>,
}

pub(super) struct AskContext<'a> {
    pub cache: Arc<SessionCache>,
    pub sessions: &'a SessionStore,
    pub schedule_store: Arc<ScheduleStore>,
    pub bg_session: Arc<std::sync::Mutex<String>>,
    pub config: &'a Config,
}

pub(super) async fn handle_ask<W, R>(
    req: AskRequest,
    ctx: AskContext<'_>,
    tx: &mut W,
    rx: &mut R,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let AskRequest {
        query: initial_query,
        client_pane,
        session_id,
        chat_pane,
        prompt_override,
        chat_width,
        client_tmux_session,
        client_target_pane,
    } = req;
    let AskContext {
        cache,
        sessions,
        schedule_store,
        bg_session,
        config,
    } = ctx;
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
        .and_then(|id| with_sessions(sessions, |mem| mem.get(id).map(|e| e.messages.clone())))
        .or_else(|| {
            session_id
                .as_ref()
                .map(|id| read_session_file(id, None))
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
        with_sessions(sessions, |store| {
            let entry = store.entry(id.clone()).or_insert_with(|| {
                // Try to restore continuity state from persisted meta.
                let meta = crate::daemon::session::read_session_meta(id);
                let (
                    started_at,
                    turn_count,
                    last_prompt_tokens,
                    tool_calls_this_session,
                    token_scale,
                    saved_name,
                ) = match meta {
                    Some(m) => (
                        m.started_at,
                        m.turn_count,
                        m.last_prompt_tokens,
                        m.tool_calls_this_session,
                        m.token_scale,
                        m.saved_name,
                    ),
                    None => (chrono::Utc::now(), 0, 0, 0, 1.5, None),
                };
                SessionEntry {
                    messages: Vec::new(),
                    last_accessed: Instant::now(),
                    chat_pane: chat_pane.clone(),
                    default_target_pane: client_target_pane.clone(),
                    bg_windows: Vec::new(),
                    last_prompt_tokens,
                    tmux_session: session_name.clone(),
                    last_detach: None,
                    detach_time_utc: None,
                    messages_at_detach: 0,
                    pipe_source_pane: None,
                    is_ghost: false,
                    ghost_config: None,
                    ghost_bg_prefix: crate::daemon::GS_BG_WINDOW_PREFIX,
                    started_at,
                    turn_count,
                    tool_calls_this_session,
                    active_model: None,
                    last_snapshot_activity: 0,
                    saved_name,
                    dirty: false,
                    artifacts_created: Vec::new(),
                    auto_name_suggested: false,
                    ghost_task_message: None,
                    loaded_tools: std::collections::HashSet::new(),
                    cost_usd: 0.0,
                    cost_by_agent: std::collections::HashMap::new(),
                    has_untracked_cost: false,
                    token_scale,
                    compaction_in_flight: false,
                    pending_compaction_notice: None,
                }
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
        })
    } else {
        (None, None, 0.0, false)
    };

    // Read the session's active model override once so it stays consistent for
    // the whole turn (including the budget line and every AI loop iteration).
    let session_active_model: Option<String> = session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
            store.get(id.as_str()).and_then(|e| e.active_model.clone())
        })
    });

    // Read last prompt token count up front — it drives the compaction decision
    // below and is also used later for the [BUDGET] line.
    // If last_prompt_tokens is 0 (post-restart blind spot), substitute the
    // calibrated estimate.
    // Also capture the session's calibrated token_scale (phase 02) so the
    // budget-based cut below plans against real per-session calibration rather
    // than a hardcoded 1.0. Defaults to 1.5 (the SessionEntry initial value)
    // when there is no entry.
    let (effective_prompt_tokens, session_token_scale): (u32, f64) = if let Some(ref id) =
        session_id
    {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get(id.as_str()) {
                let lpt = entry.last_prompt_tokens;
                let scale = entry.token_scale;
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

    // Token-pressure-driven compaction.
    //
    // Compaction config drives thresholds:
    //   elide_at_pct (50%) — elide oversized tool_results in old messages; cheap,
    //   preserves turn structure.
    //   compact_at_pct (60%) — aggressive elision NOW (cheap, sync); defer the
    //   epoch build to a background task (phase 08).
    //   emergency_pct (85%) — synchronous structured-only compaction (no
    //   narrative call) when pressure is extreme.
    //   target_pct (40%) — post-compaction working-set target.
    //
    // All paths require `messages.len() >= DIGEST_THRESHOLD` so a token-heavy
    // first turn (huge system context + memory) doesn't trigger compaction
    // before any real history exists.
    let context_window = config
        .resolve_model(session_active_model.as_deref())
        .context_window();
    let token_pct = if context_window > 0 {
        (effective_prompt_tokens as f64 / context_window as f64 * 100.0) as u32
    } else {
        0
    };
    let pre_trim_len = messages.len();
    use crate::daemon::digest::DIGEST_THRESHOLD;
    let above_floor = messages.len() >= DIGEST_THRESHOLD;

    // Decision ladder: emergency > compact > elide. Compaction is driven purely
    // by token pressure now that the message-count cap has been removed.
    let is_emergency = token_pct >= config.compaction.emergency_pct;
    let is_compact = token_pct >= config.compaction.compact_at_pct;
    let is_elide = token_pct >= config.compaction.elide_at_pct;
    // Set when the compact threshold triggers cheap sync elision but defers the
    // epoch build to the post-turn background task (phase 08). Threaded through
    // ConversationLoopCtx; the loop spawns the compaction after the turn ends.
    let mut wants_background_compaction = false;

    if is_emergency && above_floor {
        // Emergency path: synchronous structured-only compaction.
        // No narrative call — this is the backstop when pressure is extreme.
        // Aggressive elision first.
        let elided = crate::daemon::digest::elide_old_tool_results(&mut messages, true);
        let started_at = session_id
            .as_ref()
            .and_then(|id| with_sessions(sessions, |store| store.get(id).map(|e| e.started_at)));
        if let Some(since) = started_at {
            // Budget-based compaction: compute the target budget and plan the
            // cut against the session's calibrated token_scale (phase 02).
            let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;

            // Try clean boundary first, fall back to synthesized boundary.
            let tail_start = crate::daemon::digest::planned_tail_start_by_budget(
                &messages,
                budget,
                session_token_scale,
            )
            .or_else(|| {
                crate::daemon::digest::synthesized_tail_start(
                    &messages,
                    budget,
                    session_token_scale,
                )
            });

            match tail_start {
                Some(ts) => {
                    let id = session_id.as_deref().unwrap_or("-");
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
                        // Emergency: structured-only, no narrative — even when
                        // narrative_enabled = true. See background.rs
                        // `epoch_narrative_allowed` for the shared invariant.
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
                    // Attempt chapter rollup before rendering.
                    let _ = crate::daemon::context::epochs::maybe_rollup(id, config).await;
                    let chain = crate::daemon::context::epochs::read_epochs(id);
                    let env = config.context.environment.clone();
                    let host = crate::daemon::utils::daemon_hostname();
                    let rendered = crate::daemon::context::epochs::render_context_block(&chain);
                    // Get the first turn of the tail for the assistant ack.
                    let tail_first_turn = messages.get(ts).and_then(|m| m.turn).unwrap_or(0);
                    messages = crate::daemon::context::epochs::compact_with_epochs(
                        messages,
                        &rendered,
                        &env,
                        &host,
                        record.turn_end as usize,
                        tail_first_turn,
                        ts,
                    );
                    // Repair the tail head for any orphan tool_results.
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
        } else {
            log::info!(
                "Emergency compaction (elide): tokens {}% — elided {} chars, no session start recorded; keeping {} messages",
                token_pct,
                elided,
                messages.len()
            );
        }
    } else if is_compact && above_floor {
        // Compact threshold reached: aggressive elision NOW (cheap, sync) and
        // defer the epoch build to the post-turn background task. History is
        // NOT cut here — the background task owns that.
        let elided = crate::daemon::digest::elide_old_tool_results(&mut messages, true);
        wants_background_compaction = true;
        if elided > 0 {
            log::info!(
                "Compaction (compact/elide): tokens {}% — elided {} chars; epoch build deferred to background",
                token_pct,
                elided
            );
        }
    } else if is_elide && above_floor {
        // Soft elision only — cheap, preserves turn structure (unchanged).
        let elided = crate::daemon::digest::elide_old_tool_results(&mut messages, false);
        if elided > 0 {
            log::info!(
                "Compaction (elide only): tokens {}% — elided {} chars from old tool results",
                token_pct,
                elided
            );
        }
    }
    // If the message vec shrank, OR we elided tool-result content in place, the
    // on-disk file must be fully rewritten to remove/replace the stale entries.
    // The compact and elide branches mutate content without changing len, so
    // key off entering either branch as well as a length shrink.
    let did_inline_elide = (is_compact || is_elide) && above_floor && !is_emergency;
    let needs_compaction = messages.len() < pre_trim_len || did_inline_elide;
    let post_trim_len = messages.len();

    let is_first_turn = messages.is_empty();

    // Read the current turn count and increment it for this turn.  Never reset
    // by compaction — this gives the client a stable, ever-increasing indicator.
    let this_turn_count = session_id
        .as_ref()
        .map(|id| {
            with_sessions(sessions, |store| {
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
        .and_then(|id| with_sessions(sessions, |store| store.get(id).map(|e| e.is_ghost)))
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
        .and_then(|id| with_sessions(sessions, |store| store.get(id)?.default_target_pane.clone()));

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
            with_sessions(sessions, |store| {
                store.get(id).map(|e| e.last_snapshot_activity)
            })
        })
        .unwrap_or(0);
    let inject_snapshot =
        is_first_turn || (pane_activity > 0 && pane_activity > last_snapshot_activity);

    // Record the activity timestamp so the next turn can detect further changes.
    if inject_snapshot
        && pane_activity > 0
        && let Some(ref id) = session_id
    {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(id) {
                entry.last_snapshot_activity = pane_activity;
            }
        });
    }

    // Ghost sessions: resolve the effective turn cap (runbook value clamped
    // to daemon ceiling; 0 = use the ceiling).  Returns None for regular chat.
    let ghost_turn_limit: Option<usize> = session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
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
        })
    });

    // Build the prompt using the prompt module.
    let memory_namespaces_owned = crate::daemon::executor::build_memory_namespaces(
        session_id.as_deref(),
        sessions,
        is_ghost_session,
    );
    let memory_namespaces: Vec<&str> = memory_namespaces_owned.iter().map(|s| s.as_str()).collect();
    let (tool_policy_owned, agent_name_owned, is_ghost_session, parent_job_id_owned) = session_id
        .as_ref()
        .and_then(|id| {
            with_sessions(sessions, |store| {
                let entry = store.get(id)?;
                let ghost = entry.is_ghost;
                let (policy, agent) = if ghost {
                    (
                        entry
                            .ghost_config
                            .as_ref()
                            .and_then(|gc| gc.tool_policy.clone()),
                        entry.ghost_config.as_ref().and_then(|gc| gc.agent.clone()),
                    )
                } else {
                    (None, None)
                };
                let parent = entry
                    .ghost_config
                    .as_ref()
                    .and_then(|gc| gc.parent_job_id.clone());
                Some((policy, agent, ghost, parent))
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
        client_pane: client_pane.as_deref(),
        chat_pane: chat_pane.as_deref(),
        default_target_pane: default_target_pane.as_deref(),
        cache: &cache,
        config,
        chat_width,
        safe_query: &safe_query,
        last_prompt_tokens: effective_prompt_tokens,
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
                "↩ Session history compacted ({} → {} messages) — older turns in the session archive",
                pre_trim_len, post_trim_len
            )),
        )
        .await?;
    }

    // Phase 08: deliver any notice queued by a completed background compaction
    // task. Drained here at the top of the turn's response, alongside the
    // existing compaction notice above.
    let pending_notice: Option<String> = session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
            store
                .get_mut(id)
                .and_then(|e| e.pending_compaction_notice.take())
        })
    });
    if let Some(notice) = pending_notice {
        send_response_split(tx, Response::SystemMsg(notice)).await?;
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
    let ctx = stream::ConversationLoopCtx {
        session_id,
        session_name: &session_name,
        chat_pane,
        messages,
        sys_prompt,
        session_active_model,
        is_ghost_session,
        this_turn_count,
        post_trim_len,
        needs_compaction,
        wants_background_compaction,
        config,
        cache,
        sessions: sessions.clone(),
        schedule_store,
        cost_attribution,
    };
    stream::run_conversation_loop(ctx, tx, rx).await
}
