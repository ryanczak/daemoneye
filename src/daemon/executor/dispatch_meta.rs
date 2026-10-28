//! `execute_tool_call`'s arms for the tool-set and delegation tools: `InvalidArgs`, `LoadTools`, `SpawnGhost`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W>(
    call: &PendingCall,
    tx: &mut W,
    session_id: Option<&str>,
    sessions: &SessionStore,
    ghost_policy: &Option<GhostPolicy>,
    spawn_depth: u8,
    effective_parent_job_id: &Option<String>,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match call {
        PendingCall::InvalidArgs {
            id, name, error, ..
        } => {
            let msg = format!(
                "Error: invalid arguments for {name}: {error}. Fix the arguments and call {name} again."
            );
            log::warn!("model sent invalid arguments for tool '{name}': {error}");
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: msg.clone(),
                },
            )
            .await?;
            Ok(ToolCallOutcome::Result(msg))
        }
        PendingCall::LoadTools { groups, .. } => {
            let mut loaded: Vec<String> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if members.is_empty() {
                    unknown.push(g.clone());
                } else {
                    for m in members {
                        loaded.push(m.to_string());
                    }
                }
            }
            // Persist into session state
            if let Some(sid) = session_id {
                with_sessions(sessions, |store| {
                    if let Some(entry) = store.get_mut(sid) {
                        for name in &loaded {
                            entry.loaded_tools.insert(name.clone());
                        }
                        entry.dirty = true;
                    }
                });
            }
            let mut result_parts = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if !members.is_empty() {
                    result_parts.push(format!("Loaded group(s): {} → {}", g, members.join(", ")));
                }
            }
            if !unknown.is_empty() {
                result_parts.push(format!(
                    "Unknown group(s): {} (valid groups: {})",
                    unknown.join(", "),
                    crate::ai::tools::deferred_catalog_text()
                ));
            }
            result_parts.push("They are now available; call them directly.".to_string());
            Ok(ToolCallOutcome::Result(result_parts.join(". ")))
        }

        PendingCall::SpawnGhost {
            runbook,
            message,
            agent,
            host,
            ..
        } => 'arm: {
            if spawn_depth >= 2 {
                break 'arm Ok(ToolCallOutcome::Result(
                    "Delegation depth limit reached (max: coordinator + 1 level of specialists)."
                        .to_string(),
                ));
            }
            knowledge::spawn_ghost(
                runbook,
                message,
                agent.as_deref(),
                host.as_deref(),
                ghost_policy.as_ref(),
                sessions,
                spawn_depth,
                effective_parent_job_id.as_deref(),
            )
            .await
        }

        other => anyhow::bail!(
            "{} is not one of the tool-set and delegation tools",
            other.tool_name()
        ),
    }
}
