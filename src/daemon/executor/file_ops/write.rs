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
    pub target_pane: Option<&'a str>,
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

    match operation {
        "create" => {
            super::ops::run_create(id, path, content, target_pane, session_id, tx, rx).await
        }
        "delete" => super::ops::run_delete(id, path, target_pane, session_id, tx, rx).await,
        "copy" => super::ops::run_copy(id, path, dest_path, target_pane, session_id, tx, rx).await,
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
            super::ops::run_edit(id, path, old, new, target_pane, session_id, tx, rx).await
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
         t=p+'.de_tmp'\n\
         open(t,'w').write(c.replace(o,n,1))\n\
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
         $c=~s/\\Q$o\\E/$n/;\n\
         my $t=\"$p.de_tmp\";\n\
         open(my $g,'>',$t) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print $g $c;close $g;\n\
         rename($t,$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print \"DE_OK: Edited $p\\n\";\n"
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
