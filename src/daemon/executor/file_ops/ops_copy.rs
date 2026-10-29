//! File operation `copy`: approval prompt, then the local or remote run.

use super::*;

// ---------------------------------------------------------------------------
// operation = "copy"
// ---------------------------------------------------------------------------

pub(in super::super) async fn run_copy<W, R>(
    id: &str,
    src_path: &str,
    dest_path: Option<&str>,
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
    run_copy_with_spawn(id, src_path, dest_path, host, session_id, tx, rx, spawn).await
}

/// `run_copy` with the spawn seam explicit — tests inject an in-process PTY
/// host here instead of the production ssh spawn.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_copy_with_spawn<W, R>(
    id: &str,
    src_path: &str,
    dest_path: Option<&str>,
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
    let dest = match dest_path {
        Some(d) if !d.is_empty() => d,
        _ => {
            return Ok(ToolCallOutcome::Result(
                "Error: dest_path is required for operation=\"copy\".".to_string(),
            ));
        }
    };

    if dest.contains("..") || !std::path::Path::new(dest).is_absolute() {
        return Ok(ToolCallOutcome::Result(
            "Error: dest_path must be an absolute path and must not contain '..'.".to_string(),
        ));
    }

    // ── Host resolution: `Local` is the daemon host, `Named` a remote ──────
    match host {
        crate::host::HostId::Local => {}
        crate::host::HostId::Named(name) => {
            let safe_src = super::super::sq_escape(src_path);
            let safe_dst = super::super::sq_escape(dest);
            let cmd = format!(
                "if [ ! -e '{safe_src}' ]; then echo 'DE_ERROR: source not found: {safe_src}'; \
                 elif [ -e '{safe_dst}' ]; then echo 'DE_ERROR: destination already exists: {safe_dst}'; \
                 else cp -n -- '{safe_src}' '{safe_dst}' && echo 'DE_OK: Copied {safe_src} to {safe_dst}' \
                 || echo 'DE_ERROR: cp failed'; fi; echo '__DE_DONE__'"
            );

            // Show the approval prompt before executing — no local content available
            // for remote files, so show path info only (new_content = None).
            send_response_split(
                tx,
                Response::EditFilePrompt {
                    id: id.to_string(),
                    path: format!("{} (remote via host {})", src_path, name),
                    operation: "copy".to_string(),
                    existing_content: None,
                    new_content: None,
                    dest_path: Some(format!("{} (remote via host {})", dest, name)),
                    sudo: false,
                },
            )
            .await?;

            if let Err(outcome) = super::super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            let cmd_id = crate::daemon::stats::start_command(
                &format!("copy_file {} {}", src_path, dest),
                "foreground",
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
                        "file_copy",
                        serde_json::json!({ "session": session_id.unwrap_or("-"), "src": src_path, "dest": dest, "remote_host": name }),
                    );
                    return Ok(ToolCallOutcome::Result(format!(
                        "Copied {} to {} on host {}.",
                        src_path, dest, name
                    )));
                }
                Some(Err(line)) => {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error copying {} to {}: {}",
                        src_path, dest, line
                    )));
                }
                None => {}
            }
            crate::daemon::stats::finish_command(cmd_id, 1);
            return Ok(ToolCallOutcome::Result(format!(
                "Copy command completed but result was unclear. Check {} manually.",
                dest
            )));
        }
    }

    // ── Local path ────────────────────────────────────────────────────────
    let src_std = match std::fs::canonicalize(src_path) {
        Ok(p) => p,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error: cannot resolve source path {}: {}",
                src_path, e
            )));
        }
    };

    // Block copying from the daemoneye config dir.
    {
        let de_dir = crate::config::config_dir();
        if src_std.starts_with(&de_dir) {
            return Ok(ToolCallOutcome::Result(
                "Error: edit_file cannot access the daemoneye configuration directory.".to_string(),
            ));
        }
    }

    let dest_std = std::path::Path::new(dest);
    if dest_std.exists() {
        return Ok(ToolCallOutcome::Result(format!(
            "Error: destination already exists: {}. Remove it first or choose a different path.",
            dest
        )));
    }

    let source_content = match std::fs::read_to_string(&src_std) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error reading source {}: {}",
                src_path, e
            )));
        }
    };

    send_response_split(
        tx,
        Response::EditFilePrompt {
            id: id.to_string(),
            path: src_path.to_string(),
            operation: "copy".to_string(),
            existing_content: None,
            new_content: Some(source_content.clone()),
            dest_path: Some(dest.to_string()),
            sudo: false,
        },
    )
    .await?;

    if let Err(outcome) = super::super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    let cmd_id = crate::daemon::stats::start_command(
        &format!("copy_file {} {}", src_path, dest),
        "foreground",
    );

    if let Some(parent) = dest_std.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error creating destination directory: {}",
            e
        )));
    }

    let tmp_path = dest_std.with_extension("de_tmp");
    if let Err(e) = std::fs::write(&tmp_path, &source_content) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error writing temp file: {}",
            e
        )));
    }
    if let Err(e) = std::fs::rename(&tmp_path, dest_std) {
        let _ = std::fs::remove_file(&tmp_path);
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error committing copy: {}",
            e
        )));
    }

    crate::daemon::stats::finish_command(cmd_id, 0);
    crate::daemon::utils::log_event(
        "file_copy",
        serde_json::json!({ "session": session_id.unwrap_or("-"), "src": src_path, "dest": dest }),
    );
    let line_count = source_content.lines().count();
    Ok(ToolCallOutcome::Result(format!(
        "Copied {} to {}: {} line(s).",
        src_path, dest, line_count
    )))
}
