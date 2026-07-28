use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::{Config, PricingSource};
use crate::cost::{CostAttribution, CostRecord, compute_cost};
use crate::daemon::auto_name;
use crate::daemon::executor::{self, SessionCtx};
use crate::daemon::session::{
    SessionStore, append_session_message, with_sessions, write_session_file,
};
use crate::daemon::utils::{log_event, send_response_split};
use crate::ipc::Response;
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncWrite};

/// Approval-gated tool names. Tools in this list bypass per-turn budget caps
/// because the user's per-call approval prompt serves as the gate.
///
/// Keep in sync with `LimitsConfig::APPROVAL_GATED` in `config.rs`.
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

/// Run the AI conversation loop: stream tokens, collect tool calls, execute,
/// and repeat until the AI produces a final answer with no pending tool calls.
///
/// This is the core inner loop of the daemon. It handles:
/// - Event streaming from the LLM (with keep-alive on timeout)
/// - Tool call collection and budget enforcement
/// - Tool execution (foreground, background, ghost spawn)
/// - Response persistence (in-memory and on-disk)
/// - Auto-name suggestion for unnamed sessions
pub struct ConversationLoopCtx<'a> {
    pub session_id: Option<String>,
    pub session_name: &'a str,
    pub chat_pane: Option<String>,
    pub messages: Vec<Message>,
    pub sys_prompt: String,
    pub session_active_model: Option<String>,
    pub is_ghost_session: bool,
    pub this_turn_count: usize,
    pub post_trim_len: usize,
    pub needs_compaction: bool,
    pub wants_background_compaction: bool,
    pub config: &'a Config,
    pub cache: Arc<SessionCache>,
    pub sessions: SessionStore,
    pub schedule_store: Arc<ScheduleStore>,
    pub cost_attribution: CostAttribution,
}

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

        tokio::spawn(async move {
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
                    send_response_split(tx, Response::KeepAlive).await?;
                    continue;
                }
            };
            match event {
                AiEvent::Token(t) => {
                    full_response.push_str(&t);
                    send_response_split(tx, Response::Token(t)).await?;
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
                AiEvent::LoadTools {
                    id,
                    groups,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::LoadTools {
                        id,
                        groups,
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
                AiEvent::RecallContext {
                    id,
                    query,
                    turn_start,
                    turn_end,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::RecallContext {
                        id,
                        thought_signature,
                        query,
                        turn_start,
                        turn_end,
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
                    agent,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::SpawnGhost {
                        id,
                        thought_signature,
                        runbook,
                        message,
                        agent,
                    });
                }
                AiEvent::CreateAgent {
                    id,
                    name,
                    description,
                    prompt,
                    model,
                    memory_namespace,
                    max_turns,
                    auto_approve_read_only,
                    auto_approve_scripts,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::CreateAgent {
                        id,
                        thought_signature,
                        name,
                        description,
                        prompt,
                        model,
                        memory_namespace,
                        max_turns,
                        auto_approve_read_only,
                        auto_approve_scripts,
                    });
                }
                AiEvent::ReadAgent {
                    id,
                    name,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ReadAgent {
                        id,
                        thought_signature,
                        name,
                    });
                }
                AiEvent::ListAgents {
                    id,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::ListAgents {
                        id,
                        thought_signature,
                    });
                }
                AiEvent::DeleteAgent {
                    id,
                    name,
                    thought_signature,
                } => {
                    pending_calls.push(PendingCall::DeleteAgent {
                        id,
                        thought_signature,
                        name,
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
                AiEvent::Error(e) => {
                    send_response_split(tx, Response::Error(e)).await?;
                    return Ok(());
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
                        if !full_response.is_empty() {
                            messages.push(Message {
                                role: "assistant".to_string(),
                                content: full_response.clone(),
                                tool_calls: None,
                                tool_results: None,
                                turn: Some(this_turn_count),
                            });
                        }

                        let active_entry = config.resolve_model(session_active_model.as_deref());
                        let pricing = active_entry.pricing().unwrap_or(crate::config::Pricing {
                            input_per_mtok: 0.0,
                            output_per_mtok: 0.0,
                            cache_read_per_mtok: 0.0,
                            cache_write_per_mtok: 0.0,
                            source: PricingSource::Unknown,
                        });
                        let cost = compute_cost(&usage, &pricing);

                        let record = CostRecord {
                            timestamp: chrono::Utc::now(),
                            session_id: session_id.clone().unwrap_or_else(|| "-".to_string()),
                            agent_name: cost_attribution.agent_name.clone(),
                            is_ghost: cost_attribution.is_ghost,
                            parent_job_id: cost_attribution.parent_job_id.clone(),
                            provider: active_entry.provider.clone(),
                            model: active_entry.model.clone(),
                            tokens: usage.clone(),
                            cost,
                            pricing_source: pricing.source,
                        };
                        log_event(
                            "ai_cost",
                            // INVARIANT: CostRecord derives Serialize; serde_json::to_value never fails for it
                            serde_json::to_value(&record)
                                .expect("CostRecord serialization is infallible"),
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
                            if should_suggest
                                && let Some((name, desc)) =
                                    auto_name::suggest_session_name(&messages, config).await
                            {
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
                        return Ok(());
                    }

                    let active_entry = config.resolve_model(session_active_model.as_deref());
                    let pricing = active_entry.pricing().unwrap_or(crate::config::Pricing {
                        input_per_mtok: 0.0,
                        output_per_mtok: 0.0,
                        cache_read_per_mtok: 0.0,
                        cache_write_per_mtok: 0.0,
                        source: PricingSource::Unknown,
                    });
                    let cost = compute_cost(&usage, &pricing);

                    let record = CostRecord {
                        timestamp: chrono::Utc::now(),
                        session_id: session_id.clone().unwrap_or_else(|| "-".to_string()),
                        agent_name: cost_attribution.agent_name.clone(),
                        is_ghost: cost_attribution.is_ghost,
                        parent_job_id: cost_attribution.parent_job_id.clone(),
                        provider: active_entry.provider.clone(),
                        model: active_entry.model.clone(),
                        tokens: usage.clone(),
                        cost,
                        pricing_source: pricing.source,
                    };
                    log_event(
                        "ai_cost",
                        // INVARIANT: CostRecord derives Serialize; serde_json::to_value never fails for it
                        serde_json::to_value(&record)
                            .expect("CostRecord serialization is infallible"),
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

                    // Per-turn tool-call loop guard.
                    // Approval-gated tools are always exempt — the user's per-call
                    // approval prompt is the gate.
                    let mut tool_call_counts: HashMap<&str, u32> = HashMap::new();
                    let mut total_turn_call_count: u32 = 0;

                    let mut tool_results = Vec::new();
                    let mut user_message_redirect: Option<String> = None;
                    for call in pending_calls.iter() {
                        let call_id = call.id().to_string();
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
                                // Bump per-session counter for non-approval-gated tools.
                                if !APPROVAL_GATED.contains(&tool_name)
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
            }
        }
    }
}

/// Truncate tool results for history storage when they exceed the character cap.
fn truncate_tool_results(
    tool_results: Vec<ToolResult>,
    char_cap: Option<usize>,
) -> Vec<ToolResult> {
    tool_results
        .into_iter()
        .map(|r| match char_cap {
            Some(cap) if r.content.len() > cap => {
                let mut end = cap;
                while !r.content.is_char_boundary(end) {
                    end -= 1;
                }
                ToolResult {
                    tool_call_id: r.tool_call_id,
                    tool_name: r.tool_name,
                    content: format!(
                        "{}\n[truncated — {} chars total; full output archived in pane log]",
                        &r.content[..end],
                        r.content.len()
                    ),
                }
            }
            _ => r,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_gated_contains_expected_tools() {
        assert!(APPROVAL_GATED.contains(&"run_terminal_command"));
        assert!(APPROVAL_GATED.contains(&"edit_file"));
        assert!(APPROVAL_GATED.contains(&"write_script"));
        assert!(APPROVAL_GATED.contains(&"spawn_ghost_shell"));
        assert!(!APPROVAL_GATED.contains(&"read_file"));
        assert!(!APPROVAL_GATED.contains(&"get_terminal_context"));
    }

    #[test]
    fn truncate_tool_results_noop_when_under_cap() {
        let results = vec![ToolResult {
            tool_call_id: "1".to_string(),
            tool_name: "test".to_string(),
            content: "short".to_string(),
        }];
        let truncated = truncate_tool_results(results, Some(100));
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].content, "short");
    }

    #[test]
    fn truncate_tool_results_truncates_over_cap() {
        let results = vec![ToolResult {
            tool_call_id: "1".to_string(),
            tool_name: "test".to_string(),
            content: "a".repeat(200),
        }];
        let truncated = truncate_tool_results(results, Some(50));
        assert_eq!(truncated.len(), 1);
        assert!(truncated[0].content.contains("[truncated"));
        assert!(truncated[0].content.contains("200 chars total"));
    }

    #[test]
    fn truncate_tool_results_noop_when_no_cap() {
        let results = vec![ToolResult {
            tool_call_id: "1".to_string(),
            tool_name: "test".to_string(),
            content: "a".repeat(200),
        }];
        let truncated = truncate_tool_results(results, None);
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].content.len(), 200);
    }
}
