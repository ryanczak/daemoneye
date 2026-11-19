//! Helper routines for `do_ghost_turn`: wrap-up message injection, working-set
//! compaction, AI stream collection and tool-event conversion, tool-batch
//! logging and execution, and seeded-step coverage nudging.

use super::*;

const GHOST_TURN_TIMEOUT_SECS: u64 = 300;

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_ghost_turn_step(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    run_id: &RunId,
    cancel: &crate::daemon::run::RunCancelSignal,
    max_ghost_turns: usize,
    turn: &mut usize,
    wrap_up_turn: &mut bool,
    outcome: &mut GhostLoopOutcome,
    metrics: &mut RunMetrics,
) -> bool {
    for notice in [
        park_while_paused(run_id, cancel).await,
        typed_notice(run_id),
    ]
    .into_iter()
    .flatten()
    {
        push_user_note(session_id, sessions, notice, *turn + 1);
    }
    if cancel.is_cancelled() {
        log::info!(
            "Ghost Shell {session_id}: cancelled, stopping before turn {}",
            *turn + 1
        );
        outcome.cancelled = true;
        return false;
    }
    if *wrap_up_turn {
        log::warn!(
            "Ghost Shell {}: wrap-up turn complete, stopping after {} turns (limit {})",
            session_id,
            *turn,
            max_ghost_turns
        );
        return false;
    }
    if *turn >= max_ghost_turns {
        inject_ghost_wrap_up_message(session_id, sessions, max_ghost_turns, *turn);
        *wrap_up_turn = true;
        outcome.budget_exhausted = true;
    }
    *turn += 1;
    outcome.turns = *turn as u32;
    metrics.record_turn();
    emit_turn_started(run_id, *turn);

    if let Err(message) =
        crate::daemon::spend::cost_gate(config, crate::daemon::spend::SpendPath::Ghost)
    {
        log::warn!(
            "Ghost Shell {}: daily cost cap refuses turn {}: {message}",
            session_id,
            *turn
        );
        outcome.cost_refused = Some(message);
        return false;
    }

    log::info!(
        "Ghost Shell {}: starting turn {}/{}{}",
        session_id,
        *turn,
        max_ghost_turns,
        if *wrap_up_turn { " (wrap-up)" } else { "" }
    );
    true
}

pub(super) async fn finish_ghost_turn_loop(
    session_id: &str,
    turn: usize,
    ghost_config: &crate::ipc::GhostConfig,
    sessions: &SessionStore,
    config: &Config,
) {
    log::info!("Ghost Shell {}: completed in {} turn(s)", session_id, turn);
    crate::daemon::utils::log_event(
        "ghost_complete",
        serde_json::json!({
            "session_id": session_id,
            "turns_used": turn,
            "spawn_depth": ghost_config.spawn_depth,
            "parent_job_id": ghost_config.parent_job_id.clone(),
        }),
    );

    if let Some(ref agent_name) = ghost_config.agent {
        crate::daemon::briefing::generate_and_save_briefing(
            agent_name, session_id, sessions, config,
        )
        .await;
    }
}

fn inject_ghost_wrap_up_message(
    session_id: &str,
    sessions: &SessionStore,
    max_ghost_turns: usize,
    turn: usize,
) {
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
            entry.history.messages.push(wrap_up.clone());
            true
        } else {
            false
        }
    });
    if pushed {
        crate::daemon::session::append_session_message(session_id, &wrap_up);
    }
}

pub(super) fn load_and_compact_ghost_messages(
    session_id: &str,
    sessions: &SessionStore,
    context_window: u32,
    config: &Config,
) -> Option<(Vec<Message>, Vec<String>)> {
    let (messages, loaded_tools, token_scale, started_at) = with_sessions(sessions, |store| {
        let entry = store.get(session_id)?;
        Some((
            entry.history.messages.clone(),
            entry.loaded_tools.iter().cloned().collect::<Vec<String>>(),
            entry.ledger.token_scale,
            entry.persistence.started_at,
        ))
    })?;

    let (chat_messages, compacted) = crate::daemon::context::ghost_ws::enforce_ghost_working_set(
        session_id,
        messages,
        token_scale,
        started_at,
        context_window,
        config,
    );
    if compacted {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(session_id) {
                entry.history.messages = chat_messages.clone();
            }
        });
        write_session_file(session_id, &chat_messages);
    }
    Some((chat_messages, loaded_tools))
}

