//! `run_conversation_loop`, one chat turn end to end: the model call, the
//! event stream, the tool calls and their replies, the persistence. A child
//! module of `src/daemon/stream.rs`, which re-exports the function.

use super::*;

pub async fn run_conversation_loop<W, R>(
    ctx: ConversationLoopCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let ConversationLoopCtx {
        session_id,
        session_name,
        chat_pane,
        mut messages,
        sys_prompt,
        session_active_model,
        is_ghost_session,
        this_turn_count,
        post_trim_len,
        needs_compaction,
        wants_background_compaction,
        config,
        cache,
        sessions,
        schedule_store,
        cost_attribution,
    } = ctx;

    // A chat Run: RunStarted here, exactly one RunFinished however this
    // exits. Constructed first so the cancel registration below can use its
    // id — one `RunId` identifies this turn in the run log, the cancel
    // registry and the sentinel path.
    let mut run_log = crate::daemon::run::RunLogGuard::start(
        crate::daemon::run::RunId::new(),
        crate::daemon::run::RunKind::Chat {
            session: session_id.clone().unwrap_or_default(),
        },
    );

    // Register this turn's cancel signal with the run-keyed registry. Both
    // cancel doors converge here: the chat client's Ctrl-C resolves the
    // session to this run through the active-session map, and the stop
    // sentinel from `daemoneye stop <run>` is watched by a spawned task below
    // (named turns only — a ghost has no session to resolve and must stay
    // uncancellable by this path). The signal handed back belongs to
    // `run_log.run_id()`, so a select! on it is a select! on the same run
    // both doors name.
    let (_cancel_guard, mut cancel_signal) = crate::daemon::run::register_run(run_log.run_id());
    let _active_guard;
    if let Some(sid) = session_id.as_deref() {
        _active_guard = Some(crate::daemon::run::register_active(sid, run_log.run_id()));
        tokio::spawn(crate::daemon::run::watch_stop_sentinel(
            run_log.run_id().clone(),
            crate::daemon::run::STOP_POLL_INTERVAL,
        ));
    } else {
        // Ghost: no session to resolve, no watcher. `register_run`'s signal
        // can only fire through `cancel_run`, which nothing in this turn's
        // lifetime calls — the cancelled() branch below stays unreachable.
        _active_guard = None;
    }

    let turn_deadline = (config.limits.turn_timeout_secs > 0).then(|| {
        tokio::time::Instant::now()
            + std::time::Duration::from_secs(config.limits.turn_timeout_secs)
    });

    // Per-turn tool-call loop guard — spans every batch cycle in this turn.
    // Declared outside the batch loop so sequential single-call batches
    // accumulate (the 2026-08-11 per-batch cap bug). Approval-gated tools
    // are always exempt — the user's per-call approval prompt is the gate.
    let mut tool_call_counts: HashMap<&str, u32> = HashMap::new();
    let mut total_turn_call_count: u32 = 0;

    let mut channel_closed_retries: u32 = 0;

    loop {
        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        // The daily cap gates every model-call iteration — tool-loop
        // continuations included — before `make_client`, so a turn at or
        // over `[budget] max_cost_per_day` never reaches a provider. The
        // refusal is the turn-timeout branch's analogue: a `SystemMsg`, a
        // `BudgetExceeded` finish with the message as the briefing's blocker
        // on the `Usd` dimension, and the turn ends.
        if let Err(message) =
            crate::daemon::spend::cost_gate(config, crate::daemon::spend::SpendPath::Chat)
        {
            log::warn!(
                "chat turn for session {}: daily cost cap refuses the model call: {message}",
                session_id.as_deref().unwrap_or("(ghost)"),
            );
            send_response_split(tx, Response::SystemMsg(message.clone())).await?;
            run_log.finish(crate::daemon::run::RunState::BudgetExceeded(
                crate::daemon::run::Briefing::new(
                    "chat turn",
                    None,
                    Vec::new(),
                    Vec::new(),
                    Some(message),
                    Some(crate::daemon::run::BudgetDimension::Usd),
                ),
            ));
            send_response_split(tx, Response::Ok).await?;
            return Ok(());
        }

        let active_entry = config.resolve_model(session_active_model.as_deref());
        run_log
            .metrics_mut()
            .set_model(active_entry.provider.clone(), active_entry.model.clone());
        run_log.metrics_mut().record_turn();
        crate::daemon::run::emit_turn_started(run_log.run_id(), run_log.metrics().turns as usize);
        let client_instance = make_client(
            &active_entry.provider,
            active_entry.resolve_api_key(),
            active_entry.model.clone(),
            active_entry.effective_base_url(),
            active_entry.effective_max_tokens(),
        );
        let sys_prompt_turn = sys_prompt.clone();
        let messages_clone = messages.clone();

        // Read the session's on-demand tool set so deferred groups loaded via
        // `load_tools` on a prior turn are rendered for this turn (M1 phase-11).
        let loaded_tools: Vec<String> = session_id
            .as_deref()
            .and_then(|sid| {
                with_sessions(&sessions, |store| {
                    store
                        .get(sid)
                        .map(|e| e.loaded_tools.iter().cloned().collect())
                })
            })
            .unwrap_or_default();

        let mut chat_task = ChatTaskGuard::new(tokio::spawn(async move {
            if let Err(e) = client_instance
                .chat(
                    &sys_prompt_turn,
                    messages_clone,
                    ai_tx.clone(),
                    true,
                    loaded_tools,
                )
                .await
            {
                let _ = ai_tx.send(AiEvent::Error(e.to_string()));
            }
        }));

        let mut full_response = String::new();
        let mut pending_calls: Vec<PendingCall> = Vec::new();

        loop {
            if let Some(deadline) = turn_deadline
                && tokio::time::Instant::now() >= deadline
            {
                return endings::end_turn_timeout(tx, config, &mut run_log).await;
            }
            let event = tokio::select! {
                biased;
                reason = cancel_signal.cancelled() => {
                    let stage = cancel_stage(run_log.run_id());
                    if !full_response.is_empty() {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: format!("{full_response}\n\n[⊘ cancelled by user]"),
                            tool_calls: None,
                            tool_results: None,
                            turn: Some(this_turn_count),
                        });
                    }
                    // Persist the partial response so the interrupted turn leaves
                    // a trace in the session history on disk.
                    if let Some(ref id) = session_id {
                        with_sessions(&sessions, |store| {
                            if let Some(entry) = store.get_mut(id) {
                                entry.messages = messages.clone();
                                entry.dirty = true;
                            }
                        });
                        for msg in &messages[post_trim_len..] {
                            append_session_message(id, msg);
                        }
                    }
                    log::info!(
                        "turn cancelled at stage {stage} via {reason:?} for session {:?}",
                        session_id.as_deref().unwrap_or("(ghost)"),
                    );
                    send_response_split(tx, Response::SystemMsg("⊘ cancelled".to_string())).await?;
                    send_response_split(tx, Response::Ok).await?;
                    run_log.finish(crate::daemon::run::RunState::Cancelled {
                        stage,
                        turns: this_turn_count as u32,
                    });
                    return Ok(());
                }
                recv = tokio::time::timeout(
                    std::time::Duration::from_secs(crate::daemon::utils::KEEPALIVE_PERIOD_SECS),
                    ai_rx.recv(),
                ) => match recv {
                    Ok(Some(ev)) => ev,
                    Ok(None) => {
                        let cause = chat_task.describe_end().await;
                        channel_closed_retries += 1;
                        if channel_closed_retries > MAX_CHANNEL_CLOSED_RETRIES {
                            return endings::end_stream_closed(tx, &mut run_log, channel_closed_retries, cause)
                                .await;
                        }
                        log::warn!(
                            "AI event channel closed without Done/Error (attempt \
                             {channel_closed_retries}/{MAX_CHANNEL_CLOSED_RETRIES}), \
                             re-issuing the AI call: {cause}"
                        );
                        break;
                    }
                    Err(_timeout) => {
                        // No token in 15 s — send a keep-alive so the client doesn't
                        // hit its per-token deadline (slow local LLMs can stall longer).
                        send_response_split(tx, Response::KeepAlive).await?;
                        continue;
                    }
                },
            };
            match event {
                AiEvent::Token(t) => {
                    full_response.push_str(&t);
                    send_response_split(tx, Response::Token(t)).await?;
                }
                AiEvent::ToolCall {
                    id,
                    cmd,
                    wait,
                    interactive,
                    thought_signature,
                    shell,
                    host,
                    timeout_secs,
                } => {
                    if routes_to_background(wait, interactive) {
                        // `wait: false` stays on the daemon host; a named host
                        // on a background command is row 08b's surface, so the
                        // value is dropped here (sre.toml promises this).
                        let _ = host;
                        pending_calls.push(PendingCall::Background {
                            id,
                            cmd,
                            _credential: None,
                            thought_signature: thought_signature.clone(),
                            host: None,
                        });
                    } else {
                        pending_calls.push(PendingCall::Foreground {
                            id,
                            cmd,
                            shell,
                            host,
                            timeout_secs,
                            interactive,
                            thought_signature: thought_signature.clone(),
                        });
                    }
                }
                AiEvent::Error(e) => {
                    return endings::end_backend_error(tx, &mut run_log, e).await;
                }
                AiEvent::Notice(n) => {
                    send_response_split(tx, Response::SystemMsg(format!("⚠ {n}"))).await?;
                }
                AiEvent::Done(usage) => {
                    // Token pressure is the sole driver of context compaction now
                    // that the message-count cap has been removed. A provider that
                    // reports no usage leaves the compactor blind for this turn, so
                    // surface it — the prompt always contains a non-empty system
                    // block, making a zero sum a reliable "usage missing" signal.
                    if usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens == 0
                    {
                        log::warn!(
                            "provider reported no token usage for model {} (session {}); \
                             token-pressure compaction cannot run this turn",
                            config.resolve_model(session_active_model.as_deref()).model,
                            session_id.as_deref().unwrap_or("-"),
                        );
                    }
                    if pending_calls.is_empty() {
                        // No tool calls — this is the final answer.
                        if full_response.is_empty() {
                            send_response_split(
                                tx,
                                Response::SystemMsg(
                                    "The model returned an empty reply.".to_string(),
                                ),
                            )
                            .await?;
                        } else {
                            messages.push(Message {
                                role: "assistant".to_string(),
                                content: full_response.clone(),
                                tool_calls: None,
                                tool_results: None,
                                turn: Some(this_turn_count),
                            });
                        }

                        let active_entry = config.resolve_model(session_active_model.as_deref());
                        let record = crate::cost::emit_cost(crate::cost::CostCall {
                            model_entry: active_entry,
                            usage: &usage,
                            attribution: &cost_attribution,
                            session_id: session_id.as_deref().unwrap_or("-"),
                            run_id: run_log.run_id().as_str(),
                            now: chrono::Utc::now(),
                        });
                        run_log.metrics_mut().record_model_call(
                            active_entry.provider.clone(),
                            active_entry.model.clone(),
                            &usage,
                            record.cost.total_cost_usd,
                        );

                        // Accumulate cost on the session entry.
                        if let Some(ref id) = session_id {
                            with_sessions(&sessions, |store| {
                                if let Some(entry) = store.get_mut(id) {
                                    entry.cost_usd += record.cost.total_cost_usd;
                                    *entry
                                        .cost_by_agent
                                        .entry(record.agent_name.clone())
                                        .or_insert(0.0) += record.cost.total_cost_usd;
                                    if record.pricing_source == PricingSource::Unknown {
                                        entry.has_untracked_cost = true;
                                    }
                                }
                            });
                        }

                        log_event(
                            "ai_turn",
                            serde_json::json!({
                                "session": session_id.as_deref().unwrap_or("-"),
                                "model": config.resolve_model(session_active_model.as_deref()).model,
                                "prompt_tokens": usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens,
                                "completion_tokens": usage.output_tokens,
                                "cache_read_tokens": usage.cache_read_tokens,
                                "cache_write_tokens": usage.cache_write_tokens,
                            }),
                        );

                        // Persist the conversation for the next turn.
                        if let Some(ref id) = session_id {
                            with_sessions(&sessions, |store| {
                                if let Some(entry) = store.get_mut(id) {
                                    entry.messages = messages.clone();
                                    entry.last_accessed = Instant::now();
                                    entry.last_prompt_tokens = usage.input_tokens
                                        + usage.cache_read_tokens
                                        + usage.cache_write_tokens;
                                    crate::daemon::context::estimate::update_token_scale(
                                        entry, &messages,
                                    );
                                    entry.dirty = true;
                                    if chat_pane.is_some() {
                                        entry.chat_pane = chat_pane.clone();
                                    }
                                }
                            });
                            if needs_compaction {
                                // Archive invariant: every message in the pre-compaction vec
                                // was appended to the archive when first persisted, so the
                                // rewrite below cannot lose history (see docs/design/context-management.md §3.1).
                                write_session_file(id, &messages);
                            } else {
                                for msg in &messages[post_trim_len..] {
                                    append_session_message(id, msg);
                                }
                            }
                            // Persist session meta so restarts/eviction don't lose continuity state.
                            // Skipped for ghost sessions — their one-shot UUID ids are never
                            // recreated, so a ghost meta file would be a write-only orphan.
                            if !is_ghost_session {
                                let meta = with_sessions(&sessions, |store| {
                                    store
                                        .get(id)
                                        .map(|entry| crate::daemon::session::SessionMeta {
                                            started_at: entry.started_at,
                                            turn_count: entry.turn_count,
                                            last_prompt_tokens: entry.last_prompt_tokens,
                                            token_scale: entry.token_scale,
                                            tool_calls_this_session: entry.tool_calls_this_session,
                                            saved_name: entry.saved_name.clone(),
                                        })
                                });
                                if let Some(meta) = meta {
                                    crate::daemon::session::write_session_meta(id, &meta);
                                }
                            }
                            // Spawn background compaction if the turn signaled it.
                            // spawn_compaction takes the lock itself (snapshot) and
                            // no-ops on ghost / already-in-flight sessions, so we
                            // must NOT hold the lock here — std::sync::Mutex is not
                            // reentrant and re-locking would deadlock.
                            if wants_background_compaction {
                                crate::daemon::context::background::spawn_compaction(
                                    id.clone(),
                                    sessions.clone(),
                                    Arc::new(config.clone()),
                                );
                            }
                        }
                        // Auto-name suggestion: fires once when the turn count hits
                        // the configured threshold on an unnamed interactive session.
                        if !is_ghost_session
                            && config.sessions.auto_name_enabled
                            && config.sessions.auto_name_turn_threshold > 0
                        {
                            let should_suggest = with_sessions(&sessions, |store| {
                                if let Some(ref id) = session_id
                                    && let Some(entry) = store.get_mut(id)
                                    && entry.saved_name.is_none()
                                    && !entry.auto_name_suggested
                                    && entry.turn_count == config.sessions.auto_name_turn_threshold
                                {
                                    entry.auto_name_suggested = true;
                                    true
                                } else {
                                    false
                                }
                            });
                            if should_suggest {
                                let suggestion: Option<(String, String)> =
                                    crate::daemon::utils::with_keepalive(
                                        tx,
                                        auto_name::suggest_session_name(&messages, config),
                                    )
                                    .await?;
                                if let Some((name, desc)) = suggestion {
                                    let hint = if desc.is_empty() {
                                        format!(
                                            "💡 Save this session as '{}'? \
                                             Run `/session save {}`",
                                            name, name
                                        )
                                    } else {
                                        format!(
                                            "💡 Save this session as '{}'? \
                                             Run `/session save {} {}`",
                                            name, name, desc
                                        )
                                    };
                                    send_response_split(tx, Response::SystemMsg(hint)).await?;
                                }
                            }
                        }

                        send_response_split(
                            tx,
                            Response::UsageUpdate {
                                prompt_tokens: usage.input_tokens
                                    + usage.cache_read_tokens
                                    + usage.cache_write_tokens,
                            },
                        )
                        .await?;
                        send_response_split(tx, Response::Ok).await?;
                        run_log.finish(crate::daemon::run::RunState::Complete);
                        return Ok(());
                    }

                    let active_entry = config.resolve_model(session_active_model.as_deref());
                    let record = crate::cost::emit_cost(crate::cost::CostCall {
                        model_entry: active_entry,
                        usage: &usage,
                        attribution: &cost_attribution,
                        session_id: session_id.as_deref().unwrap_or("-"),
                        run_id: run_log.run_id().as_str(),
                        now: chrono::Utc::now(),
                    });
                    run_log.metrics_mut().record_model_call(
                        active_entry.provider.clone(),
                        active_entry.model.clone(),
                        &usage,
                        record.cost.total_cost_usd,
                    );

                    // Accumulate cost on the session entry.
                    if let Some(ref id) = session_id {
                        with_sessions(&sessions, |store| {
                            if let Some(entry) = store.get_mut(id) {
                                entry.cost_usd += record.cost.total_cost_usd;
                                *entry
                                    .cost_by_agent
                                    .entry(record.agent_name.clone())
                                    .or_insert(0.0) += record.cost.total_cost_usd;
                                if record.pricing_source == PricingSource::Unknown {
                                    entry.has_untracked_cost = true;
                                }
                            }
                        });
                    }

                    log_event(
                        "ai_turn",
                        serde_json::json!({
                            "session": session_id.as_deref().unwrap_or("-"),
                            "model": config.resolve_model(None).model,
                            "prompt_tokens": usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens,
                            "completion_tokens": usage.output_tokens,
                            "cache_read_tokens": usage.cache_read_tokens,
                            "cache_write_tokens": usage.cache_write_tokens,
                        }),
                    );

                    // Update session token tracking so the budget line in the next
                    // prompt reflects the actual context size of this turn.
                    if let Some(ref id) = session_id {
                        with_sessions(&sessions, |store| {
                            if let Some(entry) = store.get_mut(id) {
                                entry.last_prompt_tokens = usage.input_tokens
                                    + usage.cache_read_tokens
                                    + usage.cache_write_tokens;
                                crate::daemon::context::estimate::update_token_scale(
                                    entry, &messages,
                                );
                            }
                        });
                    }

                    // Push one assistant message listing all tool calls.
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: full_response.clone(),
                        tool_calls: Some(pending_calls.iter().map(|c| c.to_tool_call()).collect()),
                        tool_results: None,
                        turn: Some(this_turn_count),
                    });

                    let mut tool_results = Vec::new();
                    let mut user_message_redirect: Option<String> = None;
                    for call in pending_calls.iter() {
                        let call_id = call.id().to_string();
                        let tool_name = call.tool_name();
                        let mut tool_budget_hint: Option<String> = None;

                        if !is_budget_exempt(call) {
                            total_turn_call_count += 1;

                            // Per-session cumulative cap across all non-approval-gated tools.
                            if let Some(session_limit) = crate::config::LimitsConfig::cap_usize(
                                config.limits.max_tool_calls_per_session,
                            ) {
                                let session_tool_count = session_id
                                    .as_ref()
                                    .and_then(|id| {
                                        with_sessions(&sessions, |store| {
                                            store.get(id).map(|e| e.tool_calls_this_session)
                                        })
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

                        let sessions_for_exec = sessions.clone();
                        let outcome = match executor::execute_tool_call(
                            call,
                            tx,
                            rx,
                            SessionCtx {
                                session_id: session_id.as_deref(),
                                session_name,
                                chat_pane: chat_pane.as_deref(),
                                sessions: &sessions_for_exec,
                            },
                            &cache,
                            &schedule_store,
                        )
                        .await
                        {
                            Ok(res) => res,
                            Err(_) => return Ok(()),
                        };

                        executor::tally_tool_outcome(run_log.metrics_mut(), call, &outcome);

                        match outcome {
                            executor::ToolCallOutcome::UserMessage(text) => {
                                user_message_redirect = Some(text);
                                break;
                            }
                            executor::ToolCallOutcome::Result(result) => {
                                let content = match tool_budget_hint {
                                    Some(hint) => format!("{}{}", result, hint),
                                    None => result,
                                };
                                tool_results.push(ToolResult {
                                    tool_call_id: call_id,
                                    tool_name: tool_name.to_string(),
                                    content,
                                });
                                // Bump per-session counter for non-budget-exempt calls.
                                if !is_budget_exempt(call)
                                    && let Some(id) = &session_id
                                {
                                    with_sessions(&sessions, |store| {
                                        if let Some(entry) = store.get_mut(id) {
                                            entry.tool_calls_this_session += 1;
                                        }
                                    });
                                }
                            }
                            executor::ToolCallOutcome::SpawnGhostSession {
                                session_id: ghost_sid,
                                runbook_name: ghost_rb,
                                tool_result,
                                job_id: _,
                            } => {
                                let ghost_sessions = sessions.clone();
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
                                            )
                                            .await;
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
                                            )
                                            .await;
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
                    let result_char_cap =
                        crate::config::LimitsConfig::cap_usize(config.limits.tool_result_chars);
                    let history_results = truncate_tool_results(tool_results, result_char_cap);

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
                tool => pending_calls.extend(PendingCall::from_tool_event(tool)),
            }
        }
    }
}
