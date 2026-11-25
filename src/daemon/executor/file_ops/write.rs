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
        Some(name) => {
            let id = crate::host::HostId::parse(name)?;
            if let Some(msg) = crate::daemon::executor::host_gate::self_host_refusal(
                &id,
                &crate::config::current(),
                &crate::daemon::utils::daemon_hostname(),
            ) {
                return Err(msg);
            }
            Ok(id)
        }
    }
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

fn validate_edit_file_request(
    args: &EditArgs<'_>,
    is_ghost: bool,
) -> Result<crate::host::HostId, String> {
    let &EditArgs {
        path,
        operation,
        dest_path,
        host,
        sudo,
        ..
    } = args;

    if path.contains("..") {
        return Err("Error: path must not contain '..'.".to_string());
    }
    if super::contains_control(path) || dest_path.is_some_and(super::contains_control) {
        return Err("Error: path must not contain control characters.".to_string());
    }
    if super::is_blocked_secret_path(path) || dest_path.is_some_and(super::is_blocked_secret_path) {
        return Err(
            "Error: edit_file cannot access well-known credential files \
             (~/.ssh, .env, private keys)."
                .to_string(),
        );
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err("Error: path must be absolute.".to_string());
    }
    if super::in_daemoneye_dir(path) && !super::is_daemoneye_config(path) {
        return Err(
            "Error: edit_file cannot access the daemoneye configuration \
             directory. Use the dedicated tools (write_script, write_runbook, \
             add_memory, etc.) instead."
                .to_string(),
        );
    }
    if is_ghost {
        return Err(
            "Error: file operations require user approval and cannot run in a Ghost Shell."
                .to_string(),
        );
    }

    let host_id = resolve_edit_host(host).map_err(|msg| format!("Error: {msg}"))?;

    if dest_path.is_some_and(super::in_daemoneye_dir) {
        return Err(
            "Error: edit_file cannot copy into the daemoneye configuration \
             directory."
                .to_string(),
        );
    }
    if super::is_daemoneye_config(path) {
        if operation != "edit" {
            return Err(format!(
                "Error: edit_file operation=\"{operation}\" cannot touch the \
                 daemoneye config — only operation=\"edit\" opens etc/config.toml."
            ));
        }
        if sudo {
            return Err(
                "Error: a config edit runs as the daemon user — sudo is not \
                 supported for etc/config.toml."
                    .to_string(),
            );
        }
        if host_id != crate::host::HostId::Local {
            return Err(
                "Error: the daemoneye config is the daemon host's file — omit \
                 host (or pass \"local\") to edit it."
                    .to_string(),
            );
        }
    }

    if sudo {
        if operation != "edit" {
            return Err("Error: sudo is supported for operation=\"edit\" only.".to_string());
        }
        if host_id == crate::host::HostId::Local {
            return Err(
                "Error: a sudo edit runs on a configured host — name it with host. \
                 A sudo edit on the daemon host is not supported."
                    .to_string(),
            );
        }
    }
    Ok(host_id)
}

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
    let host_id = match validate_edit_file_request(&args, ghost_ctx.is_ghost) {
        Ok(id) => id,
        Err(msg) => return Ok(ToolCallOutcome::Result(msg)),
    };
    let EditArgs {
        id,
        path,
        operation,
        old_string,
        new_string,
        content,
        dest_path,
        sudo,
        ..
    } = args;

    match operation {
        "create" => super::ops::run_create(id, path, content, &host_id, session_id, tx, rx).await,
        "delete" => super::ops::run_delete(id, path, &host_id, session_id, tx, rx).await,
        "copy" => super::ops::run_copy(id, path, dest_path, &host_id, session_id, tx, rx).await,
        _ => {
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
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let read_result = tokio::time::timeout(
        USER_PROMPT_TIMEOUT,
        crate::ipc::read_bounded_line(rx, &mut line, crate::ipc::MAX_IPC_MESSAGE_BYTES),
    )
    .await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let parsed = match read_result {
        Ok(Ok(_)) => serde_json::from_slice::<Request>(&line).ok(),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(anyhow::anyhow!(
                "reply line too large ({e}); refusing the prompt and ending the turn"
            ));
        }
        // A timeout or a read error reads as a denial, as today.
        _ => None,
    };
    match parsed {
        Some(Request::EditFileResponse {
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
        "import os,sys,tempfile\n\
         p=bytes.fromhex('{path_hex}').decode()\n\
         o=bytes.fromhex('{old_hex}').decode()\n\
         n=bytes.fromhex('{new_hex}').decode()\n\
         c=open(p).read()\n\
         cnt=c.count(o)\n\
         if cnt==0: print('DE_ERROR: old_string not found in '+p); sys.exit(1)\n\
         if cnt>1: print('DE_ERROR: old_string appears '+str(cnt)+' times in '+p); sys.exit(1)\n\
         st=os.stat(p)\n\
         fd,t=tempfile.mkstemp(suffix='.de_tmp',prefix='.',dir=os.path.dirname(p) or '.')\n\
         try:\n\
         \x20 f=os.fdopen(fd,'w')\n\
         \x20 f.write(c.replace(o,n,1))\n\
         \x20 f.flush()\n\
         \x20 os.fchmod(fd,st.st_mode&0o7777)\n\
         \x20 try: os.fchown(fd,st.st_uid,st.st_gid)\n\
         \x20 except OSError: pass\n\
         \x20 f.close()\n\
         \x20 os.rename(t,p)\n\
         except BaseException:\n\
         \x20 try: os.unlink(t)\n\
         \x20 except OSError: pass\n\
         \x20 print('DE_ERROR: edit failed'); sys.exit(1)\n\
         print('DE_OK: Edited '+p)\n"
    );
    let py_hex = super::to_hex(&py);

    let pl = format!(
        "use File::Temp qw(tempfile);\n\
         use File::Basename qw(dirname);\n\
         my $p=pack('H*','{path_hex}');\n\
         my $o=pack('H*','{old_hex}');\n\
         my $n=pack('H*','{new_hex}');\n\
         open(my $f,'<',$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         my $c=do{{local $/;<$f>}};close $f;\n\
         my @m=($c=~/\\Q$o\\E/g);\n\
         if(!@m){{print \"DE_ERROR: not found\\n\";exit 1}}\n\
         if(@m>1){{print \"DE_ERROR: \".scalar(@m).\" matches\\n\";exit 1}}\n\
         my @st=stat($p);\n\
         $c=~s/\\Q$o\\E/$n/;\n\
         my $d=dirname($p);\n\
         my ($g,$t)=tempfile('.deXXXXXXXX',DIR=>$d,SUFFIX=>'.de_tmp',UNLINK=>0) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         my $ok=eval{{\n\
         \x20 print $g $c or die;\n\
         \x20 $g->flush or die;\n\
         \x20 chmod($st[2]&07777,$g) or die;\n\
         \x20 eval{{chown($st[4],$st[5],$g)}};\n\
         \x20 close($g) or die;\n\
         \x20 rename($t,$p) or die;\n\
         \x20 1}};\n\
         if(!$ok){{my $e=$@;close($g);unlink($t);print \"DE_ERROR: ${{$e||'edit failed'}}\\n\";exit 1}}\n\
         print \"DE_OK: Edited $p\\n\";\n"
    );
    let pl_hex = super::to_hex(&pl);

    (py_hex, pl_hex)
}

/// The sudo edit's line. `command_has_sudo` sees `sudo` as a command word in
/// any separator-delimited position — at the start, after `;`/`&`/`|` or a
/// newline, inside `$(`, a backtick or `(`, or as the command `xargs` runs —
/// so the `; sudo "$@"` here is always detected. The interpreter is chosen
/// with `set --`, then run under the privilege tool once.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An oversized edit reply is refused within the byte budget: the reader
    /// hands out at most `MAX_IPC_MESSAGE_BYTES + 64 KiB` of a 64 MiB source,
    /// and the result is an `Err` whose text says `too large`.
    #[tokio::test]
    async fn edit_reply_refuses_an_oversized_reply_within_budget() {
        let mut rx = tokio::io::BufReader::new(tokio::io::AsyncReadExt::take(
            tokio::io::repeat(b'a'),
            64 << 20,
        ));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            await_edit_file_response("t-1", &mut rx),
        )
        .await
        .expect("the reply is refused within 10 s, not read to the end");
        let err = match result {
            Err(e) => e,
            Ok(inner) => panic!("an oversized reply is refused, got {inner:?}"),
        };
        assert!(
            err.to_string().contains("too large"),
            "the refusal names the oversized line: {err:#}"
        );
        let handed_out = (64u64 << 20) - rx.get_ref().limit();
        assert!(
            handed_out <= (crate::ipc::MAX_IPC_MESSAGE_BYTES + (64 << 10)) as u64,
            "at most the cap plus 64 KiB is handed out before the refusal: {handed_out}"
        );
    }
}
