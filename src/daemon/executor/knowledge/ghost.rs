use super::super::ToolCallOutcome;
use crate::daemon::session::SessionStore;

// ---------------------------------------------------------------------------
// Spawn ghost shell
// ---------------------------------------------------------------------------

pub async fn spawn_ghost(
    runbook: &str,
    message: &str,
    agent_name: Option<&str>,
    sessions: &SessionStore,
    spawn_depth: u8,
    parent_job_id: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::ghost::{GhostManager, check_ghost_capacity};
    use crate::webhook::inject_ghost_event;

    let spawn_config = crate::config::Config::load().unwrap_or_default();
    if !check_ghost_capacity(&spawn_config) {
        return Ok(ToolCallOutcome::Result(format!(
            "Cannot spawn ghost shell: concurrency limit ({}) reached. \
             Wait for an active ghost to complete before spawning another.",
            spawn_config.ghost.max_concurrent_ghosts
        )));
    }

    let rb = match crate::runbook::load_runbook(runbook) {
        Ok(rb) => rb,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Failed to load runbook '{}': {}",
                runbook, e
            )));
        }
    };

    // Build merged ghost config: start from runbook defaults, then apply the
    // agent specified by the AI tool (overrides runbook's own agent: field).
    let mut ghost_config = if let Some(name) = agent_name {
        let mut base = rb.ghost_config.clone();
        base.agent = Some(name.to_string());
        crate::agents::merge_runbook_ghost_config_from(&rb, base)
    } else {
        crate::agents::merge_runbook_ghost_config(&rb)
    };

    ghost_config.spawn_depth = spawn_depth + 1;
    ghost_config.parent_job_id = parent_job_id.map(|s| s.to_string());

    let rb_name = rb.name.clone();
    match GhostManager::start_session_with_config(
        sessions.clone(),
        &rb,
        &ghost_config,
        message,
        crate::daemon::GS_BG_WINDOW_PREFIX,
        spawn_config.approvals.ghost_commands,
    )
    .await
    {
        Err(e) => Ok(ToolCallOutcome::Result(format!(
            "Failed to start ghost shell: {}",
            e
        ))),
        Ok(sid) => {
            let job_id = sid.clone();
            let task_message = message.to_string();
            if let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(&sid)
            {
                entry.ghost_task_message = Some(task_message);
            }
            inject_ghost_event(
                sessions,
                &format!(
                    "[Ghost Shell Started] AI-requested ghost shell started for runbook: {} (job_id: {})",
                    rb_name, job_id
                ),
            );
            let tool_result = format!(
                "Ghost shell started (session: {}, job_id: {}, agent: {}). It will run autonomously in the background \
                 and inject [Ghost Shell Completed] or [Ghost Shell Failed] events when done. \
                 Use the job_id and agent name with await_agent_result(job_id, agent_name) to wait for the result.",
                sid,
                job_id,
                agent_name.unwrap_or("(default)")
            );
            Ok(ToolCallOutcome::SpawnGhostSession {
                session_id: sid,
                runbook_name: rb_name,
                tool_result,
                job_id,
            })
        }
    }
}
