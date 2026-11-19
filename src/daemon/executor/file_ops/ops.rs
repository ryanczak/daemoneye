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
        let (rows, cols) = crate::shell::spawn_size();
        Box::pin(async move { crate::shell::spawn_ssh_host(id, &name, rows, cols).await })
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

pub(super) struct RemoteWriteSpec<'a> {
    pub stat_label: String,
    pub host: &'a crate::host::HostId,
    pub cmd: String,
    pub event_name: &'static str,
    pub event_payload: serde_json::Value,
    pub ok_msg: String,
    pub unclear_msg: String,
}

pub(super) async fn execute_remote_host_write<F>(
    spec: RemoteWriteSpec<'_>,
    spawn: WriteSpawn,
    err_formatter: F,
) -> ToolCallOutcome
where
    F: FnOnce(&str) -> String,
{
    let cmd_id = crate::daemon::stats::start_command(&spec.stat_label, "foreground");
    let snap = match super::run_in_host_shell(
        crate::shell::registry(),
        Owner::Chat(owner_key()),
        spec.host.clone(),
        &spec.cmd,
        REMOTE_WRITE_TIMEOUT_SECS,
        spawn,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            crate::daemon::stats::finish_command(cmd_id, 1);
            return ToolCallOutcome::Result(format!("Error: {}", e));
        }
    };
    match super::remote_write_outcome(&snap) {
        Some(Ok(_)) => {
            crate::daemon::stats::finish_command(cmd_id, 0);
            crate::daemon::utils::log_event(spec.event_name, spec.event_payload);
            ToolCallOutcome::Result(spec.ok_msg)
        }
        Some(Err(line)) => {
            crate::daemon::stats::finish_command(cmd_id, 1);
            ToolCallOutcome::Result(err_formatter(&line))
        }
        None => {
            crate::daemon::stats::finish_command(cmd_id, 1);
            ToolCallOutcome::Result(spec.unclear_msg)
        }
    }
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

/// What a sudo edit that failed to run tells the model. Worded for a file
/// operation: it names the file and the host, never a shell id (the edit
/// claims its shell under an owner key, not an id the model could use), and
/// never advises a `shell` parameter `edit_file` does not have.
fn root_edit_error_text(path: &str, host: &str, e: &anyhow::Error) -> String {
    let chain = super::super::error_chain(e);
    if e.chain().any(|c| c.is::<crate::shell::PromptGivenUp>()) {
        format!(
            "Error: the sudo edit of {path} on host {host} did not run: {chain}. \
             The password prompt was cancelled, so {path} was not changed."
        )
    } else if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
        format!(
            "Error: the sudo edit of {path} on host {host} did not run: {chain}. \
             The shell on host {host} exited before the edit ran, so {path} was \
             not changed; retrying the edit opens a new shell there."
        )
    } else {
        format!(
            "Error: the sudo edit of {path} on host {host} did not finish: {chain}. \
             It may still complete on the host — read {path} before retrying, or a \
             retry could apply the edit twice."
        )
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
    match args.host {
        crate::host::HostId::Local => run_local_edit(args, session_id, tx, rx).await,
        crate::host::HostId::Named(name) => {
            run_remote_edit(&args, name, session_id, tx, rx, spawn).await
        }
    }
}

async fn run_remote_edit<W, R>(
    args: &RunEditArgs<'_>,
    name: &str,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
    spawn: WriteSpawn,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let &RunEditArgs {
        id,
        path,
        old_string,
        new_string,
        host,
        sudo,
    } = args;
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
            existing_content: Some(old_string.to_string()),
            new_content: Some(new_string.to_string()),
            dest_path: None,
            sudo,
            always_asked: false,
        },
    )
    .await?;

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    if sudo {
        let cmd_id =
            crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");
        let cmd = super::write::build_remote_sudo_edit_cmd(path, old_string, new_string);
        let max_per_owner = crate::config::current().shells.max_per_owner;
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
                return Ok(ToolCallOutcome::Result(root_edit_error_text(
                    path, name, &e,
                )));
            }
        };
        match super::remote_write_outcome(&text) {
            Some(Ok(_)) => {
                crate::daemon::stats::finish_command(cmd_id, 0);
                crate::daemon::utils::log_event(
                    "file_edit",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name, "sudo": true }),
                );
                return Ok(ToolCallOutcome::Result(format!(
                    "Edited {path} on host {name}."
                )));
            }
            Some(Err(line)) => {
                crate::daemon::stats::finish_command(cmd_id, 1);
                return Ok(ToolCallOutcome::Result(format!(
                    "Error editing {path}: {line}"
                )));
            }
            None => {}
        }
        crate::daemon::stats::finish_command(cmd_id, 1);
        let tail = text.lines().rev().find(|l| !l.trim().is_empty());
        return Ok(ToolCallOutcome::Result(format!(
            "Error: the sudo edit of {path} on host {name} did not run — the password \
             was not accepted or sudo refused it. Output: {}",
            tail.unwrap_or("(no output)")
        )));
    }

    Ok(execute_remote_host_write(
        RemoteWriteSpec {
            stat_label: format!("edit_file {}", path),
            host,
            cmd: super::write::build_remote_edit_cmd(path, old_string, new_string),
            event_name: "file_edit",
            event_payload: serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "path": path,
                "remote_host": name,
            }),
            ok_msg: format!("Edited {path} on host {name}."),
            unclear_msg: format!(
                "Edit command completed but result was unclear. Check {} manually.",
                path
            ),
        },
        spawn,
        |line| format!("Error editing {path}: {line}"),
    )
    .await)
}

