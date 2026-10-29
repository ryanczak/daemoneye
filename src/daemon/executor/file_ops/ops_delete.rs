//! File operation `delete`: approval prompt, then the local or remote run.

use super::*;

// ---------------------------------------------------------------------------
// operation = "delete"
// ---------------------------------------------------------------------------

pub(in super::super) async fn run_delete<W, R>(
    id: &str,
    path: &str,
    host: &crate::host::HostId,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let spawn = production_spawn_for(host);
    run_delete_with_spawn(id, path, host, session_id, tx, rx, spawn).await
}

/// `run_delete` with the spawn seam explicit — tests inject an in-process
/// PTY host here instead of the production ssh spawn.
pub(super) async fn run_delete_with_spawn<W, R>(
    id: &str,
    path: &str,
    host: &crate::host::HostId,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
    spawn: WriteSpawn,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    // ── Host resolution: `Local` is the daemon host, `Named` a remote ──────
    match host {
        crate::host::HostId::Local => {}
        crate::host::HostId::Named(name) => {
            // We can't read the remote file locally, so show the path only.
            send_response_split(
                tx,
                Response::EditFilePrompt {
                    id: id.to_string(),
                    path: format!("{} (remote via host {})", path, name),
                    operation: "delete".to_string(),
                    existing_content: None,
                    new_content: None,
                    dest_path: None,
                    sudo: false,
                },
            )
            .await?;

            if let Err(outcome) = super::super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            let cmd_id =
                crate::daemon::stats::start_command(&format!("delete_file {}", path), "foreground");

            let safe_path = super::super::sq_escape(path);
            let cmd = format!(
                "if [ -e '{safe_path}' ]; then rm -- '{safe_path}' && echo 'DE_OK: Deleted {safe_path}'; \
                 else echo 'DE_ERROR: file not found: {safe_path}'; fi; echo '__DE_DONE__'"
            );
            let snap = match super::super::run_in_host_shell(
                crate::shell::registry(),
                Owner::Chat(owner_key()),
                host.clone(),
                &cmd,
                REMOTE_WRITE_TIMEOUT_SECS,
                spawn,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!("Error: {}", e)));
                }
            };
            match super::super::remote_write_outcome(&snap) {
                Some(Ok(_)) => {
                    crate::daemon::stats::finish_command(cmd_id, 0);
                    crate::daemon::utils::log_event(
                        "file_delete",
                        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name }),
                    );
                    return Ok(ToolCallOutcome::Result(format!(
                        "Deleted {} on host {}.",
                        path, name
                    )));
                }
                Some(Err(line)) => {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error deleting {}: {}",
                        path, line
                    )));
                }
                None => {}
            }
            crate::daemon::stats::finish_command(cmd_id, 1);
            return Ok(ToolCallOutcome::Result(format!(
                "Delete command completed but result was unclear. Check {} manually.",
                path
            )));
        }
    }

    // ── Local path ────────────────────────────────────────────────────────
    let std_path = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error: cannot resolve path {}: {}",
                path, e
            )));
        }
    };
    let existing = match std::fs::read_to_string(&std_path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error reading {}: {}",
                path, e
            )));
        }
    };

    send_response_split(
        tx,
        Response::EditFilePrompt {
            id: id.to_string(),
            path: path.to_string(),
            operation: "delete".to_string(),
            existing_content: Some(existing.clone()),
            new_content: None,
            dest_path: None,
            sudo: false,
        },
    )
    .await?;

    if let Err(outcome) = super::super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    let cmd_id =
        crate::daemon::stats::start_command(&format!("delete_file {}", path), "foreground");

    if let Err(e) = std::fs::remove_file(&std_path) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error deleting {}: {}",
            path, e
        )));
    }

    crate::daemon::stats::finish_command(cmd_id, 0);
    crate::daemon::utils::log_event(
        "file_delete",
        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path }),
    );
    let line_count = existing.lines().count();
    Ok(ToolCallOutcome::Result(format!(
        "Deleted {}: {} line(s) removed.",
        path, line_count
    )))
}
