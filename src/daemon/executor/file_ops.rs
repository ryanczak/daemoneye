use super::{GhostCtx, ToolCallOutcome, USER_PROMPT_TIMEOUT};
use crate::ai::mask_sensitive;
use crate::daemon::session::BUFFER_COUNTER;
use crate::daemon::utils::get_pane_remote_host;
use crate::daemon::utils::send_response_split;
use crate::ipc::{Request, Response};
use crate::tmux;

pub(super) struct EditArgs<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub operation: &'a str,
    pub old_string: Option<&'a str>,
    pub new_string: Option<&'a str>,
    pub content: Option<&'a str>,
    pub dest_path: Option<&'a str>,
    pub target_pane: Option<&'a str>,
}
use std::time::Duration;

// ---------------------------------------------------------------------------
// Remote-pane helpers for read_file / edit_file
// ---------------------------------------------------------------------------

/// Hex-encode a string (no external crate required).
fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

/// Shell-escape a single-quoted argument by replacing `'` with `'\''`.
fn sq_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Extract lines between a unique start marker and end marker from pane output.
fn extract_marked(snap: &str, start: &str, end: &str) -> Option<String> {
    let lines: Vec<&str> = snap.lines().collect();
    let s_idx = lines.iter().position(|l| l.contains(start))?;
    let e_idx = lines.iter().rposition(|l| l.contains(end))?;
    if e_idx <= s_idx {
        return None;
    }
    Some(lines[s_idx + 1..e_idx].join("\n"))
}

/// Send a command to a pane and poll until a completion marker appears.
async fn remote_run_and_capture(
    pane_id: &str,
    cmd: &str,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    tmux::send_keys(pane_id, cmd)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("Timed out waiting for remote command in pane {}", pane_id);
        }
        let snap = tmux::capture_pane(pane_id, 600).unwrap_or_default();
        if snap.contains("__DE_DONE__") {
            return Ok(snap);
        }
    }
}

/// Build the shell command to read `path` from a remote pane with markers.
fn build_remote_read_cmd(path: &str, start: usize, end: usize, pattern: Option<&str>) -> String {
    let safe_path = sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", sq_escape(p)))
        .unwrap_or_default();
    format!(
        "echo '__DE_S__'; sed -n '{},{}p' '{}' 2>&1{}; echo '__DE_E__'; echo '__DE_DONE__'",
        start, end, safe_path, grep_part
    )
}

/// Build the shell command to read `path` through the tmux buffer system (no scrollback cap).
fn build_local_buffer_read_cmd(
    path: &str,
    start: usize,
    end: usize,
    pattern: Option<&str>,
    buf_name: &str,
) -> String {
    let safe_path = sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", sq_escape(p)))
        .unwrap_or_default();
    format!(
        "sed -n '{},{}p' '{}'{}  | tmux load-buffer -b '{}' -; echo '__DE_DONE__'",
        start, end, safe_path, grep_part, buf_name
    )
}

/// Run a read-file command in a LOCAL target pane using `load-buffer`/`save-buffer`.
async fn local_read_via_buffer(
    pane_id: &str,
    path: &str,
    start: usize,
    end: usize,
    pattern: Option<&str>,
) -> anyhow::Result<String> {
    let idx = BUFFER_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let buf_name = format!("de-rb-{}", idx);
    let cmd = build_local_buffer_read_cmd(path, start, end, pattern, &buf_name);

    tmux::send_keys(pane_id, &cmd)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if tokio::time::Instant::now() > deadline {
            let _ = std::process::Command::new("tmux")
                .args(["delete-buffer", "-b", &buf_name])
                .output();
            anyhow::bail!("Timed out waiting for buffer load in pane {}", pane_id);
        }
        let snap = tmux::capture_pane(pane_id, 5).unwrap_or_default();
        if snap.contains("__DE_DONE__") {
            break;
        }
    }

    let out = std::process::Command::new("tmux")
        .args(["save-buffer", "-b", &buf_name, "-"])
        .output()?;
    let _ = std::process::Command::new("tmux")
        .args(["delete-buffer", "-b", &buf_name])
        .output();

    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Build the shell command that runs a Python3-then-Perl atomic replacement in a remote pane.
