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
use tokio::io::{AsyncBufRead, AsyncWrite};

#[path = "ask_helpers.rs"]
mod helpers;
use helpers::*;

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
    let AskContext {
        cache,
        sessions,
        schedule_store,
        config,
    } = ctx;

    let session_name: String = req
        .client_tmux_session
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| config.daemon.tmux_session.clone());

    let AskSessionState {
        mut messages,
        session_cost_usd,
        has_untracked_cost,
        session_active_model,
        effective_prompt_tokens,
        session_token_scale,
    } = upsert_ask_session_and_load_state(
        req.session_id.as_ref(),
        &session_name,
        &req.chat_pane,
        &req.client_target_pane,
        sessions,
    );

    let compaction = apply_turn_compaction_ladder(
        &mut messages,
        req.session_id.as_ref(),
        sessions,
        config,
        session_active_model.as_deref(),
        effective_prompt_tokens,
        session_token_scale,
    )
    .await;

    let is_first_turn = messages.is_empty();

    let this_turn_count = req
        .session_id
        .as_ref()
        .map(|id| {
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(id) {
                    entry.history.turn_count += 1;
                    entry.history.turn_count
                } else {
                    1
                }
            })
        })
        .unwrap_or(1);

    let is_ghost_session = req
        .session_id
        .as_ref()
        .and_then(|id| with_sessions(sessions, |store| store.get(id).map(|e| e.is_ghost())))
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

    let history_count = messages.len();
    let AssembledAskPrompts {
        user_prompt,
        sys_prompt,
        is_ghost_session,
        cost_attribution,
    } = assemble_ask_prompts(
        &req,
        sessions,
        &cache,
        config,
        is_first_turn,
        is_ghost_session,
        effective_prompt_tokens,
        history_count,
        this_turn_count,
    );

    messages.push(Message {
        role: "user".to_string(),
        content: user_prompt,
        tool_calls: None,
        tool_results: None,
        turn: Some(this_turn_count),
    });

    send_pre_turn_session_notices(
        tx,
        req.session_id.as_ref(),
        sessions,
        history_count,
        this_turn_count,
        session_cost_usd,
        has_untracked_cost,
        &compaction,
    )
    .await?;

    let ctx = stream::ConversationLoopCtx {
        session_id: req.session_id,
        session_name: &session_name,
        chat_pane: req.chat_pane,
        messages,
        sys_prompt,
        session_active_model,
        is_ghost_session,
        this_turn_count,
        post_trim_len: compaction.post_trim_len,
        needs_compaction: compaction.needs_compaction,
        wants_background_compaction: compaction.wants_background_compaction,
        config,
        cache,
        sessions: sessions.clone(),
        schedule_store,
        cost_attribution,
    };
    stream::run_conversation_loop(ctx, tx, rx).await
}
