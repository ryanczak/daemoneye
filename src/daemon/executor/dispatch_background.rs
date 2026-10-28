//! `execute_tool_call`'s `run_terminal_command` background arm
//! (`PendingCall::Background`, `wait: false`), in two stages: the chat
//! gate, the approval and the ghost route (`dispatch`), and the approved
//! start (`start_approved`). A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    ctx: SessionCtx<'_>,
    ghost_policy: &Option<GhostPolicy>,
    is_ghost: bool,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let SessionCtx {
        session_id,
        sessions,
        ..
    } = ctx;
    match call {
        PendingCall::Background { id, cmd, host, .. } => {
            // The PTY shell substrate, the only substrate, for a **chat**
            // session's plain background command. A ghost session (no
            // substrate shell of its own and sandboxed away from the host)
            // falls through to the refusal, which names the missing route.
            if !is_ghost && session_id.is_some() {
                match prompt_and_await_approval(
                    ApprovalRequest {
                        id,
                        cmd,
                        background: true,
                        target_pane_hint: None,
                        host: None,
                        user_shell: None,
                        interactive: false,
                    },
                    session_id,
                    ghost_policy.as_ref(),
                    tx,
                    rx,
                )
                .await?
                {
                    Err(outcome) => return Ok(outcome),
                    Ok(cmd_id) => {
                        return start_approved(id, cmd, cmd_id, session_id, sessions, tx).await;
                    }
                }
            }

            // The PTY shell substrate, the only substrate, for a **ghost**:
            // its remote-host, sandboxed and local PTY branches each live in
            // `foreground::run_background`, and a ghost that fits none of
            // them gets the refusal tail naming the missing route.
            foreground::run_background(
                id,
                cmd,
                host.as_deref(),
                ctx,
                GhostCtx {
                    policy: ghost_policy.as_ref(),
                    is_ghost,
                },
                tx,
                rx,
            )
            .await
        }
        other => anyhow::bail!(
            "{} is not run_terminal_command's background arm",
            other.tool_name()
        ),
    }
}

/// The approved start: the owner, the chat sandbox or the plain dedicated
/// shell, then the dispatch reply and the completion watcher, or the
/// start-error reply.
async fn start_approved<W>(
    id: &str,
    cmd: &str,
    cmd_id: usize,
    session_id: Option<&str>,
    sessions: &SessionStore,
    tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let owner = Owner::Chat(session_id.unwrap_or_default().to_string());
    let max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    let sandboxed = chat_sandbox::chat_sandbox_enabled(
        &crate::config::Config::load().unwrap_or_default().sandbox,
    );
    let started = if sandboxed {
        bg_shell::start_background_sandboxed(
            crate::shell::registry(),
            owner.clone(),
            session_id.unwrap_or_default(),
            cmd,
            max_per_owner,
        )
        .await
    } else {
        bg_shell::start_background(crate::shell::registry(), owner.clone(), cmd, max_per_owner)
            .await
    };
    match started {
        Ok((start, handle)) => {
            // The command was dispatched; its real exit code is
            // not known yet and the stats row is about
            // dispatch, not outcome.
            crate::daemon::stats::finish_command(cmd_id, 0);
            let text = format!(
                "Running in background on shell {} (run {}). The result is \
                                     not available yet — it will be delivered later as a \
                                     [Background Task Completed] message. Continue with other \
                                     work or end your turn; do not poll for it.",
                start.shell_id.as_str(),
                start.run_id
            );
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: text.clone(),
                },
            )
            .await?;
            crate::daemon::utils::log_shell_command(
                session_id,
                start.shell_id.as_str(),
                false,
                cmd,
                "approved",
                0,
                &text,
            );
            if sandboxed {
                let close_owner = owner.clone();
                tokio::spawn(bg_shell::await_notify_and_close(
                    sessions.clone(),
                    session_id.map(|s| s.to_string()).unwrap_or_default(),
                    cmd.to_string(),
                    start,
                    handle,
                    move |id: crate::shell::ShellId| async move {
                        let _ = crate::daemon::executor::knowledge::close_shell_with(
                            crate::shell::registry(),
                            id.as_str(),
                            &close_owner,
                            std::time::Duration::from_secs(5),
                        )
                        .await;
                    },
                ));
            } else {
                tokio::spawn(bg_shell::await_and_notify(
                    sessions.clone(),
                    session_id.map(|s| s.to_string()).unwrap_or_default(),
                    cmd.to_string(),
                    start,
                    handle,
                ));
            }
            Ok(ToolCallOutcome::Result(text))
        }
        Err(e) => {
            let shell_id = crate::shell::registry_lookup(&owner, |sid| sid.to_string())
                .unwrap_or_else(|| "s?".to_string());
            let msg = format!(
                "Error starting background command in shell {shell_id}: \
                                     {e:#}. Nothing was started, so retrying is safe."
            );
            crate::daemon::stats::finish_command(cmd_id, 1);
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: msg.clone(),
                },
            )
            .await?;
            crate::daemon::utils::log_shell_command(
                session_id,
                &shell_id,
                false,
                cmd,
                "shell-error",
                -1,
                &msg,
            );
            Ok(ToolCallOutcome::Result(msg))
        }
    }
}