fn build_remote_edit_cmd(path: &str, old_string: &str, new_string: &str) -> String {
    let path_hex = to_hex(path);
    let old_hex = to_hex(old_string);
    let new_hex = to_hex(new_string);

    let py = format!(
        "import os,sys\n\
         p=bytes.fromhex('{path_hex}').decode()\n\
         o=bytes.fromhex('{old_hex}').decode()\n\
         n=bytes.fromhex('{new_hex}').decode()\n\
         c=open(p).read()\n\
         cnt=c.count(o)\n\
         if cnt==0: print('DE_ERROR: old_string not found in '+p); sys.exit(1)\n\
         if cnt>1: print('DE_ERROR: old_string appears '+str(cnt)+' times in '+p); sys.exit(1)\n\
         t=p+'.de_tmp'\n\
         open(t,'w').write(c.replace(o,n,1))\n\
         os.rename(t,p)\n\
         print('DE_OK: Edited '+p)\n"
    );
    let py_hex = to_hex(&py);

    let pl = format!(
        "my $p=pack('H*','{path_hex}');\n\
         my $o=pack('H*','{old_hex}');\n\
         my $n=pack('H*','{new_hex}');\n\
         open(my $f,'<',$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         my $c=do{{local $/;<$f>}};close $f;\n\
         my @m=($c=~/\\Q$o\\E/g);\n\
         if(!@m){{print \"DE_ERROR: not found\\n\";exit 1}}\n\
         if(@m>1){{print \"DE_ERROR: \".scalar(@m).\" matches\\n\";exit 1}}\n\
         $c=~s/\\Q$o\\E/$n/;\n\
         my $t=\"$p.de_tmp\";\n\
         open(my $g,'>',$t) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print $g $c;close $g;\n\
         rename($t,$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print \"DE_OK: Edited $p\\n\";\n"
    );
    let pl_hex = to_hex(&pl);

    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\" 2>&1; \
         else \
            perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))' 2>&1; \
         fi; echo '__DE_DONE__'"
    )
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub(super) async fn run_read_file(
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<&str>,
    target_pane: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    if path.contains("..") {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain '..'.".to_string(),
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Ok(ToolCallOutcome::Result(
            "Error: path must be absolute (e.g. /var/log/syslog).".to_string(),
        ));
    }

    {
        let de_dir = crate::config::config_dir();
        let pane_logs = crate::config::pane_logs_dir();
        let candidate =
            std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
        if candidate.starts_with(&de_dir) && !candidate.starts_with(&pane_logs) {
            return Ok(ToolCallOutcome::Result(
                "Error: read_file cannot access the daemoneye configuration \
                 directory. Use the dedicated tools (read_script, read_runbook, \
                 read_memory, list_memories, etc.) instead. \
                 Exception: pane log archives under var/log/panes/ are readable."
                    .to_string(),
            ));
        }
    }

    const MAX_LINES: usize = 500;
    const DEFAULT_LINES: usize = 200;
    let limit_n = match limit {
        Some(n) if n > 0 => (n as usize).min(MAX_LINES),
        _ => DEFAULT_LINES,
    };
    let offset_n = offset.map(|o| (o as usize).saturating_sub(1)).unwrap_or(0);

    // ── Target-pane path: run sed/grep in target_pane ─────────────────────
    if let Some(pane) = target_pane {
        let start = offset_n + 1;
        let end = offset_n + limit_n;

        let (content, is_remote) = if get_pane_remote_host(pane).is_none() {
            let raw = match local_read_via_buffer(pane, path, start, end, pattern).await {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
            };
            (raw, false)
        } else {
            let cmd = build_remote_read_cmd(path, start, end, pattern);
            let snap = match remote_run_and_capture(pane, &cmd, 30).await {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
            };
            let extracted =
                extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap_or_else(|| snap.clone());
            (extracted, true)
        };

        if content.trim().is_empty() {
            return Ok(ToolCallOutcome::Result(format!(
                "{}: no output (file may be empty or lines out of range)",
                path
            )));
        }
        let body = mask_sensitive(content.trim_end());
        let label = if is_remote {
            if pattern.is_some() {
                format!("{} (remote grep, lines {}-{}):\n{}", path, start, end, body)
            } else {
                format!("{} (remote, lines {}-{}):\n{}", path, start, end, body)
            }
        } else if pattern.is_some() {
            format!("{} (local grep, lines {}-{}):\n{}", path, start, end, body)
        } else {
            format!("{} (local pane, lines {}-{}):\n{}", path, start, end, body)
        };
        return Ok(ToolCallOutcome::Result(label));
    }

    // ── Local path: read directly from daemon-host filesystem ─────────────
    let real_path = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let raw = match std::fs::read_to_string(&real_path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error reading {}: {}",
                path, e
            )));
        }
    };

    let all_lines: Vec<&str> = raw.lines().collect();
    let total = all_lines.len();
    let sliced = &all_lines[offset_n.min(total)..];
    let limited: Vec<&str> = sliced.iter().take(limit_n).copied().collect();
    let limited_len = limited.len();

    let filtered: Vec<&str> = if let Some(pat) = pattern {
        match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
            Ok(re) => limited.into_iter().filter(|l| re.is_match(l)).collect(),
            Err(e) => {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid pattern regex: {}",
                    e
                )));
            }
        }
    } else {
        limited
    };

    if filtered.is_empty() {
        return Ok(ToolCallOutcome::Result(format!(
            "{}: no lines matched (total {} lines in file)",
            path, total
        )));
    }

    let body = mask_sensitive(&filtered.join("\n"));
    if pattern.is_some() {
        Ok(ToolCallOutcome::Result(format!(
            "{} ({} matching lines, searched lines {}-{} of {}):\n{}",
            path,
            filtered.len(),
            offset_n + 1,
            (offset_n + limited_len).min(total),
            total,
            body
        )))
    } else {
        Ok(ToolCallOutcome::Result(format!(
            "{} (lines {}-{} of {}):\n{}",
            path,
            offset_n + 1,
            (offset_n + filtered.len()).min(total),
            total,
            body
        )))
    }
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