#[allow(clippy::result_large_err)]
fn ghost_read_event_to_pending_call(ev: AiEvent) -> Result<PendingCall, AiEvent> {
    match ev {
        AiEvent::ToolCall {
            id,
            cmd: command,
            wait: _wait,
            interactive,
            thought_signature,
            shell: _shell,
            host,
            timeout_secs: _timeout_secs,
        } => Ok(ghost_pending_for_tool_call(
            id,
            command,
            interactive,
            thought_signature,
            host,
        )),
        AiEvent::ListRunbooks {
            id,
            thought_signature,
        } => Ok(PendingCall::ListRunbooks {
            id,
            thought_signature,
        }),
        AiEvent::ReadRunbook {
            id,
            thought_signature,
            name,
        } => Ok(PendingCall::ReadRunbook {
            id,
            thought_signature,
            name,
        }),
        AiEvent::SearchRepository {
            id,
            thought_signature,
            query,
            kind,
        } => Ok(PendingCall::SearchRepository {
            id,
            thought_signature,
            query,
            kind,
        }),
        AiEvent::RecallContext {
            id,
            thought_signature,
            query,
            turn_start,
            turn_end,
            scope,
        } => Ok(PendingCall::RecallContext {
            id,
            thought_signature,
            query,
            turn_start,
            turn_end,
            scope,
        }),
        AiEvent::ListMemories {
            id,
            thought_signature,
            category,
        } => Ok(PendingCall::ListMemories {
            id,
            thought_signature,
            category,
        }),
        AiEvent::ReadMemory {
            id,
            thought_signature,
            key,
            category,
        } => Ok(PendingCall::ReadMemory {
            id,
            thought_signature,
            key,
            category,
        }),
        AiEvent::GetShellContext {
            id,
            thought_signature,
            scope,
        } => Ok(PendingCall::GetShellContext {
            id,
            thought_signature,
            scope,
        }),
        AiEvent::ListShells {
            id,
            thought_signature,
            scope,
        } => Ok(PendingCall::ListShells {
            id,
            thought_signature,
            scope,
        }),
        other => Err(other),
    }
}

#[allow(clippy::result_large_err)]
fn ghost_artifact_event_to_pending_call(ev: AiEvent) -> Result<PendingCall, AiEvent> {
    match ev {
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
        } => Ok(PendingCall::UpdateMemory {
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
        }),
        AiEvent::WriteRunbook {
            id,
            thought_signature,
            name,
            content,
        } => Ok(PendingCall::WriteRunbook {
            id,
            thought_signature,
            name,
            content,
        }),
        AiEvent::DeleteRunbook {
            id,
            thought_signature,
            name,
        } => Ok(PendingCall::DeleteRunbook {
            id,
            thought_signature,
            name,
        }),
        AiEvent::WriteScript {
            id,
            thought_signature,
            script_name,
            content,
        } => Ok(PendingCall::WriteScript {
            id,
            thought_signature,
            script_name,
            content,
        }),
        AiEvent::DeleteScript {
            id,
            thought_signature,
            script_name,
        } => Ok(PendingCall::DeleteScript {
            id,
            thought_signature,
            script_name,
        }),
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
        } => Ok(PendingCall::ScheduleCommand {
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
        }),
        other => Err(other),
    }
}

fn ghost_exec_event_to_pending_call(ev: AiEvent) -> Option<PendingCall> {
    match ev {
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
        } => Some(PendingCall::EditFile {
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
        }),
        AiEvent::SpawnGhost {
            id,
            runbook,
            message,
            agent,
            host,
            thought_signature,
        } => Some(PendingCall::SpawnGhost {
            id,
            thought_signature,
            runbook,
            message,
            agent,
            host,
        }),
        AiEvent::AwaitAgentResult {
            id,
            job_id,
            agent_name,
            timeout_secs,
            thought_signature,
        } => Some(PendingCall::AwaitAgentResult {
            id,
            thought_signature,
            job_id,
            agent_name,
            timeout_secs,
        }),
        AiEvent::InvalidToolArgs {
            id,
            name,
            arguments,
            error,
            thought_signature,
        } => Some(PendingCall::InvalidArgs {
            id,
            name,
            arguments,
            error,
            thought_signature,
        }),
        _ => None,
    }
}

fn ghost_event_to_pending_call(ev: AiEvent) -> Option<PendingCall> {
    match ghost_read_event_to_pending_call(ev) {
        Ok(c) => Some(c),
        Err(rem) => match ghost_artifact_event_to_pending_call(rem) {
            Ok(c) => Some(c),
            Err(rem2) => ghost_exec_event_to_pending_call(rem2),
        },
    }
}

