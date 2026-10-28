//! `execute_tool_call`'s arms for the shell opener: `OpenShell`.
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
        PendingCall::OpenShell {
            host, rows, cols, ..
        } => 'arm: {
            if let Some(policy) = ghost_policy.as_ref() {
                let asked = match host.as_deref() {
                    None => crate::host::HostId::Local,
                    Some(name) => match crate::host::HostId::parse(name) {
                        Ok(parsed) => parsed,
                        Err(msg) => {
                            break 'arm Ok(ToolCallOutcome::Result(format!("Error: {msg}")));
                        }
                    },
                };
                if !policy.may_use_host(&asked) {
                    let msg = ghost_host_refusal(policy, &asked);
                    crate::daemon::utils::log_event(
                        "ghost_host_refused",
                        serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "tool": "open_shell",
                            "requested_host": asked.as_str(),
                            "allowed_hosts": policy.hosts,
                        }),
                    );
                    break 'arm Ok(ToolCallOutcome::Result(msg));
                }
            }
            if let Err(msg) = connect_named_host(host.as_deref(), is_ghost, call.id(), tx, rx).await
            {
                crate::daemon::utils::log_event(
                    "ssh_connect_failed",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "open_shell",
                        "requested_host": host.as_deref().unwrap_or("local"),
                        "error": msg,
                    }),
                );
                break 'arm Ok(ToolCallOutcome::Result(format!("Error: {msg}")));
            }
            Ok(ToolCallOutcome::Result(
                knowledge::open_shell(
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                    host.as_deref(),
                    *rows,
                    *cols,
                )
                .await,
            ))
        }

        other => anyhow::bail!("{} is not one of the shell opener", other.tool_name()),
    }
}
