//! `execute_tool_call`'s arms for the script, runbook and agent tools: `WriteScript`, `ListScripts`, `ReadScript`, `DeleteScript`, `WriteRunbook`, `DeleteRunbook`, `ReadRunbook`, `ListRunbooks`, `CreateAgent`, `ReadAgent`, `ListAgents`, `DeleteAgent`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    session_id: Option<&str>,
    is_ghost: bool,
    schedule_store: &Arc<ScheduleStore>,
    artifact_ctx: &knowledge::ArtifactCtx<'_>,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    match call {
        PendingCall::WriteScript {
            id,
            script_name,
            content,
            ..
        } => knowledge::write_script(id, script_name, content, artifact_ctx, tx, rx).await,

        PendingCall::ListScripts { .. } => knowledge::list_scripts(tx).await,

        PendingCall::ReadScript { script_name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_script(script_name)))
        }

        PendingCall::DeleteScript {
            id, script_name, ..
        } => knowledge::delete_script(id, script_name, is_ghost, session_id, tx, rx).await,

        PendingCall::WriteRunbook {
            id, name, content, ..
        } => knowledge::write_runbook(id, name, content, artifact_ctx, tx, rx).await,

        PendingCall::DeleteRunbook { id, name, .. } => {
            knowledge::delete_runbook(id, name, is_ghost, session_id, schedule_store, tx, rx).await
        }

        PendingCall::ReadRunbook { name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_runbook(name)))
        }

        PendingCall::ListRunbooks { .. } => knowledge::list_runbooks(tx).await,

        PendingCall::CreateAgent {
            id,
            name,
            description,
            prompt,
            model,
            memory_namespace,
            max_turns,
            auto_approve_read_only,
            auto_approve_scripts,
            ..
        } => {
            knowledge::create_agent(
                knowledge::CreateAgentArgs {
                    id,
                    name,
                    description,
                    prompt,
                    model: model.as_deref(),
                    memory_namespace,
                    max_turns: *max_turns,
                    auto_approve_read_only: *auto_approve_read_only,
                    auto_approve_scripts,
                },
                artifact_ctx,
                tx,
                rx,
            )
            .await
        }

        PendingCall::ReadAgent { name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_agent(name)))
        }

        PendingCall::ListAgents { .. } => knowledge::list_agents_tool(tx).await,

        PendingCall::DeleteAgent { id, name, .. } => {
            knowledge::delete_agent(id, name, is_ghost, session_id, tx, rx).await
        }

        other => anyhow::bail!(
            "{} is not one of the script, runbook and agent tools",
            other.tool_name()
        ),
    }
}
