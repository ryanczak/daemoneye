//! `do_ghost_turn`, the ghost run's turn loop: the model call, the tool
//! calls, the wrap-up turn and the completion bookkeeping. A child module
//! of `src/daemon/ghost.rs`.

use super::*;

#[path = "turn_helpers.rs"]
mod helpers;
use helpers::*;

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
    let Some((ghost_config, ghost_active_model)) = with_sessions(sessions, |store| {
        let entry = store.get(session_id)?;
        let g = entry.as_ghost()?;
        Some((g.config.clone(), entry.active_model.clone()))
    }) else {
        anyhow::bail!("Ghost Shell '{}' not found", session_id);
    };

    let prompt::GhostPrompt {
        system,
        max_ghost_turns,
        steps,
    } = prompt::build_ghost_prompt(session_id, sessions, config, "");

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

    let mut turn = 0usize;
    let mut wrap_up_turn = false;
    loop {
        if !prepare_ghost_turn_step(
            session_id,
            sessions,
            config,
            run_id,
            cancel,
            max_ghost_turns,
            &mut turn,
            &mut wrap_up_turn,
            outcome,
            metrics,
        )
        .await
        {
            break;
        }

        let Some((chat_messages, loaded_tools)) = load_and_compact_ghost_messages(
            session_id,
            sessions,
            model_entry.context_window(),
            config,
        ) else {
            break;
        };

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

        let (assistant_content, pending_calls) = collect_ghost_stream_turn(
            session_id,
            turn,
            &mut ai_rx,
            sessions,
            &ghost_config,
            model_entry,
            run_id,
            metrics,
        )
        .await?;

        log_ghost_tool_batch(session_id, turn, &pending_calls);
        let tool_results = execute_ghost_tool_batch(
            session_id,
            turn,
            &pending_calls,
            sessions,
            config,
            cache,
            schedule_store,
            metrics,
        )
        .await?;

        persist_ghost_turn_message(
            session_id,
            sessions,
            assistant_content,
            &pending_calls,
            tool_results,
            turn,
        );

        if pending_calls.is_empty() {
            if maybe_nudge_uncovered_steps(session_id, sessions, steps.len(), turn) {
                continue;
            }
            break;
        }
    }

    finish_ghost_turn_loop(session_id, turn, &ghost_config, sessions, config).await;
    Ok(())
}