pub(super) async fn run_edit_file<W, R>(
    args: EditArgs<'_>,
    session_id: Option<&str>,
    ghost_ctx: GhostCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let EditArgs {
        id,
        path,
        operation,
        old_string,
        new_string,
        content,
        dest_path,
        target_pane,
    } = args;
    let GhostCtx { is_ghost, .. } = ghost_ctx;

    // ── Common validation ─────────────────────────────────────────────────
    if path.contains("..") {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain '..'.".to_string(),
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Ok(ToolCallOutcome::Result(
            "Error: path must be absolute.".to_string(),
        ));
    }
    {
        let de_dir = crate::config::config_dir();
        let candidate =
            std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
        if candidate.starts_with(&de_dir) {
            return Ok(ToolCallOutcome::Result(
                "Error: edit_file cannot access the daemoneye configuration \
                 directory. Use the dedicated tools (write_script, write_runbook, \
                 add_memory, etc.) instead."
                    .to_string(),
            ));
        }
    }
    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: file operations require user approval and cannot run in a Ghost Shell."
                .to_string(),
        ));
    }

    match operation {
        "create" => run_create(id, path, content, target_pane, session_id, tx, rx).await,
        "delete" => run_delete(id, path, target_pane, session_id, tx, rx).await,
        "copy" => run_copy(id, path, dest_path, target_pane, session_id, tx, rx).await,
        _ => {
            // "edit" (default) and any unrecognised value fall through here.
            let old =
                match old_string {
                    Some(s) if !s.is_empty() => s,
                    _ => return Ok(ToolCallOutcome::Result(
                        "Error: old_string is required and cannot be empty for operation=\"edit\"."
                            .to_string(),
                    )),
                };
            let new = new_string.unwrap_or("");
            run_edit(id, path, old, new, target_pane, session_id, tx, rx).await
        }
    }
}

