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
            let args = crate::daemon::context::recall::RecallArgs {
                query: query.clone(),
                turn_start: *turn_start,
                turn_end: *turn_end,
                scope: scope.clone(),
            };
            Ok(ToolCallOutcome::Result(recall_context(session_id, &args)))
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

/// `recall_context`'s result text: the session's archived turns through
/// `recall`, capped by the configured `[limits]`.
fn recall_context(
    session_id: Option<&str>,
    args: &crate::daemon::context::recall::RecallArgs,
) -> String {
    let Some(sid) = session_id else {
        return "Error: recall_context requires a session".to_string();
    };
    let limits = crate::config::Config::load().unwrap_or_default().limits;
    match crate::daemon::context::recall::recall(sid, args, &limits) {
        Ok(r) => r,
        Err(e) => format!("Error: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::recall_context;
    use crate::ai::Message;
    use crate::daemon::context::recall::RecallArgs;

    #[test]
    fn recall_context_uses_the_configured_tool_result_chars() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        std::fs::create_dir_all(crate::config::etc_dir()).unwrap();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[limits]\ntool_result_chars = 200\n",
        )
        .unwrap();

        let id = "recall_limits_test";
        let archive = crate::daemon::session::archive_file(id);
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        let msg = Message {
            role: "assistant".to_string(),
            content: "x".repeat(4000),
            tool_calls: None,
            tool_results: None,
            turn: Some(1),
        };
        std::fs::write(&archive, serde_json::to_string(&msg).unwrap() + "\n").unwrap();

        let args = RecallArgs {
            query: None,
            turn_start: Some(1),
            turn_end: Some(1),
            scope: None,
        };
        let out = recall_context(Some(id), &args);
        assert!(
            out.contains("[…truncated — narrow the turn range or refine the query…]"),
            "the configured cap of 200 must truncate a 4000-char turn: {} chars",
            out.chars().count()
        );
        assert!(
            out.chars().count() < 400,
            "output must respect the configured cap, got {} chars",
            out.chars().count()
        );
    }
}