fn record_ghost_turn_cost(
    session_id: &str,
    sessions: &SessionStore,
    ghost_config: &crate::ipc::GhostConfig,
    model_entry: &crate::config::ModelEntry,
    usage: &crate::ai::TokenBreakdown,
    run_id: &RunId,
    metrics: &mut RunMetrics,
) {
    let attribution = crate::cost::CostAttribution::from_ghost_config(ghost_config);
    let record = crate::cost::emit_cost(crate::cost::CostCall {
        model_entry,
        usage,
        attribution: &attribution,
        session_id,
        run_id: run_id.as_str(),
        now: chrono::Utc::now(),
    });
    metrics.record_model_call(
        model_entry.provider.clone(),
        model_entry.model.clone(),
        usage,
        record.cost.total_cost_usd,
    );

    // Accumulate cost on the session entry.
    with_sessions(sessions, |store| {
        if let Some(l) = store.get_mut(session_id).map(|e| &mut e.ledger) {
            l.cost_usd += record.cost.total_cost_usd;
            *l.cost_by_agent
                .entry(record.agent_name.clone())
                .or_insert(0.0) += record.cost.total_cost_usd;
            if record.pricing_source == PricingSource::Unknown {
                l.has_untracked_cost = true;
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn collect_ghost_stream_turn(
    session_id: &str,
    turn: usize,
    ai_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AiEvent>,
    sessions: &SessionStore,
    ghost_config: &crate::ipc::GhostConfig,
    model_entry: &crate::config::ModelEntry,
    run_id: &RunId,
    metrics: &mut RunMetrics,
) -> Result<(String, Vec<PendingCall>)> {
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
                AiEvent::Done(usage) => {
                    record_ghost_turn_cost(
                        session_id,
                        sessions,
                        ghost_config,
                        model_entry,
                        &usage,
                        run_id,
                        metrics,
                    );
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
                other => {
                    if let Some(c) = ghost_event_to_pending_call(other) {
                        pending_calls.push(c);
                    }
                }
            },
        }
    }

    Ok((assistant_content, pending_calls))
}

pub(super) fn log_ghost_tool_batch(session_id: &str, turn: usize, pending_calls: &[PendingCall]) {
    if pending_calls.is_empty() {
        return;
    }
    for call in pending_calls {
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_ghost_tool_batch(
    session_id: &str,
    turn: usize,
    pending_calls: &[PendingCall],
    sessions: &SessionStore,
    config: &Config,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
    metrics: &mut RunMetrics,
) -> Result<Vec<ToolResult>> {
    // Ghost shells never have a real IPC client — use sink/empty so that
    // send_response_split writes never block (they're discarded) and reads
    // immediately return EOF (ghost policy bypasses all approval prompts anyway).
    let mut tx = tokio::io::sink();
    let mut rx = BufReader::new(tokio::io::empty());
    let mut tool_results: Vec<ToolResult> = Vec::new();

    for call in pending_calls {
        let outcome = crate::daemon::executor::execute_tool_call(
            call,
            &mut tx,
            &mut rx,
            crate::daemon::executor::SessionCtx::new(
                Some(session_id),
                "",
                None,
                sessions,
                crate::shell::registry(),
            ),
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
                if let Err(e) = Box::pin(trigger_ghost_turn(
                    &ghost_sid, &sessions2, &config2, &cache2, &store2,
                ))
                .await
                {
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

    Ok(tool_results)
}

pub(super) fn persist_ghost_turn_message(
    session_id: &str,
    sessions: &SessionStore,
    assistant_content: String,
    pending_calls: &[PendingCall],
    tool_results: Vec<ToolResult>,
    turn: usize,
) {
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
            entry.history.messages.push(assistant_msg);
            entry.persistence.last_accessed = Instant::now();
        }
    });
}

pub(super) fn maybe_nudge_uncovered_steps(
    session_id: &str,
    sessions: &SessionStore,
    step_count: usize,
    turn: usize,
) -> bool {
    if step_count == 0 {
        return false;
    }
    let transcript = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .map(|entry| {
                entry
                    .history
                    .messages
                    .iter()
                    .filter(|m| m.role == "assistant")
                    .map(|m| m.content.clone())
                    .collect::<Vec<String>>()
                    .join("\n")
            })
            .unwrap_or_default()
    });
    let uncovered = uncovered_steps(&transcript, step_count);
    if uncovered.is_empty() {
        return false;
    }
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
            entry.history.messages.push(nudge);
            entry.persistence.last_accessed = Instant::now();
        }
    });
    log::warn!(
        "Ghost Shell {}: steps {} uncovered, nudging instead of completing",
        session_id,
        names.join(", ")
    );
    true
}
