use super::super::ToolCallOutcome;
use crate::daemon::utils::send_response_split;
use crate::ipc::Response;
use crate::shell::Owner;

/// The spawn closure a write op hands the transport: production resolves it
/// to a detached ssh host, a test to an in-process PTY. Boxed because the two
/// arms of the caller's host match produce different closure types.
pub(super) type WriteSpawn = Box<
    dyn FnOnce(
            crate::shell::ShellId,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
        + Send,
>;

/// The production spawn: a detached `daemoneye shell-host` running ssh to
/// `name`, in the daemon user's own PTY.
pub(super) fn production_spawn(name: &str) -> WriteSpawn {
    let name = name.to_string();
    Box::new(move |id| {
        Box::pin(async move { crate::shell::spawn_ssh_host(id, &name, 24, 80).await })
    })
}

/// The timeout one remote write is given, in seconds — the pane path's own
/// value, carried over unchanged.
const REMOTE_WRITE_TIMEOUT_SECS: u64 = 30;

/// The owner key a remote `edit_file` run claims its shell under, the same
/// tool-keyed shape `read_file` uses (`chat:read_file`): shells a write
/// claims are keyed to the tool, not the session id, so reuse on a host is
/// deterministic per call site and a chat session and a ghost cannot collide.
fn owner_key() -> String {
    "edit_file".to_string()
}

// ---------------------------------------------------------------------------
// operation = "edit"
// ---------------------------------------------------------------------------

pub(super) struct RunEditArgs<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub old_string: &'a str,
    pub new_string: &'a str,
    pub host: &'a crate::host::HostId,
    pub sudo: bool,
}

/// The production spawn for `host`: `Named(name)` spawns ssh to it, `Local`
/// never spawns (the local write path is direct filesystem access) and gets a
/// closure that is never called.
pub(super) fn production_spawn_for(host: &crate::host::HostId) -> WriteSpawn {
    match host {
        crate::host::HostId::Local => Box::new(|_id| Box::pin(async { Ok(()) })),
        crate::host::HostId::Named(name) => production_spawn(name),
    }
}

pub(super) async fn run_edit<W, R>(
    args: RunEditArgs<'_>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let spawn = production_spawn_for(args.host);
    run_edit_with_spawn(args, session_id, tx, rx, spawn).await
}

/// `run_edit` with the spawn seam explicit — tests inject an in-process PTY
/// host here instead of the production ssh spawn.
pub(super) async fn run_edit_with_spawn<W, R>(
    args: RunEditArgs<'_>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
    spawn: WriteSpawn,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let RunEditArgs {
        id,
        path,
        old_string,
        new_string,
        host,
        sudo,
    } = args;
    // ── Host resolution: `Local` is the daemon host, `Named` a remote ──────
    match host {
        crate::host::HostId::Local => {}
        crate::host::HostId::Named(name) => {
            let prompt_path = if sudo {
                format!("{} (remote via host {}, as root via sudo)", path, name)
            } else {
                format!("{} (remote via host {})", path, name)
            };
            send_response_split(
                tx,
                Response::EditFilePrompt {
                    id: id.to_string(),
                    path: prompt_path,
                    operation: "edit".to_string(),
                    // For remote files we can't read the full file locally, so show
                    // the old_string → new_string substitution as the diff context.
                    existing_content: Some(old_string.to_string()),
                    new_content: Some(new_string.to_string()),
                    dest_path: None,
                    sudo,
                },
            )
            .await?;

            if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            let cmd_id =
                crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");

            if sudo {
                let cmd = super::write::build_remote_sudo_edit_cmd(path, old_string, new_string);
                let max_per_owner = crate::config::Config::load()
                    .unwrap_or_default()
                    .shells
                    .max_per_owner;
                let run = super::super::shell_exec::run_in_shell_with_prompts(
                    crate::shell::registry(),
                    Owner::Chat(owner_key()),
                    &cmd,
                    max_per_owner,
                    None,
                    Some(REMOTE_WRITE_TIMEOUT_SECS),
                    id,
                    tx,
                    rx,
                    host.clone(),
                    spawn,
                )
                .await;
                let text = match run {
                    Ok((_, result)) => result.text,
                    Err(e) => {
                        crate::daemon::stats::finish_command(cmd_id, 1);
                        return Ok(ToolCallOutcome::Result(super::super::shell_run_error_text(
                            "edit_file",
                            &e,
                        )));
                    }
                };
                for line in text.lines().rev() {
                    if line.contains("DE_OK:") {
                        crate::daemon::stats::finish_command(cmd_id, 0);
                        crate::daemon::utils::log_event(
                            "file_edit",
                            serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name, "sudo": true }),
                        );
                        return Ok(ToolCallOutcome::Result(format!(
                            "Edited {} on host {}.",
                            path, name
                        )));
                    }
                    if line.contains("DE_ERROR:") {
                        crate::daemon::stats::finish_command(cmd_id, 1);
                        return Ok(ToolCallOutcome::Result(format!(
                            "Error editing {}: {}",
                            path,
                            line.trim()
                        )));
                    }
                }
                // No sentinel line at all: the password was refused or the
                // privilege tool never ran the payload. Loud, never "unclear".
                crate::daemon::stats::finish_command(cmd_id, 1);
                let tail = text.lines().rev().find(|l| !l.trim().is_empty());
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: the sudo edit of {path} on host {name} did not run — the password \
                     was not accepted or sudo refused it. Output: {}",
                    tail.unwrap_or("(no output)")
                )));
            }

            let cmd = super::write::build_remote_edit_cmd(path, old_string, new_string);
            let snap = match super::run_in_host_shell(
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
            for line in snap.lines().rev() {
                if line.contains("DE_OK:") {
                    crate::daemon::stats::finish_command(cmd_id, 0);
                    crate::daemon::utils::log_event(
                        "file_edit",
                        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name }),
                    );
                    return Ok(ToolCallOutcome::Result(format!(
                        "Edited {} on host {}.",
                        path, name
                    )));
                }
                if line.contains("DE_ERROR:") {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error editing {}: {}",
                        path,
                        line.trim()
                    )));
                }
            }
            crate::daemon::stats::finish_command(cmd_id, 1);
            return Ok(ToolCallOutcome::Result(format!(
                "Edit command completed but result was unclear. Check {} manually.",
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
    let original = match std::fs::read_to_string(&std_path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error reading {}: {}",
                path, e
            )));
        }
    };

    let count = original.matches(old_string).count();
    if count == 0 {
        return Ok(ToolCallOutcome::Result(format!(
            "Error: old_string not found in {}.",
            path
        )));
    }
    if count > 1 {
        return Ok(ToolCallOutcome::Result(format!(
            "Error: old_string appears {} times in {}. \
             Add more surrounding context to make it unique.",
            count, path
        )));
    }

    let updated = original.replacen(old_string, new_string, 1);

    send_response_split(
        tx,
        Response::EditFilePrompt {
            id: id.to_string(),
            path: path.to_string(),
            operation: "edit".to_string(),
            existing_content: Some(original.clone()),
            new_content: Some(updated.clone()),
            dest_path: None,
            sudo: false,
        },
    )
    .await?;

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    let cmd_id = crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    let original_mode = match std::fs::metadata(&std_path) {
        Ok(md) => md.permissions().mode(),
        Err(e) => {
            crate::daemon::stats::finish_command(cmd_id, 1);
            return Ok(ToolCallOutcome::Result(format!(
                "Error reading {}: {}",
                path, e
            )));
        }
    };
    let original_owner = std::fs::metadata(&std_path)
        .map(|md| (md.uid(), md.gid()))
        .unwrap_or((u32::MAX, u32::MAX));
    let tmp_path = std_path.with_extension("de_tmp");
    if let Err(e) = std::fs::write(&tmp_path, &updated) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error writing temp file: {}",
            e
        )));
    }
    if let Err(e) =
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(original_mode))
    {
        let _ = std::fs::remove_file(&tmp_path);
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error preserving the file's permissions: {}",
            e
        )));
    }
    // A non-root daemon may not change a file's owner; mode is the guarantee,
    // the owner is restored only where the writer is permitted to.
    let _ = std::os::unix::fs::chown(&tmp_path, Some(original_owner.0), Some(original_owner.1));
    if let Err(e) = std::fs::rename(&tmp_path, &std_path) {
        let _ = std::fs::remove_file(&tmp_path);
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error committing edit: {}",
            e
        )));
    }

    crate::daemon::stats::finish_command(cmd_id, 0);
    crate::daemon::utils::log_event(
        "file_edit",
        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path }),
    );

    let old_lines = old_string.lines().count();
    let new_lines = new_string.lines().count();
    Ok(ToolCallOutcome::Result(format!(
        "Edited {}: replaced {} line(s) with {} line(s).",
        path, old_lines, new_lines
    )))
}

