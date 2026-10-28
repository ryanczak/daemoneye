//! `execute_tool_call`'s arms for the memory and recall tools: `AddMemory`, `UpdateMemory`, `DeleteMemory`, `ReadMemory`, `ListMemories`, `SearchRepository`, `RecallContext`, `AwaitAgentResult`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W>(
    call: &PendingCall,
    tx: &mut W,
    session_id: Option<&str>,
    artifact_ctx: &knowledge::ArtifactCtx<'_>,
    memory_namespaces: &[&str],
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match call {
        PendingCall::AddMemory {
            key,
            value,
            category,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::add_memory(
            key,
            value,
            category,
            artifact_ctx,
        ))),

        PendingCall::UpdateMemory {
            key,
            category,
            body,
            append,
            tags,
            summary,
            relates_to,
            expires,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::update_memory(
            knowledge::UpdateMemoryRequest {
                key,
                category,
                body: body.as_deref(),
                append: *append,
                tags: tags.as_deref(),
                summary: summary.as_deref(),
                relates_to: relates_to.as_deref(),
                expires: expires.as_deref(),
            },
            session_id,
            memory_namespaces,
        ))),

        PendingCall::DeleteMemory { key, category, .. } => Ok(ToolCallOutcome::Result(
            knowledge::delete_memory(key, category, session_id, memory_namespaces),
        )),

        PendingCall::ReadMemory { key, category, .. } => Ok(ToolCallOutcome::Result(
            knowledge::read_memory(key, category, memory_namespaces),
        )),

        PendingCall::ListMemories { category, .. } => {
            knowledge::list_memories(category.as_deref(), memory_namespaces, tx).await
        }

        PendingCall::SearchRepository { query, kind, .. } => Ok(ToolCallOutcome::Result(
            knowledge::search_repository(query, kind, artifact_ctx.namespaces),
        )),

        PendingCall::RecallContext {
            query,
            turn_start,
            turn_end,
            scope,
            ..
        } => {
            let result = if let Some(sid) = session_id {
                let args = crate::daemon::context::recall::RecallArgs {
                    query: query.clone(),
                    turn_start: *turn_start,
                    turn_end: *turn_end,
                    scope: scope.clone(),
                };
                let limits = crate::config::LimitsConfig::default();
                match crate::daemon::context::recall::recall(sid, &args, &limits) {
                    Ok(r) => r,
                    Err(e) => format!("Error: {}", e),
                }
            } else {
                "Error: recall_context requires a session".to_string()
            };
            Ok(ToolCallOutcome::Result(result))
        }

        PendingCall::AwaitAgentResult {
            job_id,
            agent_name,
            timeout_secs,
            ..
        } => {
            crate::daemon::utils::with_keepalive(
                tx,
                knowledge::await_agent_result(job_id, agent_name, *timeout_secs, memory_namespaces),
            )
            .await?
        }

        other => anyhow::bail!(
            "{} is not one of the memory and recall tools",
            other.tool_name()
        ),
    }
}
