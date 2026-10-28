//! Three of `run_conversation_loop`'s terminal replies: the turn timeout,
//! the backend error and the stream that kept ending early. Each sends its
//! frame and finishes the run log. A child module of `src/daemon/stream.rs`.

use super::*;

/// The turn ran past `[limits] turn_timeout_secs`.
pub(super) async fn end_turn_timeout<W>(
    tx: &mut W,
    config: &Config,
    run_log: &mut crate::daemon::run::RunLogGuard,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    send_response_split(
        tx,
        Response::Error(format!(
            "Turn exceeded [limits] turn_timeout_secs ({}s) — aborting.",
            config.limits.turn_timeout_secs
        )),
    )
    .await?;
    run_log.finish(crate::daemon::run::RunState::BudgetExceeded(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!(
                "the turn exceeded [limits] turn_timeout_secs ({}s) and was aborted",
                config.limits.turn_timeout_secs
            )),
            None,
        ),
    ));
    Ok(())
}

/// The AI backend reported an error.
pub(super) async fn end_backend_error<W>(
    tx: &mut W,
    run_log: &mut crate::daemon::run::RunLogGuard,
    e: String,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    run_log.metrics_mut().record_backend_error();
    send_response_split(tx, Response::Error(e.clone())).await?;
    run_log.finish(crate::daemon::run::RunState::HardFail(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!("the AI backend reported an error: {e}")),
            None,
        ),
    ));
    Ok(())
}

/// The AI event channel closed without `Done` or `Error` more than
/// `MAX_CHANNEL_CLOSED_RETRIES` times.
pub(super) async fn end_stream_closed<W>(
    tx: &mut W,
    run_log: &mut crate::daemon::run::RunLogGuard,
    channel_closed_retries: u32,
    cause: String,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    run_log.metrics_mut().record_backend_error();
    log::error!(
        "AI event channel closed without Done/Error, giving up after \
         {MAX_CHANNEL_CLOSED_RETRIES} retries: {cause}"
    );
    send_response_split(
        tx,
        Response::Error(format!(
            "AI backend ended the stream without completing \
             (after {} attempts): {cause}",
            channel_closed_retries
        )),
    )
    .await?;
    run_log.finish(crate::daemon::run::RunState::HardFail(
        crate::daemon::run::Briefing::new(
            "chat turn",
            None,
            Vec::new(),
            Vec::new(),
            Some(format!(
                "the AI backend ended the stream without completing \
                 (after {channel_closed_retries} attempts): {cause}"
            )),
            None,
        ),
    ));
    Ok(())
}
