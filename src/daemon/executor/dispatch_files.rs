//! `execute_tool_call`'s arms for the file tools: `ReadFile`, `EditFile`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    session_id: Option<&str>,
    ghost_policy: &Option<GhostPolicy>,
    is_ghost: bool,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    match call {
        PendingCall::ReadFile {
            path,
            offset,
            limit,
            pattern,
            host,
            ..
        } => 'arm: {
            // A ghost reaching a host its runbook does not name is refused
            // before the run — the same boundary the foreground arm enforces.
            let host_id = match host.as_deref() {
                None => crate::host::HostId::Local,
                Some(name) => match crate::host::HostId::parse(name) {
                    Ok(parsed) => parsed,
                    Err(msg) => break 'arm Ok(ToolCallOutcome::Result(format!("Error: {msg}"))),
                },
            };
            if let Some(policy) = ghost_policy.as_ref()
                && !policy.may_use_host(&host_id)
            {
                let msg = ghost_host_refusal(policy, &host_id);
                crate::daemon::utils::log_event(
                    "ghost_host_refused",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "read_file",
                        "requested_host": host_id.as_str(),
                        "allowed_hosts": policy.hosts,
                    }),
                );
                break 'arm Ok(ToolCallOutcome::Result(msg));
            }
            if let Err(msg) = connect_named_host(host.as_deref(), is_ghost, call.id(), tx, rx).await
            {
                crate::daemon::utils::log_event(
                    "ssh_connect_failed",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "read_file",
                        "requested_host": host_id.as_str(),
                        "error": msg,
                    }),
                );
                let outcome = format!("Error: {msg}");
                send_response_split(
                    tx,
                    Response::ToolResult {
                        tool_call_id: call.id().to_string(),
                        output: outcome.clone(),
                    },
                )
                .await?;
                break 'arm Ok(ToolCallOutcome::Result(outcome));
            }
            file_ops::run_read_file(path, *offset, *limit, pattern.as_deref(), host.as_deref())
                .await
        }

        PendingCall::EditFile {
            id,
            path,
            operation,
            old_string,
            new_string,
            content,
            dest_path,
            host,
            sudo,
            ..
        } => {
            // The ghost refusal lives inside `run_edit_file`, so the connect
            // step must not run ahead of it: a ghost's `edit_file` would
            // otherwise open a BatchMode ControlMaster to any host, listed
            // or not, before being told file operations are refused (08d's
            // fence must not be tunnelled through a tool the ghost cannot
            // even use). Only a chat session connects first.
            if edit_file_connects_first(is_ghost, host.as_deref())
                && let Err(msg) =
                    connect_named_host(host.as_deref(), false, id.as_str(), tx, rx).await
            {
                let outcome = format!("Error: {msg}");
                send_response_split(
                    tx,
                    Response::ToolResult {
                        tool_call_id: id.to_string(),
                        output: outcome.clone(),
                    },
                )
                .await?;
                return Ok(ToolCallOutcome::Result(outcome));
            }
            file_ops::run_edit_file(
                file_ops::EditArgs {
                    id,
                    path,
                    operation,
                    old_string: old_string.as_deref(),
                    new_string: new_string.as_deref(),
                    content: content.as_deref(),
                    dest_path: dest_path.as_deref(),
                    host: host.as_deref(),
                    sudo: sudo.unwrap_or(false),
                },
                session_id,
                GhostCtx {
                    policy: ghost_policy.as_ref(),
                    is_ghost,
                },
                tx,
                rx,
            )
            .await
        }

        other => anyhow::bail!("{} is not one of the file tools", other.tool_name()),
    }
}
