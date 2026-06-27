use super::super::{ToolCallOutcome, USER_PROMPT_TIMEOUT};
use super::{ArtifactCtx, track_artifact};
use crate::daemon::utils::log_event;
use crate::daemon::utils::send_response_split;
use crate::ipc::{Request, Response, RunbookListItem, ScriptListItem};
use crate::scheduler::ScheduleStore;
use crate::scripts;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------

pub async fn write_script<W, R>(
    id: &str,
    script_name: &str,
    content: &str,
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
            "Error: cannot write scripts in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    let existing_content = scripts::read_script(script_name).ok();
    send_response_split(
        tx,
        Response::ScriptWritePrompt {
            id: id.to_string(),
            script_name: script_name.to_string(),
            content: content.to_string(),
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
        crate::daemon::stats::inc_scripts_approved();
        let stamped = match artifact_ctx.saved_name {
            Some(origin) => crate::header::inject_comment_session_origin(content, origin),
            None => content.to_string(),
        };
        match scripts::write_script(script_name, &stamped) {
            Ok(()) => {
                track_artifact(artifact_ctx, "script", script_name);
                Ok(ToolCallOutcome::Result(format!(
                    "Script '{}' written successfully",
                    script_name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to write script: {}",
                e
            ))),
        }
    } else {
        crate::daemon::stats::inc_scripts_denied();
        Ok(ToolCallOutcome::Result(
            "Script write denied by user".to_string(),
        ))
    }
}

pub async fn list_scripts<W>(tx: &mut W) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let script_list = scripts::list_scripts().unwrap_or_default();
    let items: Vec<ScriptListItem> = script_list
        .iter()
        .map(|s| ScriptListItem {
            name: s.name.clone(),
            size: s.size,
        })
        .collect();
    let count = items.len();
    let _ = send_response_split(tx, Response::ScriptList { scripts: items }).await;
    Ok(ToolCallOutcome::Result(format!(
        "{} script(s) in ~/.daemoneye/scripts/",
        count
    )))
}

pub fn read_script(script_name: &str) -> String {
    match scripts::read_script(script_name) {
        Ok(content) => content,
        Err(e) => format!("Error reading script '{}': {}", script_name, e),
    }
}

pub async fn delete_script<W, R>(
    id: &str,
    script_name: &str,
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
            "Error: cannot delete scripts in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    send_response_split(
        tx,
        Response::ScriptDeletePrompt {
            id: id.to_string(),
            script_name: script_name.to_string(),
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
        match scripts::delete_script(script_name) {
            Ok(()) => {
                log::info!("Script '{}' deleted", script_name);
                log_event(
                    "script_delete",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "script": script_name }),
                );
                Ok(ToolCallOutcome::Result(format!(
                    "Script '{}' deleted",
                    script_name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to delete script '{}': {}",
                script_name, e
            ))),
        }
    } else {
        Ok(ToolCallOutcome::Result(
            "Script deletion denied by user".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Runbooks
// ---------------------------------------------------------------------------

pub async fn write_runbook<W, R>(
    id: &str,
    name: &str,
    content: &str,
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
            "Error: cannot write runbooks in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    let existing_content = crate::runbook::load_runbook(name).ok().map(|rb| rb.content);
    send_response_split(
        tx,
        Response::RunbookWritePrompt {
            id: id.to_string(),
            runbook_name: name.to_string(),
            content: content.to_string(),
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
            Ok(Request::RunbookWriteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        crate::daemon::stats::inc_runbooks_approved();
        let stamped = match artifact_ctx.saved_name {
            Some(origin) => crate::header::inject_yaml_session_origin(content, origin),
            None => content.to_string(),
        };
        match crate::runbook::write_runbook(name, &stamped) {
            Ok(()) => {
                log::info!("Runbook '{}' written", name);
                log_event(
                    "runbook_write",
                    serde_json::json!({ "session": artifact_ctx.session_id.unwrap_or("-"), "runbook": name }),
                );
                track_artifact(artifact_ctx, "runbook", name);
                Ok(ToolCallOutcome::Result(format!(
                    "Runbook '{}' written to ~/.daemoneye/runbooks/{}.md",
                    name, name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to write runbook: {}",
                e
            ))),
        }
    } else {
        crate::daemon::stats::inc_runbooks_denied();
        Ok(ToolCallOutcome::Result(
            "Runbook write denied by user".to_string(),
        ))
    }
}

pub async fn delete_runbook<W, R>(
    id: &str,
    name: &str,
    is_ghost: bool,
    session_id: Option<&str>,
    schedule_store: &Arc<ScheduleStore>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let active_jobs: Vec<String> = schedule_store
        .list()
        .into_iter()
        .filter(|j| j.runbook.as_deref() == Some(name))
        .map(|j| j.name)
        .collect();

    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot delete runbooks in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    send_response_split(
        tx,
        Response::RunbookDeletePrompt {
            id: id.to_string(),
            runbook_name: name.to_string(),
            active_jobs,
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
            Ok(Request::RunbookDeleteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        match crate::runbook::delete_runbook(name) {
            Ok(()) => {
                log::info!("Runbook '{}' deleted", name);
                log_event(
                    "runbook_delete",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "runbook": name }),
                );
                Ok(ToolCallOutcome::Result(format!(
                    "Runbook '{}' deleted",
                    name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to delete runbook: {}",
                e
            ))),
        }
    } else {
        Ok(ToolCallOutcome::Result(
            "Runbook delete denied by user".to_string(),
        ))
    }
}

pub fn read_runbook(name: &str) -> String {
    match crate::runbook::load_runbook(name) {
        Ok(rb) => rb.content,
        Err(e) => format!("Error reading runbook '{}': {}", name, e),
    }
}

pub async fn list_runbooks<W>(tx: &mut W) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let items = crate::runbook::list_runbooks().unwrap_or_default();
    let count = items.len();
    let runbook_items: Vec<RunbookListItem> = items
        .iter()
        .map(|r| RunbookListItem {
            name: r.name.clone(),
            tags: r.tags.clone(),
            ghost_config: r.ghost_config.clone(),
        })
        .collect();
    let _ = send_response_split(
        tx,
        Response::RunbookList {
            runbooks: runbook_items,
        },
    )
    .await;
    Ok(ToolCallOutcome::Result(format!(
        "{} runbook(s) in ~/.daemoneye/runbooks/",
        count
    )))
}