// ---------------------------------------------------------------------------
// operation = "create"
// ---------------------------------------------------------------------------

fn build_remote_create_cmd(path: &str, content: &str) -> String {
    let path_hex = super::to_hex(path);
    let content_hex = super::to_hex(content);

    let py = format!(
        "import os,sys\n\
         p=bytes.fromhex('{path_hex}').decode()\n\
         c=bytes.fromhex('{content_hex}').decode()\n\
         if os.path.exists(p): print('DE_ERROR: file already exists: '+p); sys.exit(1)\n\
         os.makedirs(os.path.dirname(p) or '.', exist_ok=True)\n\
         t=p+'.de_tmp'\n\
         open(t,'w').write(c)\n\
         os.rename(t,p)\n\
         print('DE_OK: Created '+p)\n"
    );
    let py_hex = super::to_hex(&py);

    let pl = format!(
        "use File::Path qw(make_path);\n\
         use File::Basename qw(dirname);\n\
         my $p=pack('H*','{path_hex}');\n\
         my $c=pack('H*','{content_hex}');\n\
         if(-e $p){{print \"DE_ERROR: file already exists\\n\";exit 1}}\n\
         make_path(dirname($p));\n\
         my $t=\"$p.de_tmp\";\n\
         open(my $f,'>',$t) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print $f $c;close $f;\n\
         rename($t,$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print \"DE_OK: Created $p\\n\";\n"
    );
    let pl_hex = super::to_hex(&pl);

    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\" 2>&1; \
         else \
            perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))' 2>&1; \
         fi; echo '__DE_DONE__'"
    )
}

pub(super) async fn run_create<W, R>(
    id: &str,
    path: &str,
    content: Option<&str>,
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
    run_create_with_spawn(id, path, content, host, session_id, tx, rx, spawn).await
}

/// `run_create` with the spawn seam explicit — tests inject an in-process
/// PTY host here instead of the production ssh spawn.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_create_with_spawn<W, R>(
    id: &str,
    path: &str,
    content: Option<&str>,
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
    let content = match content {
        Some(c) => c,
        None => {
            return Ok(ToolCallOutcome::Result(
                "Error: content is required for operation=\"create\".".to_string(),
            ));
        }
    };

    // ── Host resolution: `Local` is the daemon host, `Named` a remote ──────
    match host {
        crate::host::HostId::Local => {}
        crate::host::HostId::Named(name) => {
            send_response_split(
                tx,
                Response::EditFilePrompt {
                    id: id.to_string(),
                    path: format!("{} (remote via host {})", path, name),
                    operation: "create".to_string(),
                    existing_content: None,
                    new_content: Some(content.to_string()),
                    dest_path: None,
                    sudo: false,
                },
            )
            .await?;

            if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            let cmd_id =
                crate::daemon::stats::start_command(&format!("create_file {}", path), "foreground");

            let cmd = build_remote_create_cmd(path, content);
            let snap = match super::run_in_host_shell(
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
            for line in snap.lines().rev() {
                if line.contains("DE_OK:") {
                    crate::daemon::stats::finish_command(cmd_id, 0);
                    crate::daemon::utils::log_event(
                        "file_create",
                        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name }),
                    );
                    return Ok(ToolCallOutcome::Result(format!(
                        "Created {} on host {}.",
                        path, name
                    )));
                }
                if line.contains("DE_ERROR:") {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error creating {}: {}",
                        path,
                        line.trim()
                    )));
                }
            }
            crate::daemon::stats::finish_command(cmd_id, 1);
            return Ok(ToolCallOutcome::Result(format!(
                "Create command completed but result was unclear. Check {} manually.",
                path
            )));
        }
    }

    // ── Local path ────────────────────────────────────────────────────────
    // For create, path need not exist yet — use parent to check directory.
    let std_path = std::path::Path::new(path);
    if std_path.exists() {
        return Ok(ToolCallOutcome::Result(format!(
            "Error: file already exists: {}. Use operation=\"edit\" to modify it.",
            path
        )));
    }

    send_response_split(
        tx,
        Response::EditFilePrompt {
            id: id.to_string(),
            path: path.to_string(),
            operation: "create".to_string(),
            existing_content: None,
            new_content: Some(content.to_string()),
            dest_path: None,
            sudo: false,
        },
    )
    .await?;

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    let cmd_id =
        crate::daemon::stats::start_command(&format!("create_file {}", path), "foreground");

    // Ensure parent directory exists.
    if let Some(parent) = std_path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error creating parent directory: {}",
            e
        )));
    }

    let tmp_path = std_path.with_extension("de_tmp");
    if let Err(e) = std::fs::write(&tmp_path, content) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error writing temp file: {}",
            e
        )));
    }
    if let Err(e) = std::fs::rename(&tmp_path, std_path) {
        let _ = std::fs::remove_file(&tmp_path);
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error committing new file: {}",
            e
        )));
    }

    crate::daemon::stats::finish_command(cmd_id, 0);
    crate::daemon::utils::log_event(
        "file_create",
        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path }),
    );
    let line_count = content.lines().count();
    Ok(ToolCallOutcome::Result(format!(
        "Created {}: {} line(s).",
        path, line_count
    )))
}

// ---------------------------------------------------------------------------
// operation = "delete"
// ---------------------------------------------------------------------------