async fn run_local_edit<W, R>(
    args: RunEditArgs<'_>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
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
        ..
    } = args;
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

    let is_config = super::is_daemoneye_config(path);
    if is_config && let Err(e) = crate::config::parse_config_text(&std_path, &updated) {
        return Ok(ToolCallOutcome::Result(format!(
            "Error: the edit would leave {} unparseable — the file is unchanged. \
             {e:#}",
            path
        )));
    }

    let mask = |s: &str| {
        if is_config {
            crate::ai::mask_sensitive(s)
        } else {
            s.to_string()
        }
    };
    send_response_split(
        tx,
        Response::EditFilePrompt {
            id: id.to_string(),
            path: path.to_string(),
            operation: "edit".to_string(),
            existing_content: Some(mask(&original)),
            new_content: Some(mask(&updated)),
            dest_path: None,
            sudo: false,
            always_asked: is_config,
        },
    )
    .await?;

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    let cmd_id = crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");
    if let Err(err_msg) = commit_local_edit_atomic(&std_path, path, &updated) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(err_msg));
    }

    crate::daemon::stats::finish_command(cmd_id, 0);
    crate::daemon::utils::log_event(
        "file_edit",
        serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path }),
    );

    let old_lines = old_string.lines().count();
    let new_lines = new_string.lines().count();
    let mut result = format!(
        "Edited {}: replaced {} line(s) with {} line(s).",
        path, old_lines, new_lines
    );
    if is_config {
        result.push_str(&format_edit_reload_report());
    }
    Ok(ToolCallOutcome::Result(result))
}

fn commit_local_edit_atomic(
    std_path: &std::path::Path,
    display_path: &str,
    updated: &str,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    let original_mode = std::fs::metadata(std_path)
        .map(|md| md.permissions().mode())
        .map_err(|e| format!("Error reading {}: {}", display_path, e))?;
    let original_owner = std::fs::metadata(std_path)
        .map(|md| (md.uid(), md.gid()))
        .unwrap_or((u32::MAX, u32::MAX));
    let tmp_path = std_path.with_extension("de_tmp");
    std::fs::write(&tmp_path, updated).map_err(|e| format!("Error writing temp file: {}", e))?;
    if let Err(e) =
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(original_mode))
    {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Error preserving the file's permissions: {}", e));
    }
    let _ = std::os::unix::fs::chown(&tmp_path, Some(original_owner.0), Some(original_owner.1));
    if let Err(e) = std::fs::rename(&tmp_path, std_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Error committing edit: {}", e));
    }
    Ok(())
}

/// Run the phase-19 reload and render its outcome the way `daemoneye
/// reload` does. The formatting seam takes the error only: the applied /
/// restart-required lists come out of [`crate::cli::commands::format_reload`]
/// directly, so the wording stays in one place.
fn format_edit_reload_report() -> String {
    match crate::config::daemon_reload() {
        Ok((applied, restart_required)) => format!(
            "\n\nReload: {}",
            crate::cli::commands::format_reload(&applied, &restart_required)
        ),
        Err(e) => format!(
            "\n\nThe write succeeded, but the reload refused it: {e:#}. \
             Restart the daemon (or run `daemoneye reload`) to pick it up."
        ),
    }
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
                    always_asked: false,
                },
            )
            .await?;

            if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
                return Ok(outcome);
            }
            return Ok(execute_remote_host_write(
                RemoteWriteSpec {
                    stat_label: format!("create_file {}", path),
                    host,
                    cmd: build_remote_create_cmd(path, content),
                    event_name: "file_create",
                    event_payload: serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "path": path,
                        "remote_host": name,
                    }),
                    ok_msg: format!("Created {path} on host {name}."),
                    unclear_msg: format!(
                        "Create command completed but result was unclear. Check {} manually.",
                        path
                    ),
                },
                spawn,
                |line| format!("Error creating {path}: {line}"),
            )
            .await);
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
            always_asked: false,
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

#[path = "ops_copy.rs"]
mod copy;
#[path = "ops_delete.rs"]
mod delete;
#[cfg(test)]
use copy::copy_tmp_path;
pub(super) use copy::run_copy;
#[cfg(test)]
use copy::run_copy_with_spawn;
pub(super) use delete::run_delete;
#[cfg(test)]
use delete::run_delete_with_spawn;

#[cfg(test)]
#[path = "ops_config_tests.rs"]
mod config_tests;
#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
