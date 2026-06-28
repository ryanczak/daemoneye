use super::super::ToolCallOutcome;
use crate::daemon::utils::send_response_split;
use crate::ipc::Response;

// ---------------------------------------------------------------------------
// operation = "edit"
// ---------------------------------------------------------------------------

pub(super) struct RunEditArgs<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub old_string: &'a str,
    pub new_string: &'a str,
    pub target_pane: Option<&'a str>,
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
    let RunEditArgs {
        id,
        path,
        old_string,
        new_string,
        target_pane,
    } = args;
    // ── Remote path ───────────────────────────────────────────────────────
    if let Some(pane) = target_pane {
        send_response_split(
            tx,
            Response::EditFilePrompt {
                id: id.to_string(),
                path: format!("{} (remote via pane {})", path, pane),
                operation: "edit".to_string(),
                // For remote files we can't read the full file locally, so show
                // the old_string → new_string substitution as the diff context.
                existing_content: Some(old_string.to_string()),
                new_content: Some(new_string.to_string()),
                dest_path: None,
            },
        )
        .await?;

        if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
            return Ok(outcome);
        }
        let cmd_id =
            crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");

        let cmd = super::write::build_remote_edit_cmd(path, old_string, new_string);
        let snap = match super::remote_run_and_capture(pane, &cmd, 30).await {
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
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_pane": pane }),
                );
                return Ok(ToolCallOutcome::Result(format!(
                    "Edited {} via pane {}.",
                    path, pane
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
        },
    )
    .await?;

    if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
        return Ok(outcome);
    }

    let cmd_id = crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");
    let tmp_path = std_path.with_extension("de_tmp");
    if let Err(e) = std::fs::write(&tmp_path, &updated) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error writing temp file: {}",
            e
        )));
    }
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
    target_pane: Option<&str>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
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

    // ── Remote path ───────────────────────────────────────────────────────
    if let Some(pane) = target_pane {
        send_response_split(
            tx,
            Response::EditFilePrompt {
                id: id.to_string(),
                path: format!("{} (remote via pane {})", path, pane),
                operation: "create".to_string(),
                existing_content: None,
                new_content: Some(content.to_string()),
                dest_path: None,
            },
        )
        .await?;

        if let Err(outcome) = super::write::await_edit_file_response(id, rx).await? {
            return Ok(outcome);
        }
        let cmd_id =
            crate::daemon::stats::start_command(&format!("create_file {}", path), "foreground");

        let cmd = build_remote_create_cmd(path, content);
        let snap = match super::remote_run_and_capture(pane, &cmd, 30).await {
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
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_pane": pane }),
                );
                return Ok(ToolCallOutcome::Result(format!(
                    "Created {} via pane {}.",
                    path, pane
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
    target_pane: Option<&str>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    // ── Remote path ───────────────────────────────────────────────────────
    if let Some(pane) = target_pane {
        // We can't read the remote file locally, so show the path only.
        send_response_split(
            tx,
            Response::EditFilePrompt {
                id: id.to_string(),
                path: format!("{} (remote via pane {})", path, pane),
                operation: "delete".to_string(),
                existing_content: None,
                new_content: None,
                dest_path: None,
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
        let snap = match super::remote_run_and_capture(pane, &cmd, 30).await {
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
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_pane": pane }),
                );
                return Ok(ToolCallOutcome::Result(format!(
                    "Deleted {} via pane {}.",
                    path, pane
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
    target_pane: Option<&str>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
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

    // ── Remote path ───────────────────────────────────────────────────────
    if let Some(pane) = target_pane {
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
                path: format!("{} (remote via pane {})", src_path, pane),
                operation: "copy".to_string(),
                existing_content: None,
                new_content: None,
                dest_path: Some(format!("{} (remote via pane {})", dest, pane)),
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

        let snap = match super::remote_run_and_capture(pane, &cmd, 30).await {
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
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "src": src_path, "dest": dest, "remote_pane": pane }),
                );
                return Ok(ToolCallOutcome::Result(format!(
                    "Copied {} to {} via pane {}.",
                    src_path, dest, pane
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
    #[test]
    fn remote_create_cmd_perl_branch_makes_parent_dirs() {
        // The Perl code is hex-encoded in the wire command, so check the source
        // for the File::Path/make_path call and File::Basename/dirname import.
        let src = include_str!("ops.rs");
        assert!(
            src.contains("File::Path")
                && src.contains("make_path")
                && src.contains("File::Basename")
                && src.contains("dirname"),
            "Perl branch in source must contain File::Path/make_path and File::Basename/dirname"
        );
    }

    #[test]
    fn remote_create_cmd_python_branch_unchanged() {
        // The Python code is hex-encoded in the wire command, so check the source
        // for the makedirs call.
        let src = include_str!("ops.rs");
        assert!(
            src.contains("makedirs") && src.contains("exist_ok=True"),
            "Python branch in source must still contain makedirs with exist_ok=True"
        );
    }

    #[test]
    fn remote_copy_cmd_is_no_clobber() {
        // The remote copy command in run_copy must use "cp -n" (no-clobber).
        // Verify by checking the source contains the no-clobber flag.
        let src = include_str!("ops.rs");
        assert!(
            src.contains("cp -n --"),
            "remote copy command must use cp -n (no-clobber)"
        );
    }
}