pub(super) async fn run_delete<W, R>(
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

            if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            let cmd_id =
                crate::daemon::stats::start_command(&format!("delete_file {}", path), "foreground");

            let safe_path = super::sq_escape(path);
            let cmd = format!(
                "if [ -e '{safe_path}' ]; then rm -- '{safe_path}' && echo 'DE_OK: Deleted {safe_path}'; \
                 else echo 'DE_ERROR: file not found: {safe_path}'; fi; echo '__DE_DONE__'"
            );
            let snap = match super::run_in_host_shell(
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
            for line in snap.lines().rev() {
                if line.contains("DE_OK:") {
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
                if line.contains("DE_ERROR:") {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error deleting {}: {}",
                        path,
                        line.trim()
                    )));
                }
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

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
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

// ---------------------------------------------------------------------------
// operation = "copy"
// ---------------------------------------------------------------------------

pub(super) async fn run_copy<W, R>(
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
            let safe_src = super::sq_escape(src_path);
            let safe_dst = super::sq_escape(dest);
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

            if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            let cmd_id = crate::daemon::stats::start_command(
                &format!("copy_file {} {}", src_path, dest),
                "foreground",
            );

            let snap = match super::run_in_host_shell(
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
            for line in snap.lines().rev() {
                if line.contains("DE_OK:") {
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
                if line.contains("DE_ERROR:") {
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error copying {} to {}: {}",
                        src_path,
                        dest,
                        line.trim()
                    )));
                }
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

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
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

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::executor::ToolCallOutcome;
    use crate::daemon::executor::file_ops::run_in_host_shell;
    use crate::ipc::Request;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::Arc;

    // ── Shared fixtures ───────────────────────────────────────────────────

    /// A test `HOME` with `[hosts.web01]` configured, so the transport's
    /// `host_entry_or_bare` resolves against something real.
    fn remote_write_test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .unwrap();
        (guard, dir)
    }

    /// The in-process PTY host the transport tests drive, the same fixture
    /// `read.rs`, `shell_exec.rs` and `bg_shell.rs` hold their own copies of:
    /// a real bash session serving the id's socket, per-id instance lock held
    /// so the shell reads as live. `spawn_ssh_host` re-invokes ssh, which a
    /// test cannot use against a named host it cannot reach.
    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        use anyhow::Context as _;

        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    /// A two-pipe pump standing in for the client: the daemon's write half
    /// feeds the pump the `EditFilePrompt`, the pump records it and writes an
    /// approval back on the daemon's read half. Returns `(daemon_tx,
    /// daemon_rx, prompts)`; the record is what test 10 reads — a host write
    /// must open `EditFilePrompt`, not run unapproved.
    async fn auto_approve_channel() -> anyhow::Result<(
        tokio::io::DuplexStream,
        tokio::io::BufReader<tokio::io::DuplexStream>,
        Arc<std::sync::Mutex<Vec<String>>>,
    )> {
        let (mut to_pump, daemon_tx) = tokio::io::duplex(64 * 1024);
        let (pump_out, daemon_rx) = tokio::io::duplex(64 * 1024);
        let prompts: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let prompt_ref = Arc::clone(&prompts);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut pump_out = pump_out;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match tokio::io::AsyncReadExt::read(&mut to_pump, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        for line in text.lines() {
                            if let Ok(Response::EditFilePrompt { id, path, .. }) =
                                serde_json::from_str::<Response>(line)
                            {
                                let reply = Request::EditFileResponse {
                                    id,
                                    approved: true,
                                    user_message: None,
                                };
                                prompt_ref.lock().unwrap().push(path);
                                let mut wire = serde_json::to_string(&reply).unwrap();
                                wire.push('\n');
                                let _ = pump_out.write_all(wire.as_bytes()).await;
                            }
                        }
                    }
                }
            }
        });
        Ok((daemon_tx, tokio::io::BufReader::new(daemon_rx), prompts))
    }

    /// `approve_and_answer_channel`: the same two-pipe pump, but the client
    /// also handles a `CredentialPrompt` — it records `"cred"` and then either
    /// writes the credential back or, with `None`, drops its write half so the
    /// daemon reads EOF (the "nobody is attached" case).
    async fn approve_and_answer_channel(
        credential: Option<&'static str>,
    ) -> anyhow::Result<(
        tokio::io::DuplexStream,
        tokio::io::BufReader<tokio::io::DuplexStream>,
        Arc<std::sync::Mutex<Vec<String>>>,
    )> {
        let (mut to_pump, daemon_tx) = tokio::io::duplex(64 * 1024);
        let (pump_out, daemon_rx) = tokio::io::duplex(64 * 1024);
        let prompts: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let prompt_ref = Arc::clone(&prompts);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut pump_out = pump_out;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match tokio::io::AsyncReadExt::read(&mut to_pump, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        for line in text.lines() {
                            if let Ok(Response::EditFilePrompt { id, path, sudo, .. }) =
                                serde_json::from_str::<Response>(line)
                            {
                                let reply = Request::EditFileResponse {
                                    id,
                                    approved: true,
                                    user_message: None,
                                };
                                prompt_ref
                                    .lock()
                                    .unwrap()
                                    .push(format!("edit:{path}:sudo={sudo}"));
                                let mut wire = serde_json::to_string(&reply).unwrap();
                                wire.push('\n');
                                let _ = pump_out.write_all(wire.as_bytes()).await;
                                continue;
                            }
                            if let Ok(Response::CredentialPrompt { id, .. }) =
                                serde_json::from_str::<Response>(line)
                            {
                                prompt_ref.lock().unwrap().push("cred".to_string());
                                let Some(c) = credential else {
                                    return;
                                };
                                let reply = Request::CredentialResponse {
                                    id,
                                    credential: c.to_string(),
                                };
                                let mut wire = serde_json::to_string(&reply).unwrap();
                                wire.push('\n');
                                let _ = pump_out.write_all(wire.as_bytes()).await;
                            }
                        }
                    }
                }
            }
        });
        Ok((daemon_tx, tokio::io::BufReader::new(daemon_rx), prompts))
    }

    /// The Perl code is hex-encoded in the wire command, so check the source
    /// for the File::Path/make_path call and File::Basename/dirname import.
    #[test]
    fn remote_create_cmd_perl_branch_makes_parent_dirs() {
        let src = include_str!("ops.rs");
        assert!(
            src.contains("File::Path")
                && src.contains("make_path")
                && src.contains("File::Basename")
                && src.contains("dirname"),
            "Perl branch in source must contain File::Path/make_path and File::Basename/dirname"
        );
    }
    /// The Python code is hex-encoded in the wire command, so check the source
    /// for the makedirs call.
    #[test]
    fn remote_create_cmd_python_branch_unchanged() {
        let src = include_str!("ops.rs");
        assert!(
            src.contains("makedirs") && src.contains("exist_ok=True"),
            "Python branch in source must still contain makedirs with exist_ok=True"
        );
    }

    /// The remote copy command must use `cp -n` (no-clobber) — a silent
    /// overwrite would be a write the approval prompt never showed.
    #[test]
    fn remote_copy_cmd_is_no_clobber() {
        let src = include_str!("ops.rs");
        assert!(
            src.contains("cp -n --"),
            "remote copy command must use cp -n (no-clobber)"
        );
    }

    /// Test 9, the retirement: `target_pane` is gone from `edit_file`'s
    /// schema — and from the whole tool surface. This asserts against the
    /// `ToolDef`, not a grep, so the test fails if the parameter comes back
    /// under any spelling.
    #[test]
    fn edit_file_no_longer_accepts_target_pane() {
        let def = crate::ai::tools::TOOLS
            .iter()
            .find(|t| t.name == "edit_file")
            .unwrap_or_else(|| panic!("edit_file is in TOOLS"));
        let param_names: Vec<&str> = def.params.iter().map(|p| p.name).collect();
        assert!(
            !param_names.contains(&"target_pane"),
            "edit_file must not carry target_pane any more: {param_names:?}"
        );
        assert!(
            param_names.contains(&"host"),
            "edit_file carries host in its place: {param_names:?}"
        );
        let anywhere = crate::ai::tools::TOOLS
            .iter()
            .any(|t| t.params.iter().any(|p| p.name == "target_pane"));
        assert!(
            !anywhere,
            "target_pane is retired from the whole tool surface"
        );
    }

    /// The `DE_OK:` / `DE_ERROR:` sentinel contract, pinned for all four
    /// command builders. Edit's and create's builders hex-encode the
    /// Python/Perl payloads, so the sentinels live in the *source* (checked
    /// with `include_str!`); delete's and copy's emit them inline, so the
    /// built strings carry them (checked directly). Mutation B drifts one of
    /// these — the scan and the builder are a contract.
    #[test]
    fn the_write_sentinels_are_unchanged() {
        let create_cmd = build_remote_create_cmd("/tmp/x", "hi");
        assert!(
            create_cmd.contains("echo '__DE_DONE__'"),
            "create's builder must still terminate with the done marker: {create_cmd:?}"
        );
        let edit_cmd = super::super::write::build_remote_edit_cmd("/tmp/x", "a", "b");
        assert!(
            edit_cmd.contains("echo '__DE_DONE__'"),
            "edit's builder must still terminate with the done marker: {edit_cmd:?}"
        );
        let src = include_str!("ops.rs");
        for why in [
            "DE_ERROR: file already exists",
            "DE_OK: Created",
            "DE_OK: Edited",
            "DE_ERROR: file not found",
            "DE_OK: Deleted",
            "DE_ERROR: source not found",
            "DE_ERROR: destination already exists",
            "DE_OK: Copied",
            "DE_ERROR: cp failed",
            "DE_OK:",
            "DE_ERROR:",
        ] {
            assert!(src.contains(why), "the sentinel contract lost {why:?}");
        }
        // Both interpreter branches carry their own sentinel, so a drift in
        // one must halve the count, not hide behind the other's copy. The
        // builder's payload is hex-encoded, so decode it out of the built
        // command and check each branch's copy there.
        let decode_hex_from = |cmd: &str, what: &str| -> Option<String> {
            let start = cmd.find(what)? + what.len();
            let hex: String = cmd[start..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            (hex.len().is_multiple_of(2) && !hex.is_empty())
                .then(|| {
                    (0..hex.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(b'?'))
                        .collect::<Vec<u8>>()
                })
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        for (label, payload) in [
            ("create python", decode_hex_from(&create_cmd, "fromhex('")),
            (
                "create perl",
                decode_hex_from(&create_cmd, "pack(\"H*\",\""),
            ),
            ("edit python", decode_hex_from(&edit_cmd, "fromhex('")),
            ("edit perl", decode_hex_from(&edit_cmd, "pack(\"H*\",\"")),
        ] {
            let payload = payload.unwrap_or_else(|| panic!("{label} branch's hex payload decoded"));
            assert!(
                payload.contains("DE_OK:") && payload.contains("DE_ERROR:"),
                "{label} branch must still emit both sentinels: {payload:?}"
            );
        }
    }

    /// A mode a file carried before an edit must still be on it after one,
    /// on the local path: the temp file is chmod'd to the original's mode
    /// before the rename, so a `0600` file does not come back `0644` and a
    /// `0750` file keeps its execute bits under any umask.
    #[tokio::test]
    async fn a_local_edit_keeps_the_files_mode() {
        let (_home, dir) = remote_write_test_home();
        for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
            let file = dir.path().join(name);
            std::fs::write(&file, "a\nold-line\nb\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
            let outcome = run_edit_with_spawn(
                RunEditArgs {
                    id: "t09a",
                    path: file.to_str().unwrap(),
                    old_string: "old-line",
                    new_string: "new-line",
                    host: &crate::host::HostId::Local,
                    sudo: false,
                },
                Some("sess-09a"),
                &mut tx,
                &mut rx,
                Box::new(|_id| Box::pin(async { Ok(()) })),
            )
            .await
            .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
            let ToolCallOutcome::Result(s) = outcome else {
                panic!("expected a Result outcome")
            };
            assert!(s.contains("Edited"), "the local edit really ran: {s:?}");
            let edited = std::fs::read_to_string(&file).unwrap_or_default();
            assert!(
                edited.contains("new-line"),
                "the edit landed on the file: {edited:?}"
            );
            let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(got, mode, "{name} must keep its mode across an edit: {s:?}");
        }
    }

    /// The same preservation on the remote path: the op builds its own
    /// command and the in-process host shell runs the payload, so the chmod
    /// the payload carries is what the file's mode reflects afterwards.
    #[tokio::test]
    async fn a_remote_edit_keeps_the_files_mode() {
        let (_home, dir) = remote_write_test_home();
        for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
            let file = dir.path().join(name);
            std::fs::write(&file, "a\nold-line\nb\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
            let outcome = run_edit_with_spawn(
                RunEditArgs {
                    id: "t09a",
                    path: file.to_str().unwrap(),
                    old_string: "old-line",
                    new_string: "new-line",
                    host: &crate::host::HostId::Named("web01".to_string()),
                    sudo: false,
                },
                Some("sess-09a"),
                &mut tx,
                &mut rx,
                Box::new(|id| Box::pin(spawn_in_process_host(id))),
            )
            .await
            .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
            let ToolCallOutcome::Result(s) = outcome else {
                panic!("expected a Result outcome")
            };
            assert!(
                s.contains(&format!("Edited {} on host web01.", file.display())),
                "the approved host edit really ran: {s:?}"
            );
            let edited = std::fs::read_to_string(&file).unwrap_or_default();
            assert!(
                edited.contains("new-line"),
                "the edit landed on the host filesystem: {edited:?}"
            );
            let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(
                got, mode,
                "{name} must keep its mode across a host edit: {s:?}"
            );
        }
        let owner = Owner::Chat(owner_key());
        if let Some(id) = crate::shell::registry_lookup(&owner, |id| id.to_string()) {
            let _ = crate::daemon::executor::knowledge::close_shell_with(
                crate::shell::registry(),
                &id,
                &owner,
                std::time::Duration::from_millis(3000),
            )
            .await;
        }
    }

    /// The Python branch alone: the builder's payload is driven by a real
    /// `python3`, with `PATH` narrowed so only that interpreter resolves —
    /// the fallback would never be reached, so the assertion is about the
    /// branch itself.
    #[test]
    fn the_python_branch_keeps_the_files_mode() {
        let which = |prog: &str| -> String {
            let out = std::process::Command::new("sh")
                .args(["-c", &format!("command -v {prog}")])
                .output()
                .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let bash = which("bash");
        let python3 = which("python3");
        assert!(
            !bash.is_empty() && !python3.is_empty(),
            "bash and python3 must resolve"
        );
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&python3, bin.path().join("python3")).unwrap();
        for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join(name);
            std::fs::write(&file, "a\nold-line\nb\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let cmd = super::super::write::build_remote_edit_cmd(
                file.to_str().unwrap(),
                "old-line",
                "new-line",
            );
            let out = std::process::Command::new(&bash)
                .arg("-c")
                .arg(&cmd)
                .env("PATH", bin.path())
                .output()
                .unwrap_or_else(|e| panic!("running the python branch: {e}"));
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("DE_OK:"),
                "the python branch edited the file: {stdout:?}"
            );
            let edited = std::fs::read_to_string(&file).unwrap_or_default();
            assert!(edited.contains("new-line"), "the edit landed: {edited:?}");
            let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(
                got, mode,
                "{name} must keep its mode through the python branch: {stdout:?}"
            );
        }
    }

    /// The Perl branch alone: the same narrowed `PATH`, this time with only
    /// a `perl` symlink, so `command -v python3` fails and the fallback runs.
    #[test]
    fn the_perl_branch_keeps_the_files_mode() {
        let which = |prog: &str| -> String {
            let out = std::process::Command::new("sh")
                .args(["-c", &format!("command -v {prog}")])
                .output()
                .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let bash = which("bash");
        let perl = which("perl");
        assert!(
            !bash.is_empty() && !perl.is_empty(),
            "bash and perl must resolve"
        );
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&perl, bin.path().join("perl")).unwrap();
        for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join(name);
            std::fs::write(&file, "a\nold-line\nb\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let cmd = super::super::write::build_remote_edit_cmd(
                file.to_str().unwrap(),
                "old-line",
                "new-line",
            );
            let out = std::process::Command::new(&bash)
                .arg("-c")
                .arg(&cmd)
                .env("PATH", bin.path())
                .output()
                .unwrap_or_else(|e| panic!("running the perl branch: {e}"));
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("DE_OK:"),
                "the perl branch edited the file: {stdout:?}"
            );
            let edited = std::fs::read_to_string(&file).unwrap_or_default();
            assert!(edited.contains("new-line"), "the edit landed: {edited:?}");
            let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(
                got, mode,
                "{name} must keep its mode through the perl branch: {stdout:?}"
            );
        }
    }

    /// Owner restoration cannot be exercised without root (a non-root
    /// writer's chown is a no-op by kernel rule), so the payload that does it
    /// is pinned by decoding both hex payloads out of the builder, the way
    /// `the_write_sentinels_are_unchanged` does. The mode tests above pin the
    /// observable half.
    #[test]
    fn the_edit_payloads_restore_ownership() {
        let edit_cmd = super::super::write::build_remote_edit_cmd("/tmp/x", "a", "b");
        let decode_hex_from = |cmd: &str, what: &str| -> Option<String> {
            let start = cmd.find(what)? + what.len();
            let hex: String = cmd[start..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            (hex.len().is_multiple_of(2) && !hex.is_empty())
                .then(|| {
                    (0..hex.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(b'?'))
                        .collect::<Vec<u8>>()
                })
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        let python = decode_hex_from(&edit_cmd, "fromhex('")
            .unwrap_or_else(|| panic!("the python branch's hex payload decoded"));
        assert!(
            python.contains("os.chmod(t,st.st_mode&0o7777)")
                && python.contains("os.chown(t,st.st_uid,st.st_gid)"),
            "the python branch must chmod the temp file to the original's mode and chown it: {python:?}"
        );
        let perl = decode_hex_from(&edit_cmd, "pack(\"H*\",\"")
            .unwrap_or_else(|| panic!("the perl branch's hex payload decoded"));
        assert!(
            perl.contains("chmod($st[2]&07777,$t)") && perl.contains("chown($st[4],$st[5],$t)"),
            "the perl branch must chmod the temp file to the original's mode and chown it: {perl:?}"
        );
    }

    /// Test 10, the security preservation: a **host** write is still gated by
    /// `EditFilePrompt` before anything runs. The transport must not create an
    /// unapproved write path.
    #[tokio::test]
    async fn a_remote_write_still_prompts_for_approval() {
        let (_home, _dir) = remote_write_test_home();
        let probe = std::env::temp_dir().join(format!("de-07b-approval-{}", std::process::id()));
        let _ = std::fs::remove_file(&probe);
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();

        let outcome = run_create_with_spawn(
            "t10",
            probe.to_str().unwrap(),
            Some("approved only\n"),
            &crate::host::HostId::Named("web01".to_string()),
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
        let got = prompts.lock().unwrap().clone();
        assert_eq!(
            got.len(),
            1,
            "exactly one EditFilePrompt must fire before the host run: {got:?}"
        );
        assert!(
            got[0].contains("(remote via host web01)"),
            "the prompt names the host: {:?}",
            got[0]
        );
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Created {} on host web01.", probe.display())),
            "the approved host write really ran: {s:?}"
        );
        let written = std::fs::read_to_string(&probe).unwrap_or_default();
        assert!(
            written.contains("approved only"),
            "the file landed on the (in-process) host filesystem: {written:?}"
        );
        let _ = std::fs::remove_file(&probe);
    }

    /// Test 5, preservation: a local create produces byte-identical output to
    /// before this phase — the same prompt (the client sees the plain path,
    /// no host named). Approval is denied by construction here (`rx` is a pipe
    /// nobody answers, so the await ends in the same "User denied execution" a
    /// silent client produces); the *sentences*, not the write, are what this
    /// test pins.
    #[tokio::test]
    async fn the_local_write_path_is_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fresh.txt");
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_create(
            "t1",
            path.to_str().unwrap(),
            Some("hello local\n"),
            &crate::host::HostId::Local,
            Some("sess-1"),
            &mut tx,
            &mut rx,
        )
        .await
        .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
        let prompt = prompts.lock().unwrap().join("\n");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            !s.contains("on host "),
            "a local create must not name a host: {s:?}"
        );
        assert!(
            !prompt.contains("(remote via host"),
            "a local create's prompt must not name a remote host: {prompt:?}"
        );
        assert!(
            prompt.contains(path.to_str().unwrap()),
            "the prompt names the plain path, as before: {prompt:?}"
        );
    }

    // ── Tests 1–4: each op runs end to end on a named host ────────────────
    //
    // The four "runs through the shell" tests drive the **operation** itself —
    // `run_<op>_with_spawn` with `HostId::Named("web01")` and the in-process
    // PTY host — the same seam test 10 (`a_remote_write_still_prompts_for_approval`)
    // uses. The command the op builds reaches the shell through the op's own
    // branch, and the assertion is on the **filesystem effect**, so a
    // named-host branch that reports `DE_OK` without doing the work (the
    // class bug-07b-1 was filed for) turns its test red.

    /// Test 1: a create on a named host runs through the shell — the op's
    /// own command, the op's own scan, the file landing on the host
    /// filesystem.
    #[tokio::test]
    async fn create_on_a_named_host_runs_through_the_shell() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("probe-create.txt");
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_create_with_spawn(
            "t1",
            probe.to_str().unwrap(),
            Some("through the shell\n"),
            &crate::host::HostId::Named("web01".to_string()),
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Created {} on host web01.", probe.display())),
            "the approved host create really ran: {s:?}"
        );
        let written = std::fs::read_to_string(&probe).unwrap_or_default();
        assert!(
            written.contains("through the shell"),
            "the file landed on the (in-process) host filesystem: {written:?}"
        );
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "the create is gated by exactly one EditFilePrompt"
        );
    }

    /// Test 2: a delete on a named host runs through the shell — the op
    /// itself, not a retyped `rm`.
    #[tokio::test]
    async fn delete_on_a_named_host_runs_through_the_shell() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("probe-delete.txt");
        std::fs::write(&probe, "to be deleted").unwrap();
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_delete_with_spawn(
            "t2",
            probe.to_str().unwrap(),
            &crate::host::HostId::Named("web01".to_string()),
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_delete must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Deleted {} on host web01.", probe.display())),
            "the approved host delete really ran: {s:?}"
        );
        assert!(
            !probe.exists(),
            "the file really was deleted by the op's own command"
        );
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "the delete is gated by exactly one EditFilePrompt"
        );
    }

    /// Test 3: a copy on a named host runs through the shell — the op
    /// itself, not a retyped `cp`.
    #[tokio::test]
    async fn copy_on_a_named_host_runs_through_the_shell() {
        let (_home, dir) = remote_write_test_home();
        let src = dir.path().join("probe-src.txt");
        let dst = dir.path().join("probe-dst.txt");
        std::fs::write(&src, "copy me").unwrap();
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_copy_with_spawn(
            "t3",
            src.to_str().unwrap(),
            Some(dst.to_str().unwrap()),
            &crate::host::HostId::Named("web01".to_string()),
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_copy must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!(
                "Copied {} to {} on host web01.",
                src.display(),
                dst.display()
            )),
            "the approved host copy really ran: {s:?}"
        );
        let copied = std::fs::read_to_string(&dst).unwrap_or_default();
        assert!(
            copied.contains("copy me"),
            "the copy landed on the host filesystem: {copied:?}"
        );
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "the copy is gated by exactly one EditFilePrompt"
        );
    }

    /// Test 4: an edit on a named host runs through the shell — the op
    /// itself, its own builder's payload, not a builder call bypassing the
    /// branch.
    #[tokio::test]
    async fn edit_on_a_named_host_runs_through_the_shell() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("probe-edit.txt");
        std::fs::write(&probe, "the old string here").unwrap();
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t4",
                path: probe.to_str().unwrap(),
                old_string: "the old string",
                new_string: "the new string",
                host: &crate::host::HostId::Named("web01".to_string()),
                sudo: false,
            },
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Edited {} on host web01.", probe.display())),
            "the approved host edit really ran: {s:?}"
        );
        let edited = std::fs::read_to_string(&probe).unwrap_or_default();
        assert!(
            edited.contains("the new string"),
            "the edit landed on the host filesystem: {edited:?}"
        );
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "the edit is gated by exactly one EditFilePrompt"
        );
    }

    /// Test 7, the § Current state 3 negative: a remote write's success
    /// sentence and its approval prompt both name the **host**, and neither
    /// contains the word `pane`. Byte-pins the string change the row made.
    #[tokio::test]
    async fn a_remote_write_names_the_host_not_a_pane() {
        let (_home, _dir) = remote_write_test_home();
        let probe = std::env::temp_dir().join(format!("de-07b-names-{}", std::process::id()));
        let _ = std::fs::remove_file(&probe);
        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();

        let outcome = run_create_with_spawn(
            "t7",
            probe.to_str().unwrap(),
            Some("host named\n"),
            &crate::host::HostId::Named("web01".to_string()),
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
        let got = prompts.lock().unwrap().clone();
        let prompt = got.join("\n");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Created {} on host web01.", probe.display())),
            "the success sentence names the host: {s:?}"
        );
        assert!(!s.contains("pane"), "the sentence must not say pane: {s:?}");
        assert!(
            prompt.contains("(remote via host web01)"),
            "the prompt names the host: {prompt:?}"
        );
        assert!(
            !prompt.contains("pane"),
            "the prompt must not say pane: {prompt:?}"
        );
        let _ = std::fs::remove_file(&probe);
    }

    /// Test 6: a malformed host name is refused with `HostId::parse`'s own
    /// message before anything is written and before any shell is opened —
    /// driven through `run_edit_file`, where the resolution is the first
    /// thing the host argument meets.
    #[tokio::test]
    async fn a_malformed_host_is_refused_by_edit_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("never-written.txt");

        let outcome = super::super::run_edit_file(
            super::super::EditArgs {
                id: "t6",
                path: path.to_str().unwrap(),
                operation: "create",
                old_string: None,
                new_string: None,
                content: Some("must not land\n"),
                dest_path: None,
                host: Some("../etc"),
                sudo: false,
            },
            Some("sess-1"),
            super::super::super::GhostCtx {
                policy: None,
                is_ghost: false,
            },
            &mut tokio::io::sink(),
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await
        .unwrap_or_else(|e| panic!("run_edit_file must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains("Error:") && s.contains("../etc") && s.contains("A-Za-z0-9._-"),
            "HostId::parse's own message comes back: {s:?}"
        );
        assert!(
            !path.exists(),
            "nothing was written on a malformed host name"
        );
    }

    /// Test 11, the deletion negative: `get_pane_remote_host` survives with
    /// its two live callers — the tmux foreground path in `prompt.rs` and
    /// `foreground.rs`, which file ops never owned.
    #[test]
    fn get_pane_remote_host_still_has_its_callers() {
        let src = [
            include_str!("../../prompt.rs"),
            include_str!("../../executor/foreground.rs"),
        ];
        let mut callers = 0;
        for file in src {
            if file.contains("get_pane_remote_host") {
                callers += 1;
            }
        }
        assert_eq!(
            callers, 2,
            "get_pane_remote_host keeps exactly its two live callers (prompt.rs, foreground.rs)"
        );
        let helper = include_str!("../../utils/host.rs");
        assert!(
            helper.contains("pub fn get_pane_remote_host"),
            "the helper itself still exists — deleting it was never this row's job"
        );
    }

    /// The § Live verification, in the same seam `read_file_on_pinky_end_to_end`
    /// established: the real ssh transport to the live host `pinky`, the real
    /// approval pump, the real builder and the real scan — everything but the
    /// TUI, which cannot be driven from a headless run. Run with
    /// `cargo test --lib edit_file_on_pinky_end_to_end -- --ignored`.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn edit_file_on_pinky_end_to_end() {
        let (_home, dir) = {
            let g = crate::test_home_guard();
            let dir = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", dir.path()) }
            crate::config::Config::ensure_dirs().ok();
            (g, dir)
        };
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();

        let reg = crate::shell::ShellRegistry::default();
        let owner = Owner::Chat(owner_key());
        let probe = "/tmp/de-07b-probe.txt";

        let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_create_with_spawn(
            "t-live",
            probe,
            Some("wrote-through-daemoneye\n"),
            &crate::host::HostId::Named("pinky".to_string()),
            Some("live07b"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id))),
        )
        .await
        .expect("the pinky create resolves");
        let got = prompts.lock().unwrap().clone();
        assert_eq!(
            got.len(),
            1,
            "exactly one EditFilePrompt fired before the remote run: {got:?}"
        );
        assert!(
            got[0].contains("(remote via host pinky)"),
            "the prompt names the host: {:?}",
            got[0]
        );
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Created {probe} on host pinky.")),
            "the success sentence names the host: {s:?}"
        );
        println!("PASTE MATCH >>>\nedit_file_on_pinky create sentence={s:?}\n<<<");

        // The write really landed on pinky's filesystem — read it back
        // through the same host transport.
        // The ops run against the daemon's global registry, not a local one —
        // the readback must reuse that same shell, so it names the same
        // registry and the same owner key.
        let readback = run_in_host_shell(
            crate::shell::registry(),
            owner.clone(),
            crate::host::HostId::Named("pinky".to_string()),
            &format!("cat '{probe}'; rm -f '{probe}'"),
            REMOTE_WRITE_TIMEOUT_SECS,
            |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
        )
        .await
        .expect("the pinky readback resolves");
        assert!(
            readback.contains("wrote-through-daemoneye"),
            "the file on pinky carries the content: {readback:?}"
        );
        println!("PASTE MATCH >>>\nedit_file_on_pinky readback={readback:?}\n<<<");

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_default()
                .as_str(),
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
        let cp = crate::config::ssh_dir();
        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", cp.display()),
                "pinky",
            ])
            .status();
        let _ = dir;
    }

    /// The live, mode-checking variant: a `0600` probe on the real host
    /// `pinky` — whose PTY shell's umask is `0002` — must still be `0600`
    /// after an edit, not `0664`. Run with
    /// `cargo test --lib a_remote_edit_keeps_mode_on_pinky -- --ignored`.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn a_remote_edit_keeps_mode_on_pinky() {
        let (_home, dir) = {
            let g = crate::test_home_guard();
            let dir = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", dir.path()) }
            crate::config::Config::ensure_dirs().ok();
            (g, dir)
        };
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();

        let reg = crate::shell::ShellRegistry::default();
        let owner = Owner::Chat(owner_key());
        let probe = "/tmp/de-09a-mode-probe";

        let _ = run_in_host_shell(
            crate::shell::registry(),
            owner.clone(),
            crate::host::HostId::Named("pinky".to_string()),
            "printf 'a\\nold-line\\nb\\n' > /tmp/de-09a-mode-probe && chmod 600 /tmp/de-09a-mode-probe && stat -c before=%a /tmp/de-09a-mode-probe",
            REMOTE_WRITE_TIMEOUT_SECS,
            |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
        )
        .await
        .expect("the pinky probe setup resolves");

        let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09a-live",
                path: probe,
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Named("pinky".to_string()),
                sudo: false,
            },
            Some("live09a"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id))),
        )
        .await
        .expect("the pinky edit resolves");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Edited {probe} on host pinky.")),
            "the approved host edit really ran: {s:?}"
        );

        let readback = run_in_host_shell(
            crate::shell::registry(),
            owner.clone(),
            crate::host::HostId::Named("pinky".to_string()),
            "stat -c after=%a /tmp/de-09a-mode-probe; rm -f /tmp/de-09a-mode-probe",
            REMOTE_WRITE_TIMEOUT_SECS,
            |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
        )
        .await
        .expect("the pinky readback resolves");
        println!("before=600 (stat -c before=%a /tmp/de-09a-mode-probe)");
        println!("after: {readback:?}");
        assert!(
            readback.contains("after=600"),
            "the edited file keeps its mode on pinky: {readback:?}"
        );

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_default()
                .as_str(),
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
        let cp = crate::config::ssh_dir();
        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", cp.display()),
                "pinky",
            ])
            .status();
        let _ = dir;
    }

    // ── 09b: the root edit ─────────────────────────────────────────────────

    /// A fake privilege tool the shell defines before the edit runs: it
    /// prompts with `$SUDO_PROMPT`, accepts only `pw-xyz`, and then runs its
    /// arguments. Spelled `f=su; f+=do; eval …` so the defining command does
    /// not itself name the privilege tool and is not routed through the
    /// prompt path — the same trick `shell_exec.rs`'s `PROMPTING` uses.
    const FAKE_ROOT: &str = r#"f=su; f+=do; eval "$f() { local pw=; read -rs -p \"\$SUDO_PROMPT\" pw || return 1; echo; [ \"\$pw\" = pw-xyz ] || return 1; \"\$@\"; }""#;

    /// Seed `FAKE_ROOT` into the shell the edit will reuse, so the spawned
    /// in-process host's bash already holds the fake and the spawn closure is
    /// never called again.
    async fn seed_fake_root(host: &str) {
        run_in_host_shell(
            crate::shell::registry(),
            Owner::Chat(owner_key()),
            crate::host::HostId::Named(host.to_string()),
            FAKE_ROOT,
            REMOTE_WRITE_TIMEOUT_SECS,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .expect("seed the fake privilege tool");
    }

    /// Close every shell the tool-keyed owner holds, so a leaked host does
    /// not make unrelated PTY tests fail at random (09a's lesson).
    async fn close_tool_shells() {
        let owner = Owner::Chat(owner_key());
        if let Some(id) = crate::shell::registry_lookup(&owner, |id| id.to_string()) {
            let _ = crate::daemon::executor::knowledge::close_shell_with(
                crate::shell::registry(),
                &id,
                &owner,
                std::time::Duration::from_millis(3000),
            )
            .await;
        }
    }

    /// The answered path: the password is typed in the chat, the edit runs as
    /// root, and the file's mode survives — the whole point of 09b.
    #[tokio::test]
    async fn a_root_edit_on_a_host_answers_the_password_prompt() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("probe-root.txt");
        std::fs::write(&probe, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o640)).unwrap();
        seed_fake_root("web01").await;
        let (mut tx, mut rx, prompts) = approve_and_answer_channel(Some("pw-xyz")).await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09b-1",
                path: probe.to_str().unwrap(),
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Named("web01".to_string()),
                sudo: true,
            },
            Some("sess-09b"),
            &mut tx,
            &mut rx,
            Box::new(|_id| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the root edit resolves");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Edited {} on host web01.", probe.display())),
            "the approved root edit really ran: {s:?}"
        );
        let edited = std::fs::read_to_string(&probe).unwrap_or_default();
        assert!(
            edited.contains("new-line"),
            "the edit landed on the file: {edited:?}"
        );
        let got = std::fs::metadata(&probe).unwrap().permissions().mode() & 0o7777;
        assert_eq!(got, 0o640, "the root edit keeps the file's mode: {s:?}");
        let got = prompts.lock().unwrap().clone();
        let edits: Vec<_> = got.iter().filter(|l| l.starts_with("edit:")).collect();
        let creds: Vec<_> = got.iter().filter(|l| l == &"cred").collect();
        assert_eq!(edits.len(), 1, "exactly one edit prompt fired: {got:?}");
        assert!(
            edits[0].contains("sudo=true") && edits[0].contains("as root via sudo"),
            "the prompt carries the root signal: {:?}",
            edits[0]
        );
        assert_eq!(
            creds.len(),
            1,
            "exactly one credential round trip fired: {got:?}"
        );
        close_tool_shells().await;
    }

    /// The give-up: nobody is attached, so the prompt is cancelled and the
    /// file is untouched — the failure is loud, not "result was unclear".
    #[tokio::test]
    async fn an_unanswered_root_edit_leaves_the_file_unedited() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("probe-nobody.txt");
        std::fs::write(&probe, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o640)).unwrap();
        seed_fake_root("web01").await;
        let (mut tx, mut rx, _prompts) = approve_and_answer_channel(None).await.unwrap();
        let started = std::time::Instant::now();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09b-2",
                path: probe.to_str().unwrap(),
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Named("web01".to_string()),
                sudo: true,
            },
            Some("sess-09b"),
            &mut tx,
            &mut rx,
            Box::new(|_id| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the root edit resolves");
        let elapsed = started.elapsed();
        println!("root-edit give-up elapsed: {elapsed:?}");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains("Error") && s.contains("nobody is attached to answer it"),
            "the failure is loud, naming why: {s:?}"
        );
        assert!(
            !s.contains("result was unclear"),
            "a cancelled prompt is never \"result was unclear\": {s:?}"
        );
        let edited = std::fs::read_to_string(&probe).unwrap_or_default();
        assert!(
            edited.contains("old-line"),
            "the file is untouched: {edited:?}"
        );
        close_tool_shells().await;
    }

    /// A wrong password leaves no sentinel line at all — today's scan calls
    /// that "result was unclear"; for a root edit it must be an `Error:`
    /// saying the edit did not run.
    #[tokio::test]
    async fn a_wrong_password_root_edit_leaves_the_file_unedited() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("probe-wrong.txt");
        std::fs::write(&probe, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o640)).unwrap();
        seed_fake_root("web01").await;
        let (mut tx, mut rx, _prompts) = approve_and_answer_channel(Some("wrong")).await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09b-3",
                path: probe.to_str().unwrap(),
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Named("web01".to_string()),
                sudo: true,
            },
            Some("sess-09b"),
            &mut tx,
            &mut rx,
            Box::new(|_id| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the root edit resolves");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.starts_with("Error:") && s.contains("did not run"),
            "a refused password says the edit did not run: {s:?}"
        );
        assert!(
            !s.contains("unclear"),
            "never \"result was unclear\" on a root edit: {s:?}"
        );
        let edited = std::fs::read_to_string(&probe).unwrap_or_default();
        assert!(
            edited.contains("old-line"),
            "the file is untouched: {edited:?}"
        );
        close_tool_shells().await;
    }

    /// The scoping refusals, before any prompt: the root edit needs a
    /// configured host and the `edit` operation, and nothing is offered for
    /// approval in either refusal.
    #[tokio::test]
    async fn a_root_edit_needs_a_host_and_the_edit_operation() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("never-edited.txt");
        let mut out: Vec<u8> = Vec::new();
        let outcome = super::super::run_edit_file(
            super::super::EditArgs {
                id: "t09b-4",
                path: probe.to_str().unwrap(),
                operation: "edit",
                old_string: Some("old-line"),
                new_string: Some("new-line"),
                content: None,
                dest_path: None,
                host: None,
                sudo: true,
            },
            Some("sess-09b"),
            super::super::super::GhostCtx {
                policy: None,
                is_ghost: false,
            },
            &mut out,
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await
        .expect("the local root edit is refused");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains("runs on a configured host"),
            "the refusal names the missing host: {s:?}"
        );
        let outcome = super::super::run_edit_file(
            super::super::EditArgs {
                id: "t09b-4",
                path: probe.to_str().unwrap(),
                operation: "create",
                old_string: None,
                new_string: None,
                content: Some("nope\n"),
                dest_path: None,
                host: Some("web01"),
                sudo: true,
            },
            Some("sess-09b"),
            super::super::super::GhostCtx {
                policy: None,
                is_ghost: false,
            },
            &mut out,
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await
        .expect("the create root op is refused");
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains("operation=\"edit\" only"),
            "the refusal names the only operation: {s:?}"
        );
        assert!(
            out.is_empty(),
            "no prompt reached the client before either refusal: {} bytes",
            out.len()
        );
    }

    /// The line is detected as a privilege-tool command (so the run path
    /// turns the prompt handling on) and its payloads are the same bytes the
    /// plain edit's builder produces — shared, not copied.
    #[test]
    fn the_root_edit_line_is_detected_and_reuses_the_payloads() {
        let cmd = super::super::write::build_remote_sudo_edit_cmd("/tmp/x", "a", "b");
        assert!(
            crate::daemon::utils::command_has_sudo(&cmd),
            "the run path must classify the line as a privilege-tool command: {cmd:?}"
        );
        assert!(
            cmd.contains("set -- python3 -c") && cmd.contains("set -- perl -e"),
            "the interpreter is chosen with set --, not inside the branch: {cmd:?}"
        );
        let decode_hex_from = |cmd: &str, what: &str| -> Option<String> {
            let start = cmd.find(what)? + what.len();
            let hex: String = cmd[start..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            (hex.len().is_multiple_of(2) && !hex.is_empty())
                .then(|| {
                    (0..hex.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(b'?'))
                        .collect::<Vec<u8>>()
                })
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        let plain = super::super::write::build_remote_edit_cmd("/tmp/x", "a", "b");
        for (what, label) in [("fromhex('", "python"), ("pack(\"H*\",\"", "perl")] {
            let a = decode_hex_from(&cmd, what)
                .unwrap_or_else(|| panic!("the root line's {label} payload decoded"));
            let b = decode_hex_from(&plain, what)
                .unwrap_or_else(|| panic!("the plain line's {label} payload decoded"));
            assert_eq!(
                a, b,
                "the root line's {label} payload is the plain edit's payload"
            );
            assert!(
                a.contains("DE_OK: Edited") && a.contains("DE_ERROR:"),
                "the {label} payload still carries the sentinel contract"
            );
        }
    }

    /// Both interpreter branches of the root line run a real edit: with
    /// `PATH` narrowed to a `bin` dir holding only the interpreter and an
    /// executable pass-through named for the privilege tool, the line's own
    /// mechanics are pinned — the `set --` selection, the `"$@"` run, the
    /// sentinels.
    #[test]
    fn the_root_edit_line_runs_under_both_interpreters() {
        let which = |prog: &str| -> String {
            let out = std::process::Command::new("sh")
                .args(["-c", &format!("command -v {prog}")])
                .output()
                .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let bash = which("bash");
        assert!(!bash.is_empty(), "bash must resolve");
        for prog in ["python3", "perl"] {
            let interp = which(prog);
            assert!(!interp.is_empty(), "{prog} must resolve");
            let bin = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(&interp, bin.path().join(prog)).unwrap();
            let fake = bin.path().join("sudo");
            std::fs::write(&fake, "#!/bin/sh\nexec \"$@\"\n").unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
                let dir = tempfile::tempdir().unwrap();
                let file = dir.path().join(name);
                std::fs::write(&file, "a\nold-line\nb\n").unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
                let cmd = super::super::write::build_remote_sudo_edit_cmd(
                    file.to_str().unwrap(),
                    "old-line",
                    "new-line",
                );
                assert!(
                    cmd.contains(&format!("set -- {prog}")),
                    "the {prog} branch was selected: {cmd:?}"
                );
                let out = std::process::Command::new(&bash)
                    .arg("-c")
                    .arg(&cmd)
                    .env("PATH", bin.path())
                    .output()
                    .unwrap_or_else(|e| panic!("running the {prog} branch: {e}"));
                let stdout = String::from_utf8_lossy(&out.stdout);
                assert!(
                    stdout.contains("DE_OK:"),
                    "the {prog} branch edited the file: {stdout:?}"
                );
                let edited = std::fs::read_to_string(&file).unwrap_or_default();
                assert!(edited.contains("new-line"), "the edit landed: {edited:?}");
                let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
                assert_eq!(
                    got, mode,
                    "{name} must keep its mode through the {prog} branch: {stdout:?}"
                );
            }
        }
    }

    /// The § Live verification, in the seam `edit_file_on_pinky_end_to_end`
    /// established: the real privilege tool (sudo-rs 0.2.13) on the live host
    /// `pinky`, nobody attached. The **answered** path needs the real
    /// password, so it is 09c's live close, not this test. Run with
    /// `cargo test --lib a_root_edit_on_pinky_with_nobody_attached_fails_cleanly -- --ignored`.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn a_root_edit_on_pinky_with_nobody_attached_fails_cleanly() {
        let (_home, dir) = {
            let g = crate::test_home_guard();
            let dir = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", dir.path()) }
            crate::config::Config::ensure_dirs().ok();
            (g, dir)
        };
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();

        let reg = crate::shell::ShellRegistry::default();
        let owner = Owner::Chat(owner_key());
        let probe = "/tmp/de-09b-probe";

        let _ = run_in_host_shell(
            crate::shell::registry(),
            owner.clone(),
            crate::host::HostId::Named("pinky".to_string()),
            "printf 'a\\nold-line\\nb\\n' > /tmp/de-09b-probe && chmod 640 /tmp/de-09b-probe",
            REMOTE_WRITE_TIMEOUT_SECS,
            |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
        )
        .await
        .expect("the pinky probe setup resolves");

        let (mut tx, mut rx, _prompts) = approve_and_answer_channel(None).await.unwrap();
        let started = std::time::Instant::now();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09b-live",
                path: probe,
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Named("pinky".to_string()),
                sudo: true,
            },
            Some("live09b"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id))),
        )
        .await
        .expect("the pinky root edit resolves");
        let elapsed = started.elapsed();
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        println!("root-edit-on-pinky result: {s:?}");
        println!("root-edit-on-pinky elapsed: {elapsed:?}");
        assert!(
            s.contains("nobody is attached to answer it"),
            "the give-up names the real problem: {s:?}"
        );

        let readback = run_in_host_shell(
            crate::shell::registry(),
            owner.clone(),
            crate::host::HostId::Named("pinky".to_string()),
            "cat /tmp/de-09b-probe; rm -f /tmp/de-09b-probe",
            REMOTE_WRITE_TIMEOUT_SECS,
            |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
        )
        .await
        .expect("the pinky readback resolves");
        assert!(
            readback.contains("old-line"),
            "the file is untouched: {readback:?}"
        );

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_default()
                .as_str(),
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
        let cp = crate::config::ssh_dir();
        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", cp.display()),
                "pinky",
            ])
            .status();
        let _ = dir;
    }
}
