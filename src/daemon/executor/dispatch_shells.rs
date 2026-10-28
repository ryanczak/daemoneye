//! `execute_tool_call`'s arms for the shell readers and observers: `ListShells`, `ReadShell`, `ReadShellScreen`, `FindInShells`, `CloseShell`, `WatchShell`, `ListShellLogs`, `ReadShellLog`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W>(
    call: &PendingCall,
    tx: &mut W,
    session_id: Option<&str>,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match call {
        PendingCall::ListShells { scope, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::list_shells(
                scope.as_deref(),
                &Owner::Chat(session_id.unwrap_or_default().to_string()),
            )))
        }

        PendingCall::ReadShell {
            shell,
            lines,
            since_command,
            filter,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::read_shell(
            shell,
            *lines,
            *since_command,
            filter.as_deref(),
        ))),

        PendingCall::ReadShellScreen {
            shell,
            lines,
            scrollback,
            pattern,
            annotated,
            wait_for,
            settle_ms,
            timeout_secs,
            ..
        } => Ok(ToolCallOutcome::Result(
            crate::daemon::utils::with_keepalive(
                tx,
                knowledge::read_shell_screen(
                    shell,
                    *lines,
                    *scrollback,
                    pattern.as_deref(),
                    *annotated,
                    wait_for.as_deref(),
                    *settle_ms,
                    *timeout_secs,
                ),
            )
            .await?,
        )),

        PendingCall::FindInShells { pattern, scope, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::find_in_shells(
                pattern,
                scope.as_deref(),
                &Owner::Chat(session_id.unwrap_or_default().to_string()),
            )))
        }

        PendingCall::CloseShell { shell, .. } => Ok(ToolCallOutcome::Result(
            knowledge::close_shell(
                shell,
                &Owner::Chat(session_id.unwrap_or_default().to_string()),
            )
            .await,
        )),

        PendingCall::WatchShell {
            shell,
            pattern,
            timeout_secs,
            ..
        } => Ok(ToolCallOutcome::Result(
            crate::daemon::utils::with_keepalive(
                tx,
                knowledge::watch_shell(shell, pattern.as_deref(), *timeout_secs),
            )
            .await?,
        )),

        PendingCall::ListShellLogs { since, limit, .. } => Ok(ToolCallOutcome::Result(
            knowledge::list_shell_logs(*since, *limit),
        )),

        PendingCall::ReadShellLog {
            log,
            command_index,
            lines,
            filter,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::read_shell_log(
            log,
            *command_index,
            *lines,
            filter.as_deref(),
        ))),

        other => anyhow::bail!(
            "{} is not one of the shell readers and observers",
            other.tool_name()
        ),
    }
}
