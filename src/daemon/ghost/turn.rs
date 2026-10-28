//! `do_ghost_turn`, the ghost run's turn loop: the model call, the tool
//! calls, the wrap-up turn and the completion bookkeeping. A child module
//! of `src/daemon/ghost.rs`.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn do_ghost_turn(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
    run_id: &RunId,
    cancel: &crate::daemon::run::RunCancelSignal,
    outcome: &mut GhostLoopOutcome,
    metrics: &mut RunMetrics,
) -> Result<()> {
    let Some((_messages, ghost_config, tmux_session, _target_pane, ghost_active_model)) =
        with_sessions(sessions, |store| {
            let entry = store.get(session_id)?;
            Some((
                entry.messages.clone(),
                entry.ghost_config.clone(),
                entry.tmux_session.clone(),
                entry.default_target_pane.clone(),
                entry.active_model.clone(),
            ))
        })
    else {
        anyhow::bail!("Ghost Shell '{}' not found", session_id);
    };

    let prompt::GhostPrompt {
        system,
        max_ghost_turns,
        steps,
    } = prompt::build_ghost_prompt(session_id, sessions, config, &tmux_session);

    // Ghost shells never have a real IPC client — use sink/empty so that
    // send_response_split writes never block (they're discarded) and reads
    // immediately return EOF (ghost policy bypasses all approval prompts anyway).
    let mut tx = tokio::io::sink();
    let mut rx = BufReader::new(tokio::io::empty());

    let model_entry = config.resolve_model(ghost_active_model.as_deref());
    metrics.set_model(model_entry.provider.clone(), model_entry.model.clone());
    if let Some(ref name) = ghost_active_model {
        log::info!("Ghost Shell {}: using model '{}'", session_id, name);
    }
    let client: Arc<Box<dyn crate::ai::AiClient>> = Arc::new(make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
        model_entry.effective_max_tokens(),
    ));

    const GHOST_TURN_TIMEOUT_SECS: u64 = 300;

    let mut turn = 0usize;
    // Tracks whether the wrap-up turn has already been injected so we run it
    // exactly once before breaking the loop.
    let mut wrap_up_turn = false;
    loop {
        let notices = [
            park_while_paused(run_id, cancel).await,
            typed_notice(run_id),
        ];
        for notice in notices.into_iter().flatten() {
            push_user_note(session_id, sessions, notice, turn + 1);
        }
        if cancel.is_cancelled() {
            log::info!(
                "Ghost Shell {session_id}: cancelled, stopping before turn {}",
                turn + 1
            );
            outcome.cancelled = true;
            break;
        }
        if wrap_up_turn {
            // The previous iteration was the wrap-up turn — stop now.
            log::warn!(
                "Ghost Shell {}: wrap-up turn complete, stopping after {} turns (limit {})",
                session_id,
                turn,
                max_ghost_turns
            );
            break;
        }
        if turn >= max_ghost_turns {
            // Inject a synthetic user message asking the agent to wrap up, then
            // run one final turn so it can leave a clean handoff rather than
            // stopping mid-thought.
            log::warn!(
                "Ghost Shell {}: reached max turns ({}), running wrap-up turn",
                session_id,
                max_ghost_turns
            );
            let wrap_up = crate::ai::Message {
                role: "user".to_string(),
                content: format!(
                    "[BUDGET EXHAUSTED — turn {0}/{0} reached. This is your final turn. \
                     Summarize what was accomplished, record any critical state or follow-ups \
                     to memory (add_memory), and stop. Do not call run_terminal_command, \
                     edit_file, write_script, schedule_command, or spawn_ghost_shell in this turn.]",
                    max_ghost_turns
                ),
                tool_calls: None,
                tool_results: None,
                turn: Some(turn + 1),
            };
            let pushed = with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages.push(wrap_up.clone());
                    true
                } else {
                    false
                }
            });
            if pushed {
                crate::daemon::session::append_session_message(session_id, &wrap_up);
            }
            wrap_up_turn = true;
            outcome.budget_exhausted = true;
        }
        turn += 1;
        outcome.turns = turn as u32;
        metrics.record_turn();
        emit_turn_started(run_id, turn);

        // The daily cap gates every ghost turn, before the model call: the
        // refusal ends the loop without a wrap-up turn, and
        // `ghost_terminal_state` turns it into `BudgetExceeded` — an error
        // and a cancellation still beat it. The check sits after the turn
        // counter so the recorded `turns` includes the refused one, and
        // before `make_client` so nothing reaches a provider.
        if let Err(message) =
            crate::daemon::spend::cost_gate(config, crate::daemon::spend::SpendPath::Ghost)
        {
            log::warn!(
                "Ghost Shell {}: daily cost cap refuses turn {}: {message}",
                session_id,
                turn
            );
            outcome.cost_refused = Some(message);
            break;
        }

        log::info!(
            "Ghost Shell {}: starting turn {}/{}{}",
            session_id,
            turn,
            max_ghost_turns,
            if wrap_up_turn { " (wrap-up)" } else { "" }
        );

        let Some((messages, loaded_tools, token_scale, started_at)) =
            with_sessions(sessions, |store| {
                let entry = store.get(session_id)?;
                Some((
                    entry.messages.clone(),
                    entry.loaded_tools.iter().cloned().collect::<Vec<String>>(),
                    entry.token_scale,
                    entry.started_at,
                ))
            })
        else {
            break;
        };

        let (chat_messages, compacted) =
            crate::daemon::context::ghost_ws::enforce_ghost_working_set(
                session_id,
                messages,
                token_scale,
                started_at,
                model_entry.context_window(),
                config,
            );
        if compacted {
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages = chat_messages.clone();
                }
            });
            write_session_file(session_id, &chat_messages);
        }

        let client_clone = Arc::clone(&client);
        let system_clone = system.clone();

        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        tokio::spawn(async move {
            if let Err(e) = client_clone
                .chat(&system_clone, chat_messages, ai_tx, true, loaded_tools)
                .await
            {
                log::error!("Ghost Shell AI error: {}", e);
            }
        });

        let mut assistant_content = String::new();
        let mut pending_calls = Vec::new();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(GHOST_TURN_TIMEOUT_SECS);

        loop {
            match tokio::time::timeout_at(deadline, ai_rx.recv()).await {
                Err(_elapsed) => {
                    log::error!(
                        "Ghost Shell {}: turn {} timed out after {}s",
                        session_id,
                        turn,
                        GHOST_TURN_TIMEOUT_SECS
                    );
                    crate::daemon::utils::log_event(
                        "ghost_error",
                        serde_json::json!({
                            "session_id": session_id,
                            "turn": turn,
                            "error": format!("turn timed out after {}s", GHOST_TURN_TIMEOUT_SECS),
                        }),
                    );
                    anyhow::bail!("ghost turn timed out");
                }
                Ok(None) => break,
                Ok(Some(ev)) => match ev {
                    AiEvent::Token(t) => {
                        assistant_content.push_str(&t);
                    }
                    AiEvent::ToolCall {
                        id,
                        cmd: command,
                        wait: _wait,
                        interactive,
                        thought_signature,
                        shell: _shell,
                        host,
                        timeout_secs: _timeout_secs,
                    } => {
                        pending_calls.push(ghost_pending_for_tool_call(
                            id,
                            command,
                            interactive,
                            thought_signature,
                            host,
                        ));
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
                    AiEvent::ReadRunbook {
                        id,
                        thought_signature,
                        name,
                    } => {
                        pending_calls.push(PendingCall::ReadRunbook {
                            id,
                            thought_signature,
                            name,
                        });
                    }
                    AiEvent::SearchRepository {
                        id,
                        thought_signature,
                        query,
                        kind,
                    } => {
                        pending_calls.push(PendingCall::SearchRepository {
                            id,
                            thought_signature,
                            query,
                            kind,
                        });
                    }
                    AiEvent::RecallContext {
                        id,
                        thought_signature,
                        query,
                        turn_start,
                        turn_end,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::RecallContext {
                            id,
                            thought_signature,
                            query,
                            turn_start,
                            turn_end,
                            scope,
                        });
                    }
                    AiEvent::ListMemories {
                        id,
                        thought_signature,
                        category,
                    } => {
                        pending_calls.push(PendingCall::ListMemories {
                            id,
                            thought_signature,
                            category,
                        });
                    }
                    AiEvent::ReadMemory {
                        id,
                        thought_signature,
                        key,
                        category,
                    } => {
                        pending_calls.push(PendingCall::ReadMemory {
                            id,
                            thought_signature,
                            key,
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
                    AiEvent::GetShellContext {
                        id,
                        thought_signature,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::GetShellContext {
                            id,
                            thought_signature,
                            scope,
                        });
                    }
                    AiEvent::ListShells {
                        id,
                        thought_signature,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::ListShells {
                            id,
                            thought_signature,
                            scope,
                        });
                    }
                    AiEvent::WriteRunbook {
                        id,
                        thought_signature,
                        name,
                        content,
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
                        thought_signature,
                        name,
                    } => {
                        pending_calls.push(PendingCall::DeleteRunbook {
                            id,
                            thought_signature,
                            name,
                        });
                    }
                    AiEvent::WriteScript {
                        id,
                        thought_signature,
                        script_name,
                        content,
                    } => {
                        pending_calls.push(PendingCall::WriteScript {
                            id,
                            thought_signature,
                            script_name,
                            content,
                        });
                    }
                    AiEvent::DeleteScript {
                        id,
                        thought_signature,
                        script_name,
                    } => {
                        pending_calls.push(PendingCall::DeleteScript {
                            id,
                            thought_signature,
                            script_name,
                        });
                    }
                    AiEvent::ScheduleCommand {
                        id,
                        thought_signature,
                        name,
                        command,
                        is_script,
                        run_at,
                        interval,
                        runbook,
                        ghost_runbook,
                        cron,
                    } => {
                        pending_calls.push(PendingCall::ScheduleCommand {
                            id,
                            thought_signature,
                            name,
                            command,
                            is_script,
                            run_at,
                            interval,
                            runbook,
                            ghost_runbook,
                            cron,
                        });
                    }
                    AiEvent::EditFile {
                        id,
                        thought_signature,
                        path,
                        operation,
                        old_string,
                        new_string,
                        content,
                        dest_path,
                        host,
                        sudo,
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
                            host,
                            sudo,
                        });
                    }
                    AiEvent::SpawnGhost {
                        id,
                        runbook,
                        message,
                        agent,
                        host,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::SpawnGhost {
                            id,
                            thought_signature,
                            runbook,
                            message,
                            agent,
                            host,
                        });
                    }
                    AiEvent::AwaitAgentResult {
                        id,
                        job_id,
                        agent_name,
                        timeout_secs,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::AwaitAgentResult {
                            id,
                            thought_signature,
                            job_id,
                            agent_name,
                            timeout_secs,
                        });
                    }
                    AiEvent::InvalidToolArgs {
                        id,
                        name,
                        arguments,
                        error,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::InvalidArgs {
                            id,
                            name,
                            arguments,
                            error,
                            thought_signature,
                        });
                    }
                    AiEvent::Done(usage) => {
                        let attribution =
                            crate::cost::CostAttribution::from_ghost_config(ghost_config.as_ref());
                        let record = crate::cost::emit_cost(crate::cost::CostCall {
                            model_entry,
                            usage: &usage,
                            attribution: &attribution,
                            session_id,
                            run_id: run_id.as_str(),
                            now: chrono::Utc::now(),
                        });
                        metrics.record_model_call(
                            model_entry.provider.clone(),
                            model_entry.model.clone(),
                            &usage,
                            record.cost.total_cost_usd,
                        );

                        // Accumulate cost on the session entry.
                        with_sessions(sessions, |store| {
                            if let Some(entry) = store.get_mut(session_id) {
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
                        break;
                    }
                    AiEvent::Error(e) => {
                        metrics.record_backend_error();
                        crate::daemon::utils::log_event(
                            "ghost_error",
                            serde_json::json!({
                                "session_id": session_id,
                                "turn": turn,
                                "error": format!("AI error: {e}"),
                            }),
                        );
                        anyhow::bail!("AI error: {}", e);
                    }
                    _ => {}
                },
            }
        }

        // Log what tools this turn will execute before dispatching.
        if !pending_calls.is_empty() {
            for call in &pending_calls {
                let detail = match call {
                    PendingCall::Background { cmd, .. } => format!(": {}", cmd),
                    _ => String::new(),
                };
                log::info!(
                    "Ghost Shell {}: turn {} dispatching '{}'{detail}",
                    session_id,
                    turn,
                    call.tool_name(),
                );
            }
            crate::daemon::utils::log_event(
                "ghost_turn",
                serde_json::json!({
                    "session_id": session_id,
                    "turn": turn,
                    "tool_count": pending_calls.len(),
                    "tools": pending_calls.iter().map(|c| {
                        let mut obj = serde_json::Map::new();
                        obj.insert("name".to_string(), serde_json::Value::String(c.tool_name().to_string()));
                        if let PendingCall::Background { cmd, .. } = c {
                            obj.insert("cmd".to_string(), serde_json::Value::String(cmd.clone()));
                        }
                        serde_json::Value::Object(obj)
                    }).collect::<Vec<_>>(),
                }),
            );
        }

        let mut tool_results: Vec<ToolResult> = Vec::new();

        for call in &pending_calls {
            let outcome = crate::daemon::executor::execute_tool_call(
                call,
                &mut tx,
                &mut rx,
                crate::daemon::executor::SessionCtx {
                    session_id: Some(session_id),
                    session_name: &tmux_session,
                    chat_pane: None,
                    sessions,
                },
                cache,
                schedule_store,
            )
            .await?;

            crate::daemon::executor::tally_tool_outcome(metrics, call, &outcome);

            match outcome {
                crate::daemon::executor::ToolCallOutcome::Result(r) => {
                    tool_results.push(ToolResult {
                        tool_call_id: call.id().to_string(),
                        tool_name: call.tool_name().to_string(),
                        content: r,
                    });
                }
                crate::daemon::executor::ToolCallOutcome::SpawnGhostSession {
                    session_id: ghost_sid,
                    runbook_name: _,
                    tool_result,
                    job_id: _,
                } => {
                    let sessions2 = sessions.clone();
                    let cache2 = Arc::clone(cache);
                    let store2 = Arc::clone(schedule_store);
                    let config2 = config.clone();
                    match Box::pin(trigger_ghost_turn(
                        &ghost_sid, &sessions2, &config2, &cache2, &store2,
                    ))
                    .await
                    {
                        Ok(()) => {}
                        Err(e) => {
                            log::error!("nested SpawnGhost failed for {}: {}", ghost_sid, e);
                            crate::daemon::utils::log_event(
                                "ghost_error",
                                serde_json::json!({
                                    "session_id": session_id,
                                    "turn": turn,
                                    "error": format!("nested SpawnGhost failed for {ghost_sid}: {e}"),
                                }),
                            );
                        }
                    }
                    tool_results.push(ToolResult {
                        tool_call_id: call.id().to_string(),
                        tool_name: call.tool_name().to_string(),
                        content: tool_result,
                    });
                }
                crate::daemon::executor::ToolCallOutcome::UserMessage(_) => {}
            }
        }

        if !tool_results.is_empty()
            && tool_results
                .iter()
                .all(|r| r.content.starts_with("Command denied by Ghost Policy"))
        {
            log::warn!(
                "Ghost Shell {}: all {} tool call(s) denied by ghost policy on turn {} — \
                 runbook may need auto_approve_scripts (for sudo) or auto_approve_commands: true (for non-sudo investigation commands)",
                session_id,
                tool_results.len(),
                turn,
            );
        }

        let assistant_msg = Message {
            role: "assistant".to_string(),
            content: assistant_content,
            tool_calls: if pending_calls.is_empty() {
                None
            } else {
                Some(pending_calls.iter().map(|c| c.to_tool_call()).collect())
            },
            tool_results: if tool_results.is_empty() {
                None
            } else {
                Some(tool_results)
            },
            turn: Some(turn),
        };

        append_session_message(session_id, &assistant_msg);
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(session_id) {
                entry.messages.push(assistant_msg);
                entry.last_accessed = Instant::now();
            }
        });

        if pending_calls.is_empty() {
            if !steps.is_empty() {
                let transcript = with_sessions(sessions, |store| {
                    store
                        .get(session_id)
                        .map(|entry| {
                            entry
                                .messages
                                .iter()
                                .filter(|m| m.role == "assistant")
                                .map(|m| m.content.clone())
                                .collect::<Vec<String>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                });
                let uncovered = uncovered_steps(&transcript, steps.len());
                if !uncovered.is_empty() {
                    let names: Vec<String> = uncovered.iter().map(|n| n.to_string()).collect();
                    let nudge = crate::ai::Message {
                        role: "user".to_string(),
                        content: format!(
                            "[SEEDED STEPS INCOMPLETE — steps {} have no {} <n>: DONE or \
                             {} <n>: SKIPPED <reason> line. Emit one for each, then finish. \
                             Do not re-run work already done.]",
                            names.join(", "),
                            STEP_MARKER_PREFIX,
                            STEP_MARKER_PREFIX
                        ),
                        tool_calls: None,
                        tool_results: None,
                        turn: Some(turn + 1),
                    };
                    append_session_message(session_id, &nudge);
                    with_sessions(sessions, |store| {
                        if let Some(entry) = store.get_mut(session_id) {
                            entry.messages.push(nudge);
                            entry.last_accessed = Instant::now();
                        }
                    });
                    log::warn!(
                        "Ghost Shell {}: steps {} uncovered, nudging instead of completing",
                        session_id,
                        names.join(", ")
                    );
                    continue;
                }
            }
            break;
        }
    }

    log::info!("Ghost Shell {}: completed in {} turn(s)", session_id, turn);
    let (spawn_depth, parent_job_id) = ghost_config
        .as_ref()
        .map(|gc| (gc.spawn_depth, gc.parent_job_id.clone()))
        .unwrap_or((0, None));
    crate::daemon::utils::log_event(
        "ghost_complete",
        serde_json::json!({
            "session_id": session_id,
            "turns_used": turn,
            "spawn_depth": spawn_depth,
            "parent_job_id": parent_job_id,
        }),
    );
    crate::daemon::stats::inc_ghosts_completed();

    // Briefing generation: best-effort summary for the next invocation.
    if let Some(ref gc) = ghost_config
        && let Some(ref agent_name) = gc.agent
    {
        crate::daemon::briefing::generate_and_save_briefing(
            agent_name, session_id, sessions, config,
        )
        .await;
    }

    Ok(())
}