// ---------------------------------------------------------------------------
// await_edit_file_response — shared response-await helper
// ---------------------------------------------------------------------------

async fn await_edit_file_response<R>(
    id: &str,
    rx: &mut R,
) -> anyhow::Result<Result<bool, ToolCallOutcome>>
where
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::EditFileResponse {
                id: resp_id,
                approved,
                user_message,
            }) if resp_id == id => {
                if let Some(msg) = user_message {
                    return Ok(Err(ToolCallOutcome::UserMessage(msg)));
                }
                if !approved {
                    return Ok(Err(ToolCallOutcome::Result(
                        "User denied execution".to_string(),
                    )));
                }
                Ok(Ok(true))
            }
            _ => Ok(Err(ToolCallOutcome::Result(
                "User denied execution".to_string(),
            ))),
        },
        _ => Ok(Err(ToolCallOutcome::Result(
            "User denied execution".to_string(),
        ))),
    }
}

// ---------------------------------------------------------------------------
// operation = "edit"
// ---------------------------------------------------------------------------

async fn run_edit<W, R>(
    id: &str,
    path: &str,
    old_string: &str,
    new_string: &str,
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

        if let Err(outcome) = await_edit_file_response(id, rx).await? {
            return Ok(outcome);
        }
        let cmd_id =
            crate::daemon::stats::start_command(&format!("edit_file {}", path), "foreground");

        let cmd = build_remote_edit_cmd(path, old_string, new_string);
        let snap = match remote_run_and_capture(pane, &cmd, 30).await {
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

    if let Err(outcome) = await_edit_file_response(id, rx).await? {
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
    let path_hex = to_hex(path);
    let content_hex = to_hex(content);

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
    let py_hex = to_hex(&py);

    let pl = format!(
        "my $p=pack('H*','{path_hex}');\n\
         my $c=pack('H*','{content_hex}');\n\
         if(-e $p){{print \"DE_ERROR: file already exists\\n\";exit 1}}\n\
         my $t=\"$p.de_tmp\";\n\
         open(my $f,'>',$t) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print $f $c;close $f;\n\
         rename($t,$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print \"DE_OK: Created $p\\n\";\n"
    );
    let pl_hex = to_hex(&pl);

    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\" 2>&1; \
         else \
            perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))' 2>&1; \
         fi; echo '__DE_DONE__'"
    )
}

async fn run_create<W, R>(
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

        if let Err(outcome) = await_edit_file_response(id, rx).await? {
            return Ok(outcome);
        }
        let cmd_id =
            crate::daemon::stats::start_command(&format!("create_file {}", path), "foreground");

        let cmd = build_remote_create_cmd(path, content);
        let snap = match remote_run_and_capture(pane, &cmd, 30).await {
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

    if let Err(outcome) = await_edit_file_response(id, rx).await? {
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

async fn run_delete<W, R>(
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

        if let Err(outcome) = await_edit_file_response(id, rx).await? {
            return Ok(outcome);
        }
        let cmd_id =
            crate::daemon::stats::start_command(&format!("delete_file {}", path), "foreground");

        let safe_path = sq_escape(path);
        let cmd = format!(
            "if [ -e '{safe_path}' ]; then rm -- '{safe_path}' && echo 'DE_OK: Deleted {safe_path}'; \
             else echo 'DE_ERROR: file not found: {safe_path}'; fi; echo '__DE_DONE__'"
        );
        let snap = match remote_run_and_capture(pane, &cmd, 30).await {
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

    if let Err(outcome) = await_edit_file_response(id, rx).await? {
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

async fn run_copy<W, R>(
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
        let safe_src = sq_escape(src_path);
        let safe_dst = sq_escape(dest);
        let cmd = format!(
            "if [ ! -e '{safe_src}' ]; then echo 'DE_ERROR: source not found: {safe_src}'; \
             elif [ -e '{safe_dst}' ]; then echo 'DE_ERROR: destination already exists: {safe_dst}'; \
             else cp -- '{safe_src}' '{safe_dst}' && echo 'DE_OK: Copied {safe_src} to {safe_dst}' \
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

        if let Err(outcome) = await_edit_file_response(id, rx).await? {
            return Ok(outcome);
        }
        let cmd_id = crate::daemon::stats::start_command(
            &format!("copy_file {} {}", src_path, dest),
            "foreground",
        );

        let snap = match remote_run_and_capture(pane, &cmd, 30).await {
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

    if let Err(outcome) = await_edit_file_response(id, rx).await? {
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
    use crate::util::UnpoisonExt;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("de_fops_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
    }
    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
        let old = env::var("HOME").ok();
        unsafe {
            env::set_var("HOME", &tmp.0);
        }
        f();
        match old {
            Some(v) => unsafe {
                env::set_var("HOME", v);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
    }

    fn simulate_read_file(lines: &[&str]) -> (TmpHome, std::path::PathBuf) {
        let tmp = TmpHome::new();
        let path = tmp.0.join("test_file.txt");
        std::fs::write(&path, lines.join("\n")).unwrap();
        (tmp, path)
    }

    #[tokio::test]
    async fn read_file_default_reads_from_start() {
        let (tmp, path) = simulate_read_file(&["line1", "line2", "line3"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, None, None, None)
            .await
            .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("line1"));
        assert!(s.contains("line3"));
    }

    #[tokio::test]
    async fn read_file_offset_skips_lines() {
        // Use zero-padded names to avoid "line1" being a substring of "line10".
        let lines: Vec<String> = (1..=10).map(|i| format!("line{:02}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), Some(5), None, None, None)
            .await
            .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(!s.contains("line01"), "offset should skip line01");
        assert!(s.contains("line05"), "should start from line05");
    }

    #[tokio::test]
    async fn read_file_limit_caps_output() {
        let lines: Vec<String> = (1..=20).map(|i| format!("line{}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, Some(3), None, None)
            .await
            .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("line1"));
        assert!(s.contains("line3"));
        assert!(!s.contains("line4"), "limit=3 should not include line4");
    }

    #[tokio::test]
    async fn read_file_limit_capped_at_max() {
        let lines: Vec<String> = (1..=600).map(|i| format!("line{}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, Some(600), None, None)
            .await
            .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        // MAX_LINES = 500; line501 should not appear
        assert!(!s.contains("line501"), "should be capped at 500 lines");
    }

    #[tokio::test]
    async fn read_file_pattern_grep_mode_header() {
        let (tmp, path) = simulate_read_file(&["apple", "banana", "cherry"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, None, Some("banana"), None)
            .await
            .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("matching lines"));
        assert!(s.contains("banana"));
    }

    #[tokio::test]
    async fn read_file_pattern_no_match_returns_message() {
        let (tmp, path) = simulate_read_file(&["apple", "banana"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(
            path.to_str().unwrap(),
            None,
            None,
            Some("xyzzy_not_found"),
            None,
        )
        .await
        .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("no lines matched"));
    }

    #[tokio::test]
    async fn read_file_offset_beyond_eof_returns_empty() {
        let (tmp, path) = simulate_read_file(&["line1", "line2"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), Some(1000), None, None, None)
            .await
            .unwrap();
        let super::ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("no lines matched"));
    }
}
