//! Helper routines for `run_conversation_loop`: cost-gate refusal, model task
//! spawning, cancellation persistence, final-answer persistence and auto-name
//! suggestion, and tool-batch budget checks and execution.

use super::*;

#[derive(Default)]
pub(super) struct TurnLoopState {
    pub tool_call_counts: HashMap<&'static str, u32>,
    pub total_turn_call_count: u32,
    pub turn_output_tokens: u32,
}

pub(super) async fn check_chat_cost_gate<W: AsyncWrite + Unpin>(
    ctx: &ConversationLoopCtx<'_>,
    tx: &mut W,
    run_log: &mut RunLogGuard,
) -> Result<bool> {
    if let Err(message) =
        crate::daemon::spend::cost_gate(ctx.config, crate::daemon::spend::SpendPath::Chat)
    {
        log::warn!(
            "chat turn for session {}: daily cost cap refuses the model call: {message}",
            ctx.session_id.as_deref().unwrap_or("(ghost)"),
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
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn spawn_chat_model_task(
    ctx: &ConversationLoopCtx<'_>,
    ai_tx: tokio::sync::mpsc::UnboundedSender<AiEvent>,
    run_log: &mut RunLogGuard,
) -> ChatTaskGuard {
    let active_entry = ctx
        .config
        .resolve_model(ctx.session_active_model.as_deref());
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
    let sys_prompt_turn = ctx.sys_prompt.clone();
    let messages_clone = ctx.messages.clone();

    // Read the session's on-demand tool set so deferred groups loaded via
    // `load_tools` on a prior turn are rendered for this turn (M1 phase-11).
    let loaded_tools: Vec<String> = ctx
        .session_id
        .as_deref()
        .and_then(|sid| {
            with_sessions(&ctx.sessions, |store| {
                store
                    .get(sid)
                    .map(|e| e.loaded_tools.iter().cloned().collect())
            })
        })
        .unwrap_or_default();

    ChatTaskGuard::new(tokio::spawn(async move {
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
    }))
}

pub(super) async fn handle_turn_cancelled<W: AsyncWrite + Unpin>(
    ctx: &mut ConversationLoopCtx<'_>,
    tx: &mut W,
    run_log: &mut RunLogGuard,
    reason: crate::daemon::run::CancelReason,
    full_response: &str,
) -> Result<()> {
    let stage = cancel_stage(run_log.run_id());
    if !full_response.is_empty() {
        ctx.messages.push(Message {
            role: "assistant".to_string(),
            content: format!("{full_response}\n\n[⊘ cancelled by user]"),
            tool_calls: None,
            tool_results: None,
            turn: Some(ctx.this_turn_count),
        });
    }
    // Persist the partial response so the interrupted turn leaves
    // a trace in the session history on disk.
    if let Some(ref id) = ctx.session_id {
        let msgs_snapshot = ctx.messages.clone();
        with_sessions(&ctx.sessions, |store| {
            if let Some(entry) = store.get_mut(id) {
                entry.finish_turn(msgs_snapshot);
                entry.persistence.dirty = true;
            }
        });
        for msg in &ctx.messages[ctx.post_trim_len..] {
            append_session_message(id, msg);
        }
    }
    log::info!(
        "turn cancelled at stage {stage} via {reason:?} for session {:?}",
        ctx.session_id.as_deref().unwrap_or("(ghost)"),
    );
    send_response_split(tx, Response::SystemMsg("⊘ cancelled".to_string())).await?;
    send_response_split(tx, Response::Ok).await?;
    run_log.finish(crate::daemon::run::RunState::Cancelled {
        stage,
        turns: ctx.this_turn_count as u32,
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn collect_terminal_tool_call(
    id: String,
    cmd: String,
    wait: Option<bool>,
    interactive: bool,
    thought_signature: Option<String>,
    shell: Option<String>,
    host: Option<String>,
    timeout_secs: Option<u64>,
) -> PendingCall {
    if routes_to_background(wait, interactive) {
        // `wait: false` stays on the daemon host; a named host
        // on a background command is row 08b's surface, so the
        // value is dropped here (sre.toml promises this).
        let _ = host;
        PendingCall::Background {
            id,
            cmd,
            _credential: None,
            thought_signature,
            host: None,
        }
    } else {
        PendingCall::Foreground {
            id,
            cmd,
            shell,
            host,
            timeout_secs,
            interactive,
            thought_signature,
        }
    }
}

fn persist_final_turn_history(ctx: &ConversationLoopCtx<'_>, id: &str, usage: &TokenBreakdown) {
    let mut merged_messages: Vec<Message> = ctx.messages.clone();
    with_sessions(&ctx.sessions, |store| {
        if let Some(entry) = store.get_mut(id) {
            entry.finish_turn(ctx.messages.clone());
            merged_messages = entry.history.messages.clone();
            entry.persistence.last_accessed = Instant::now();
            entry.ledger.last_prompt_tokens =
                usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
            crate::daemon::context::estimate::update_token_scale(entry, &ctx.messages);
            entry.persistence.dirty = true;
            if ctx.chat_pane.is_some()
                && let Some(i) = entry.as_interactive_mut()
            {
                i.chat_pane = ctx.chat_pane.clone();
            }
        }
    });
    if ctx.needs_compaction {
        write_session_file(id, &merged_messages);
    } else {
        for msg in &ctx.messages[ctx.post_trim_len..] {
            append_session_message(id, msg);
        }
    }
    if !ctx.is_ghost_session {
        let meta = with_sessions(&ctx.sessions, |store| {
            store
                .get(id)
                .map(|entry| crate::daemon::session::SessionMeta {
                    started_at: entry.persistence.started_at,
                    turn_count: entry.history.turn_count,
                    last_prompt_tokens: entry.ledger.last_prompt_tokens,
                    token_scale: entry.ledger.token_scale,
                    tool_calls_this_session: entry.ledger.tool_calls_this_session,
                    saved_name: entry.persistence.saved_name.clone(),
                })
        });
        if let Some(meta) = meta {
            crate::daemon::session::write_session_meta(id, &meta);
        }
    }
    if ctx.wants_background_compaction {
        crate::daemon::context::background::spawn_compaction(
            id.to_string(),
            ctx.sessions.clone(),
            Arc::new(ctx.config.clone()),
        );
    }
}

async fn maybe_suggest_auto_name<W: AsyncWrite + Unpin>(
    ctx: &ConversationLoopCtx<'_>,
    tx: &mut W,
) -> Result<()> {
    if !ctx.is_ghost_session
        && ctx.config.sessions.auto_name_enabled
        && ctx.config.sessions.auto_name_turn_threshold > 0
    {
        let should_suggest = with_sessions(&ctx.sessions, |store| {
            if let Some(ref id) = ctx.session_id
                && let Some(entry) = store.get_mut(id)
                && entry.persistence.saved_name.is_none()
                && entry.history.turn_count == ctx.config.sessions.auto_name_turn_threshold
                && let Some(i) = entry.as_interactive_mut()
                && !i.auto_name_suggested
            {
                i.auto_name_suggested = true;
                true
            } else {
                false
            }
        });
        if should_suggest {
            let suggestion: Option<(String, String)> = crate::daemon::utils::with_keepalive(
                tx,
                auto_name::suggest_session_name(&ctx.messages, ctx.config),
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
    Ok(())
}

async fn finish_final_answer<W: AsyncWrite + Unpin>(
    ctx: &mut ConversationLoopCtx<'_>,
    tx: &mut W,
    full_response: &str,
    usage: &TokenBreakdown,
    state: &mut TurnLoopState,
    run_log: &mut RunLogGuard,
) -> Result<()> {
    if full_response.is_empty() {
        send_response_split(
            tx,
            Response::SystemMsg("The model returned an empty reply.".to_string()),
        )
        .await?;
    } else {
        ctx.messages.push(Message {
            role: "assistant".to_string(),
            content: full_response.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(ctx.this_turn_count),
        });
    }

    record_model_call(
        ctx.config,
        ctx.session_active_model.as_deref(),
        usage,
        &ctx.cost_attribution,
        ctx.session_id.as_deref(),
        run_log,
        &ctx.sessions,
    );
    state.turn_output_tokens = state.turn_output_tokens.saturating_add(usage.output_tokens);

    if let Some(ref id) = ctx.session_id {
        persist_final_turn_history(ctx, id, usage);
    }

    maybe_suggest_auto_name(ctx, tx).await?;

    send_response_split(
        tx,
        Response::UsageUpdate {
            prompt_tokens: usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens,
            output_tokens: state.turn_output_tokens,
        },
    )
    .await?;
    send_response_split(tx, Response::Ok).await?;
    run_log.finish(crate::daemon::run::RunState::Complete);
    Ok(())
}

enum ToolBudgetDecision {
    Allowed(Option<String>),
    Blocked(ToolResult),
}

fn check_tool_call_budgets(
    call: &PendingCall,
    ctx: &ConversationLoopCtx<'_>,
    state: &mut TurnLoopState,
) -> ToolBudgetDecision {
    if is_budget_exempt(call) {
        return ToolBudgetDecision::Allowed(None);
    }
    let call_id = call.id().to_string();
    let tool_name = call.tool_name();
    state.total_turn_call_count += 1;

    // Per-session cumulative cap across all non-approval-gated tools.
    if let Some(session_limit) =
        crate::config::LimitsConfig::cap_usize(ctx.config.limits.max_tool_calls_per_session)
    {
        let session_tool_count = ctx
            .session_id
            .as_deref()
            .and_then(|id| {
                with_sessions(&ctx.sessions, |store| {
                    store.get(id).map(|e| e.ledger.tool_calls_this_session)
                })
            })
            .unwrap_or(0);
        if session_tool_count >= session_limit {
            log::warn!(
                "Per-session tool cap ({session_limit}) reached; \
                 blocking `{tool_name}`"
            );
            return ToolBudgetDecision::Blocked(ToolResult {
                tool_call_id: call_id,
                tool_name: tool_name.to_string(),
                content: format!(
                    "Error: the per-session tool call limit \
                     ({session_limit}) has been reached. This call was \
                     not executed. No further tool calls can be made in \
                     this session."
                ),
            });
        }
    }

    // Per-turn total cap across all non-approval-gated tools.
    if let Some(total_limit) =
        crate::config::LimitsConfig::cap_u32(ctx.config.limits.total_tool_calls_per_turn)
        && state.total_turn_call_count > total_limit
    {
        log::warn!(
            "Per-turn total tool cap ({total_limit}) reached; \
             blocking `{tool_name}`"
        );
        return ToolBudgetDecision::Blocked(ToolResult {
            tool_call_id: call_id,
            tool_name: tool_name.to_string(),
            content: format!(
                "Error: the per-turn total tool call limit \
                 ({total_limit}) has been reached. This call was \
                 not executed. Summarise what you have gathered \
                 so far and stop calling tools this turn."
            ),
        });
    }

    // Per-tool batch cap (same tool repeated within one turn).
    let mut tool_budget_hint = None;
    if let Some(limit) = ctx.config.limits.per_tool_cap(tool_name) {
        let count = state.tool_call_counts.entry(tool_name).or_insert(0);
        *count += 1;
        if *count > limit {
            log::warn!(
                "Per-tool limit for `{tool_name}` reached ({limit}); \
                 blocking call"
            );
            return ToolBudgetDecision::Blocked(ToolResult {
                tool_call_id: call_id,
                tool_name: tool_name.to_string(),
                content: format!(
                    "Error: `{tool_name}` has been called {limit} times \
                     this turn. This call was not executed. Proceed with \
                     the information already gathered and do not call this \
                     tool again."
                ),
            });
        }
        if *count * 2 >= limit {
            tool_budget_hint = Some(format!(
                "\n\n[note: {}/{} `{}` calls this turn — approaching per-turn cap]",
                *count, limit, tool_name
            ));
        }
    }
    ToolBudgetDecision::Allowed(tool_budget_hint)
}

fn spawn_chat_ghost_session(ctx: &ConversationLoopCtx<'_>, ghost_sid: String, ghost_rb: String) {
    let ghost_sessions = ctx.sessions.clone();
    let ghost_cache = Arc::clone(&ctx.cache);
    let ghost_store = Arc::clone(&ctx.schedule_store);
    let ghost_config = ctx.config.clone();
    tokio::spawn(async move {
        let session_log = crate::daemon::session::session_file(&ghost_sid)
            .display()
            .to_string();
        match crate::daemon::ghost::trigger_ghost_turn(
            &ghost_sid,
            &ghost_sessions,
            &ghost_config,
            &ghost_cache,
            &ghost_store,
        )
        .await
        {
            Ok(()) => {
                crate::webhook::publish_ghost_event(
                    crate::bus::global(),
                    crate::webhook::GhostEvent::Completed,
                    &ghost_rb,
                    &format!(
                        "[Ghost Shell Completed] AI-requested ghost shell finished for runbook: {} — session log: {}",
                        ghost_rb, session_log
                    ),
                );
            }
            Err(e) => {
                log::error!("SpawnGhost: ghost turn failed for {}: {}", ghost_sid, e);
                crate::webhook::publish_ghost_event(
                    crate::bus::global(),
                    crate::webhook::GhostEvent::Failed,
                    &ghost_rb,
                    &format!(
                        "[Ghost Shell Failed] AI-requested ghost shell error for runbook: {} — {} — session log: {}",
                        ghost_rb, e, session_log
                    ),
                );
            }
        }
    });
}

pub(super) enum ToolBatchControl {
    ContinueNextModelTurn,
    EndTurn(Result<()>),
}

async fn execute_chat_tool_batch<W, R>(
    ctx: &mut ConversationLoopCtx<'_>,
    pending_calls: &[PendingCall],
    tx: &mut W,
    rx: &mut R,
    state: &mut TurnLoopState,
    run_log: &mut RunLogGuard,
) -> ToolBatchControl
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let mut tool_results = Vec::new();
    let mut user_message_redirect: Option<String> = None;

    for call in pending_calls.iter() {
        let call_id = call.id().to_string();
        let tool_name = call.tool_name();
        let tool_budget_hint = match check_tool_call_budgets(call, ctx, state) {
            ToolBudgetDecision::Allowed(hint) => hint,
            ToolBudgetDecision::Blocked(blocked_res) => {
                tool_results.push(blocked_res);
                continue;
            }
        };

        let outcome = match executor::execute_tool_call(
            call,
            tx,
            rx,
            SessionCtx::new(
                ctx.session_id.as_deref(),
                ctx.session_name,
                ctx.chat_pane.as_deref(),
                &ctx.sessions,
                crate::shell::registry(),
            ),
            &ctx.cache,
            &ctx.schedule_store,
        )
        .await
        {
            Ok(res) => res,
            Err(e) => {
                let result_char_cap =
                    crate::config::LimitsConfig::cap_usize(ctx.config.limits.tool_result_chars);
                let gone = endings::ClientGoneTurn {
                    session_id: ctx.session_id.as_deref(),
                    post_trim_len: ctx.post_trim_len,
                    this_turn_count: ctx.this_turn_count,
                    tool_name: call.tool_name(),
                    error: &e,
                };
                let cstate = endings::ClientGoneState {
                    messages: &mut ctx.messages,
                    pending_calls,
                    tool_results,
                    result_char_cap,
                    sessions: &ctx.sessions,
                };
                return ToolBatchControl::EndTurn(
                    endings::end_client_gone(tx, gone, cstate, run_log).await,
                );
            }
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
                if !is_budget_exempt(call)
                    && let Some(ref id) = ctx.session_id
                {
                    with_sessions(&ctx.sessions, |store| {
                        if let Some(entry) = store.get_mut(id) {
                            entry.ledger.tool_calls_this_session += 1;
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
                spawn_chat_ghost_session(ctx, ghost_sid, ghost_rb);
                tool_results.push(ToolResult {
                    tool_call_id: call_id,
                    tool_name: tool_name.to_string(),
                    content: tool_result,
                });
            }
        }
    }

    if let Some(user_msg) = user_message_redirect {
        ctx.messages.pop();
        ctx.messages.push(Message {
            role: "user".to_string(),
            content: user_msg,
            tool_calls: None,
            tool_results: None,
            turn: Some(ctx.this_turn_count),
        });
        return ToolBatchControl::ContinueNextModelTurn;
    }

    let result_char_cap =
        crate::config::LimitsConfig::cap_usize(ctx.config.limits.tool_result_chars);
    let history_results = truncate_tool_results(tool_results, result_char_cap);
    ctx.messages.push(Message {
        role: "user".to_string(),
        content: String::new(),
        tool_calls: None,
        tool_results: Some(history_results),
        turn: Some(ctx.this_turn_count),
    });
    ToolBatchControl::ContinueNextModelTurn
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_chat_done_event<W, R>(
    ctx: &mut ConversationLoopCtx<'_>,
    tx: &mut W,
    rx: &mut R,
    usage: TokenBreakdown,
    full_response: &str,
    pending_calls: &[PendingCall],
    state: &mut TurnLoopState,
    run_log: &mut RunLogGuard,
) -> ToolBatchControl
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    if usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens == 0 {
        log::warn!(
            "provider reported no token usage for model {} (session {}); \
             token-pressure compaction cannot run this turn",
            ctx.config
                .resolve_model(ctx.session_active_model.as_deref())
                .model,
            ctx.session_id.as_deref().unwrap_or("-"),
        );
    }
    if pending_calls.is_empty() {
        return ToolBatchControl::EndTurn(
            finish_final_answer(ctx, tx, full_response, &usage, state, run_log).await,
        );
    }

    record_model_call(
        ctx.config,
        ctx.session_active_model.as_deref(),
        &usage,
        &ctx.cost_attribution,
        ctx.session_id.as_deref(),
        run_log,
        &ctx.sessions,
    );
    state.turn_output_tokens = state.turn_output_tokens.saturating_add(usage.output_tokens);

    if let Some(ref id) = ctx.session_id {
        let msgs_ref = &ctx.messages;
        with_sessions(&ctx.sessions, |store| {
            if let Some(entry) = store.get_mut(id) {
                entry.ledger.last_prompt_tokens =
                    usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
                crate::daemon::context::estimate::update_token_scale(entry, msgs_ref);
            }
        });
    }

    ctx.messages.push(Message {
        role: "assistant".to_string(),
        content: full_response.to_string(),
        tool_calls: Some(pending_calls.iter().map(|c| c.to_tool_call()).collect()),
        tool_results: None,
        turn: Some(ctx.this_turn_count),
    });

    execute_chat_tool_batch(ctx, pending_calls, tx, rx, state, run_log).await
}
