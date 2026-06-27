use super::super::{ToolCallOutcome, USER_PROMPT_TIMEOUT};
use super::{ArtifactCtx, track_artifact};
use crate::daemon::utils::log_event;
use crate::daemon::utils::send_response_split;
use crate::ipc::{Request, Response};

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

// TODO(M2): consolidate params into a struct
#[allow(clippy::too_many_arguments)]
pub async fn create_agent<W, R>(
    id: &str,
    name: &str,
    description: &str,
    prompt: &str,
    model: Option<&str>,
    memory_namespace: &str,
    max_turns: Option<u32>,
    auto_approve_read_only: bool,
    auto_approve_scripts: &[String],
    artifact_ctx: &ArtifactCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    if artifact_ctx.is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot create agents in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    let existing = crate::agents::load_agent(name).ok();
    let existing_content = existing
        .as_ref()
        .map(|c| toml::to_string_pretty(c).unwrap_or_default());

    let config_str = toml::to_string_pretty(&crate::agents::AgentConfig {
        name: name.to_string(),
        description: description.to_string(),
        prompt: prompt.to_string(),
        model: model.map(|s| s.to_string()),
        memory_namespace: memory_namespace.to_string(),
        max_turns,
        auto_approve_read_only,
        auto_approve_scripts: auto_approve_scripts.to_vec(),
        read_namespaces: Vec::new(),
        tools: None,
    })
    .unwrap_or_default();

    send_response_split(
        tx,
        Response::ScriptWritePrompt {
            id: id.to_string(),
            script_name: format!("agents/{}", name),
            content: config_str.clone(),
            existing_content,
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ScriptWriteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        let config = crate::agents::AgentConfig {
            name: name.to_string(),
            description: description.to_string(),
            prompt: prompt.to_string(),
            model: model.map(|s| s.to_string()),
            memory_namespace: memory_namespace.to_string(),
            max_turns,
            auto_approve_read_only,
            auto_approve_scripts: auto_approve_scripts.to_vec(),
            read_namespaces: Vec::new(),
            tools: None,
        };
        match crate::agents::save_agent(&config) {
            Ok(()) => {
                track_artifact(artifact_ctx, "agent", name);
                Ok(ToolCallOutcome::Result(format!(
                    "Agent '{}' created at ~/.daemoneye/agents/{}/config.toml",
                    name, name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to create agent '{}': {}",
                name, e
            ))),
        }
    } else {
        Ok(ToolCallOutcome::Result(
            "Agent creation denied by user".to_string(),
        ))
    }
}

pub fn read_agent(name: &str) -> String {
    match crate::agents::load_agent(name) {
        Ok(cfg) => {
            let mut out = format!("Agent: {}\n", cfg.name);
            out.push_str(&format!("  description: {}\n", cfg.description));
            out.push_str(&format!(
                "  model: {}\n",
                cfg.model.as_deref().unwrap_or("(default)")
            ));
            out.push_str(&format!("  memory_namespace: {}\n", cfg.memory_namespace));
            if let Some(t) = cfg.max_turns {
                out.push_str(&format!("  max_turns: {}\n", t));
            }
            out.push_str(&format!(
                "  auto_approve_read_only: {}\n",
                cfg.auto_approve_read_only
            ));
            if !cfg.auto_approve_scripts.is_empty() {
                out.push_str("  auto_approve_scripts:\n");
                for s in &cfg.auto_approve_scripts {
                    out.push_str(&format!("    - {}\n", s));
                }
            }
            if !cfg.prompt.is_empty() {
                out.push_str(&format!("\n  prompt:\n{}\n", cfg.prompt));
            }
            if let Some(briefing) = crate::daemon::briefing::read_briefing(name) {
                out.push_str("\n## Briefing (from previous run)\n");
                out.push_str(&briefing);
                out.push('\n');
            }
            out
        }
        Err(e) => format!("Error reading agent '{}': {}", name, e),
    }
}

pub async fn list_agents_tool<W>(_tx: &mut W) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let agents = crate::agents::list_agents()?;
    let count = agents.len();
    if count == 0 {
        Ok(ToolCallOutcome::Result(
            "No agents defined. Use create_agent to create one.".to_string(),
        ))
    } else {
        let mut out = format!("{} agent(s):\n", count);
        for a in &agents {
            let model = a.model.as_deref().unwrap_or("(default)");
            out.push_str(&format!(
                "  {} (model: {}) — {}\n",
                a.name, model, a.description
            ));
        }
        Ok(ToolCallOutcome::Result(out))
    }
}

pub async fn delete_agent<W, R>(
    id: &str,
    name: &str,
    is_ghost: bool,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot delete agents in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    send_response_split(
        tx,
        Response::ScriptDeletePrompt {
            id: id.to_string(),
            script_name: format!("agent: {}", name),
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ScriptDeleteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        match crate::agents::delete_agent(name) {
            Ok(()) => {
                log::info!("Agent '{}' deleted", name);
                log_event(
                    "agent_delete",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "agent": name }),
                );
                Ok(ToolCallOutcome::Result(format!("Agent '{}' deleted", name)))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to delete agent '{}': {}",
                name, e
            ))),
        }
    } else {
        Ok(ToolCallOutcome::Result(
            "Agent deletion denied by user".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Await agent result (G5 — Agent-to-Agent Delegation)
// ---------------------------------------------------------------------------

pub async fn await_agent_result(
    job_id: &str,
    agent_name: &str,
    timeout_secs: u64,
    _namespaces: &[&str],
) -> anyhow::Result<ToolCallOutcome> {
    const MAX_TIMEOUT_SECS: u64 = 3600;
    let capped_secs = timeout_secs.min(MAX_TIMEOUT_SECS);
    let deadline = tokio::time::Duration::from_secs(capped_secs);
    let poll_interval = tokio::time::Duration::from_secs(2);

    let result = tokio::time::timeout(deadline, async {
        loop {
            match crate::agents::mailbox::read_mailbox(agent_name, job_id) {
                Ok(Some(entry)) => {
                    if entry.status != crate::agents::mailbox::MailboxStatus::Pending {
                        return Ok(entry);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(format!("Error reading mailbox: {}", e));
                }
            }
            tokio::time::sleep(poll_interval).await;
        }
    })
    .await;

    match result {
        Ok(Ok(entry)) => Ok(ToolCallOutcome::Result(
            serde_json::to_string_pretty(&entry).unwrap_or_else(|_| {
                format!("job_id: {}, status: {:?}", entry.job_id, entry.status)
            }),
        )),
        Ok(Err(e)) => Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
        Err(_) => Ok(ToolCallOutcome::Result(format!(
            "Error: await_agent_result timed out after {} seconds for job_id: {}",
            capped_secs, job_id
        ))),
    }
}
