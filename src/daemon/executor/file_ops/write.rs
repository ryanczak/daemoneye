use super::super::{GhostCtx, ToolCallOutcome, USER_PROMPT_TIMEOUT};
use crate::ipc::Request;

pub struct EditArgs<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub operation: &'a str,
    pub old_string: Option<&'a str>,
    pub new_string: Option<&'a str>,
    pub content: Option<&'a str>,
    pub dest_path: Option<&'a str>,
    pub host: Option<&'a str>,
    pub sudo: bool,
}

/// Resolve an `edit_file` host argument the way `read_file` does: `None` /
/// `"local"` is the daemon host; a name is refused up front if it can never
/// be one (`HostId::parse`'s own message). An *unconfigured* well-formed name
/// is deliberately not refused here — it is phase-06c's `new_host` case and
/// `run_in_host_shell` resolves it to its bare ssh destination on approval.
pub(super) fn resolve_edit_host(host: Option<&str>) -> Result<crate::host::HostId, String> {
    match host {
        None => Ok(crate::host::HostId::Local),
        Some(name) => crate::host::HostId::parse(name),
    }
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

pub async fn run_edit_file<W, R>(
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
        host,
        sudo,
    } = args;
    let GhostCtx { is_ghost, .. } = ghost_ctx;

    // ── Common validation ─────────────────────────────────────────────────
    if path.contains("..") {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain '..'.".to_string(),
        ));
    }
    if super::contains_control(path) || dest_path.is_some_and(super::contains_control) {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain control characters.".to_string(),
        ));
    }
    if super::is_blocked_secret_path(path) || dest_path.is_some_and(super::is_blocked_secret_path) {
        return Ok(ToolCallOutcome::Result(
            "Error: edit_file cannot access well-known credential files \
             (~/.ssh, .env, private keys)."
                .to_string(),
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Ok(ToolCallOutcome::Result(
            "Error: path must be absolute.".to_string(),
        ));
    }
    {
        let de_dir = crate::config::config_dir();
        let candidate = super::resolve_path_for_guard(path);
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

    // ── Host resolution ───────────────────────────────────────────────────
    let host_id = match resolve_edit_host(host) {
        Ok(id) => id,
        Err(msg) => return Ok(ToolCallOutcome::Result(format!("Error: {msg}"))),
    };

    // ── sudo scoping refusals, before any prompt ──────────────────────────
    if sudo {
        if operation != "edit" {
            return Ok(ToolCallOutcome::Result(
                "Error: sudo is supported for operation=\"edit\" only.".to_string(),
            ));
        }
        if host_id == crate::host::HostId::Local {
            return Ok(ToolCallOutcome::Result(
                "Error: a sudo edit runs on a configured host — name it with host. \
                 A sudo edit on the daemon host is not supported."
                    .to_string(),
            ));
        }
    }

    match operation {
        "create" => super::ops::run_create(id, path, content, &host_id, session_id, tx, rx).await,
        "delete" => super::ops::run_delete(id, path, &host_id, session_id, tx, rx).await,
        "copy" => super::ops::run_copy(id, path, dest_path, &host_id, session_id, tx, rx).await,
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
            super::ops::run_edit(
                super::ops::RunEditArgs {
                    id,
                    path,
                    old_string: old,
                    new_string: new,
                    host: &host_id,
                    sudo,
                },
                session_id,
                tx,
                rx,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// await_edit_file_response — shared response-await helper
// ---------------------------------------------------------------------------

pub(super) async fn await_edit_file_response<R>(
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
                    crate::daemon::stats::inc_file_edits_denied();
                    return Ok(Err(ToolCallOutcome::UserMessage(msg)));
                }
                if !approved {
                    crate::daemon::stats::inc_file_edits_denied();
                    return Ok(Err(ToolCallOutcome::Result(
                        "User denied execution".to_string(),
                    )));
                }
                crate::daemon::stats::inc_file_edits_approved();
                Ok(Ok(true))
            }
            _ => {
                crate::daemon::stats::inc_file_edits_denied();
                Ok(Err(ToolCallOutcome::Result(
                    "User denied execution".to_string(),
                )))
            }
        },
        _ => {
            crate::daemon::stats::inc_file_edits_denied();
            Ok(Err(ToolCallOutcome::Result(
                "User denied execution".to_string(),
            )))
        }
    }
}

/// Build the shell command that runs a Python3-then-Perl atomic replacement in a remote pane.
pub(super) fn build_remote_edit_cmd(path: &str, old_string: &str, new_string: &str) -> String {
    let (py_hex, pl_hex) = remote_edit_payload_hex(path, old_string, new_string);

    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\" 2>&1; \
         else \
            perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))' 2>&1; \
         fi; echo '__DE_DONE__'"
    )
}

