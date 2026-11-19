//! `run_conversation_loop`, one chat turn end to end: the model call, the
//! event stream, the tool calls and their replies, the persistence. A child
//! module of `src/daemon/stream.rs`, which re-exports the function.

use super::*;

#[path = "conversation_helpers.rs"]
mod helpers;
use helpers::*;

pub async fn run_conversation_loop<W, R>(
    mut ctx: ConversationLoopCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let mut run_log = crate::daemon::run::RunLogGuard::start(
        crate::daemon::run::RunId::new(),
        crate::daemon::run::RunKind::Chat {
            session: ctx.session_id.clone().unwrap_or_default(),
        },
    );
    let (_cancel_guard, mut cancel_signal) = crate::daemon::run::register_run(run_log.run_id());
    let _active_guard = ctx.session_id.as_deref().map(|sid| {
        tokio::spawn(crate::daemon::run::watch_stop_sentinel(
            run_log.run_id().clone(),
            crate::daemon::run::STOP_POLL_INTERVAL,
        ));
        crate::daemon::run::register_active(sid, run_log.run_id())
    });

    let turn_deadline = (ctx.config.limits.turn_timeout_secs > 0).then(|| {
        tokio::time::Instant::now()
            + std::time::Duration::from_secs(ctx.config.limits.turn_timeout_secs)
    });

    let mut state = TurnLoopState::default();
    let mut channel_closed_retries: u32 = 0;

    loop {
        if check_chat_cost_gate(&ctx, tx, &mut run_log).await? {
            return Ok(());
        }
        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
        let mut chat_task = spawn_chat_model_task(&ctx, ai_tx, &mut run_log);

        let mut full_response = String::new();
        let mut pending_calls: Vec<PendingCall> = Vec::new();

        loop {
            if let Some(deadline) = turn_deadline
                && tokio::time::Instant::now() >= deadline
            {
                return endings::end_turn_timeout(tx, ctx.config, &mut run_log).await;
            }
            let event = tokio::select! {
                biased;
                reason = cancel_signal.cancelled() => {
                    return handle_turn_cancelled(&mut ctx, tx, &mut run_log, reason, &full_response).await;
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
                            return endings::end_stream_closed(tx, &mut run_log, channel_closed_retries, cause).await;
                        }
                        log::warn!(
                            "AI event channel closed without Done/Error (attempt \
                             {channel_closed_retries}/{MAX_CHANNEL_CLOSED_RETRIES}), \
                             re-issuing the AI call: {cause}"
                        );
                        break;
                    }
                    Err(_timeout) => {
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
                    pending_calls.push(collect_terminal_tool_call(
                        id,
                        cmd,
                        wait,
                        interactive,
                        thought_signature,
                        shell,
                        host,
                        timeout_secs,
                    ));
                }
                AiEvent::Error(e) => {
                    return endings::end_backend_error(tx, &mut run_log, e).await;
                }
                AiEvent::Notice(n) => {
                    send_response_split(tx, Response::SystemMsg(format!("⚠ {n}"))).await?;
                }
                AiEvent::Done(usage) => {
                    match handle_chat_done_event(
                        &mut ctx,
                        tx,
                        rx,
                        usage,
                        &full_response,
                        &pending_calls,
                        &mut state,
                        &mut run_log,
                    )
                    .await
                    {
                        ToolBatchControl::EndTurn(res) => return res,
                        ToolBatchControl::ContinueNextModelTurn => break,
                    }
                }
                tool => pending_calls.extend(PendingCall::from_tool_event(tool)),
            }
        }
    }
}