/// The two interpreter payloads a remote edit runs, hex-encoded. Shared by the
/// plain remote edit and the sudo edit so neither copy can drift: a fix to the
/// mode/owner preservation lands in both with one change.
pub(super) fn remote_edit_payload_hex(
    path: &str,
    old_string: &str,
    new_string: &str,
) -> (String, String) {
    let path_hex = super::to_hex(path);
    let old_hex = super::to_hex(old_string);
    let new_hex = super::to_hex(new_string);

    let py = format!(
        "import os,sys\n\
         p=bytes.fromhex('{path_hex}').decode()\n\
         o=bytes.fromhex('{old_hex}').decode()\n\
         n=bytes.fromhex('{new_hex}').decode()\n\
         c=open(p).read()\n\
         cnt=c.count(o)\n\
         if cnt==0: print('DE_ERROR: old_string not found in '+p); sys.exit(1)\n\
         if cnt>1: print('DE_ERROR: old_string appears '+str(cnt)+' times in '+p); sys.exit(1)\n\
         st=os.stat(p)\n\
         t=p+'.de_tmp'\n\
         open(t,'w').write(c.replace(o,n,1))\n\
         os.chmod(t,st.st_mode&0o7777)\n\
         try: os.chown(t,st.st_uid,st.st_gid)\n\
         except OSError: pass\n\
         os.rename(t,p)\n\
         print('DE_OK: Edited '+p)\n"
    );
    let py_hex = super::to_hex(&py);

    let pl = format!(
        "my $p=pack('H*','{path_hex}');\n\
         my $o=pack('H*','{old_hex}');\n\
         my $n=pack('H*','{new_hex}');\n\
         open(my $f,'<',$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         my $c=do{{local $/;<$f>}};close $f;\n\
         my @m=($c=~/\\Q$o\\E/g);\n\
         if(!@m){{print \"DE_ERROR: not found\\n\";exit 1}}\n\
         if(@m>1){{print \"DE_ERROR: \".scalar(@m).\" matches\\n\";exit 1}}\n\
         my @st=stat($p);\n\
         $c=~s/\\Q$o\\E/$n/;\n\
         my $t=\"$p.de_tmp\";\n\
         open(my $g,'>',$t) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print $g $c;close $g;\n\
         chmod($st[2]&07777,$t);\n\
         chown($st[4],$st[5],$t);\n\
         rename($t,$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print \"DE_OK: Edited $p\\n\";\n"
    );
    let pl_hex = super::to_hex(&pl);

    (py_hex, pl_hex)
}

/// The sudo edit's line. `command_has_sudo`'s regex is `(?:^|[;&|])\s*sudo\b`,
/// so the word must sit **after** a `;` — inside the `if`'s branches it is
/// never seen and the run would never prompt. The interpreter is chosen with
/// `set --`, then run under the privilege tool once.
pub(super) fn build_remote_sudo_edit_cmd(path: &str, old_string: &str, new_string: &str) -> String {
    let (py_hex, pl_hex) = remote_edit_payload_hex(path, old_string, new_string);
    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            set -- python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\"; \
         else \
            set -- perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))'; \
         fi; sudo \"$@\" 2>&1; echo '__DE_DONE__'"
    )
}
