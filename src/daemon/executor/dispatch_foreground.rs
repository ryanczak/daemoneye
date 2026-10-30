//! `execute_tool_call`'s `run_terminal_command` foreground arm
//! (`PendingCall::Foreground`), split into its stages: the gates and the
//! approval (`dispatch`), the approved run (`run_approved`), the named-host
//! path (`run_on_named_host`) and the reply (`finish_run`). A child module of
//! `src/daemon/executor/mod.rs`.

use super::*;

/// The gate's verdict for a foreground call: the refusal text to send when
/// the call may not go on, `None` when it may. `config` is the same load the
/// run will use, so a ghost with an unreadable config is refused here
/// instead of the gate reading "sandbox off" out of the defaults — the gate
/// fails closed (M32 phase-01).
fn ghost_foreground_refusal(
    is_ghost: bool,
    config: &anyhow::Result<crate::config::Config>,
) -> Option<String> {
    match config {
        Err(e) => is_ghost.then(|| {
            format!(
                "Error: foreground execution is refused because the configuration \
                 could not be read: {e}. Fix or remove {path}, then retry; nothing \
                 was run.",
                path = crate::config::etc_dir().join("config.toml").display(),
            )
        }),
        Ok(config) => (!ghost_may_run_foreground(is_ghost, config.sandbox.enabled)).then(|| {
            "Foreground execution is not sandboxed and is denied for ghost \
                 shells while the container sandbox is enabled. Re-issue this \
                 command with `wait: false` so it runs in a container."
                .to_string()
        }),
    }
}

/// The approved call, carried whole so each stage keeps a short parameter
/// list. Every field is a borrow or a copy, so stages pass it by value.
#[derive(Clone, Copy)]
struct ApprovedRun<'a> {
    id: &'a String,
    cmd: &'a String,
    shell: &'a Option<String>,
    timeout_secs: &'a Option<u64>,
    interactive: &'a bool,
    cmd_id: usize,
    session_id: Option<&'a str>,
    max_per_owner: u32,
}

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    ctx: SessionCtx<'_>,
    ghost_policy: &Option<GhostPolicy>,
    is_ghost: bool,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let session_id = ctx.session_id;
    match call {
        PendingCall::Foreground {
            id,
            cmd,
            shell,
            host,
            timeout_secs,
            interactive,
            ..
        } => {
            // Ghost gate — a ghost has no unsandboxed door out (M19 phase-03).
            // One load per call feeds both this gate and the approved run's
            // `max_per_owner`; an unreadable config fails the gate **closed**
            // for a ghost and reads as the defaults for a non-ghost
            // (M32 phase-01).
            let config = crate::config::Config::load();
            if let Some(r) = ghost_foreground_refusal(is_ghost, &config) {
                return Ok(ToolCallOutcome::Result(r));
            }

            // The host is parsed **before** the approval prompt: a malformed
            // name is refused here, without asking — a name that can never be
            // a host is not a decision to put to the user. The parsed id feeds
            // both the prompt's classification and the run, so the name is
            // parsed once. An *unconfigured* well-formed name is not refused
            // here; it classifies as `new_host` and is asked (06b), then
            // connects on approval using the bare name (06c).
            let host = match resolve_foreground_host(host.as_deref()) {
                Ok(host) => host,
                Err(msg) => {
                    let msg = format!("Error: {msg}");
                    send_response_split(
                        tx,
                        Response::ToolResult {
                            tool_call_id: id.to_string(),
                            output: msg.clone(),
                        },
                    )
                    .await?;
                    return Ok(ToolCallOutcome::Result(msg));
                }
            };
            // A runbook's `hosts:` list is a boundary, not advice: a ghost
            // reaching a host its runbook did not name is refused here, with
            // the allowed set named so the model can correct itself.
            if let Some(policy) = ghost_policy.as_ref()
                && !policy.may_use_host(&host)
            {
                let msg = ghost_host_refusal(policy, &host);
                crate::daemon::utils::log_event(
                    "ghost_host_refused",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "run_terminal_command",
                        "requested_host": host.as_str(),
                        "allowed_hosts": policy.hosts,
                    }),
                );
                send_response_split(
                    tx,
                    Response::ToolResult {
                        tool_call_id: id.to_string(),
                        output: msg.clone(),
                    },
                )
                .await?;
                return Ok(ToolCallOutcome::Result(msg));
            }
            // The PTY shell substrate, the only substrate. A ghost session
            // falls through to `refuse_foreground`, which refuses.
            if !is_ghost && session_id.is_some() {
                match prompt_and_await_approval(
                    ApprovalRequest {
                        id,
                        cmd,
                        background: false,
                        target_pane_hint: None,
                        host: Some(host.as_str()),
                        user_shell: shell
                            .as_deref()
                            .filter(|s| crate::shell::is_user_shell(crate::shell::registry(), s)),
                        interactive: is_launch(*interactive, cmd),
                    },
                    session_id,
                    ghost_policy.as_ref(),
                    tx,
                    rx,
                )
                .await?
                {
                    Err(outcome) => return Ok(outcome),
                    Ok(cmd_id) => {
                        let run = ApprovedRun {
                            id,
                            cmd,
                            shell,
                            timeout_secs,
                            interactive,
                            cmd_id,
                            session_id,
                            max_per_owner: config
                                .as_ref()
                                .map(|c| c.shells.max_per_owner)
                                .unwrap_or_else(|_| {
                                    crate::config::Config::default().shells.max_per_owner
                                }),
                        };
                        let outcome = run_approved(run, host, tx, rx).await?;
                        return Ok(ToolCallOutcome::Result(outcome));
                    }
                }
            }

            foreground::refuse_foreground(
                foreground::FgArgs { id, cmd },
                ctx,
                GhostCtx {
                    policy: ghost_policy.as_ref(),
                    is_ghost,
                },
                tx,
                rx,
            )
            .await
        }

        other => anyhow::bail!(
            "{} is not run_terminal_command's foreground arm",
            other.tool_name()
        ),
    }
}

/// The approved run: the owner, the post-approval host lookup, then the
/// local or named host's run. Returns the tool result's text.
async fn run_approved<W, R>(
    a: ApprovedRun<'_>,
    host: crate::host::HostId,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let ApprovedRun {
        id,
        cmd,
        shell,
        timeout_secs,
        interactive,
        session_id,
        max_per_owner,
        ..
    } = a;
    let owner = Owner::Chat(session_id.unwrap_or_default().to_string());
    // `host` is already parsed above, before the prompt. A named host's
    // permissive lookup runs once, after approval, in `run_on_named_host`.
    // The two arms spawn different programs, so their closure
    // types differ and each calls `run_in_shell_with_prompts`
    // itself (the same split `open_shell` makes). An
    // `interactive: true` call takes `run_interactive_in_shell`
    // instead, after the same approval: the launch path waits
    // only for the screen to settle, and `wait: false` on it
    // is ignored (03e).
    let outcome = match &host {
        crate::host::HostId::Local => {
            let run = if *interactive {
                shell_exec::run_interactive_in_shell(
                    crate::shell::registry(),
                    owner.clone(),
                    cmd,
                    max_per_owner,
                    shell.as_deref(),
                    crate::host::HostId::Local,
                    |id| {
                        let (rows, cols) = crate::shell::spawn_size();
                        crate::shell::spawn_detached_host(id, &[], rows, cols)
                    },
                )
                .await
            } else {
                shell_exec::run_in_shell_with_prompts(
                    crate::shell::registry(),
                    owner.clone(),
                    cmd,
                    max_per_owner,
                    shell.as_deref(),
                    *timeout_secs,
                    id.as_str(),
                    tx,
                    rx,
                    crate::host::HostId::Local,
                    |id| {
                        let (rows, cols) = crate::shell::spawn_size();
                        crate::shell::spawn_detached_host(id, &[], rows, cols)
                    },
                )
                .await
            };
            finish_run(run, owner, a, tx).await?
        }
        crate::host::HostId::Named(name) => {
            run_on_named_host(a, name, &host, owner, tx, rx).await?
        }
    };
    Ok(outcome)
}

/// A named host's run: the permissive lookup, the connect, then the run
/// over ssh. Returns the tool result's text.
async fn run_on_named_host<W, R>(
    a: ApprovedRun<'_>,
    name: &str,
    host: &crate::host::HostId,
    owner: Owner,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let ApprovedRun {
        id,
        cmd,
        shell,
        timeout_secs,
        interactive,
        cmd_id,
        session_id,
        max_per_owner,
    } = a;
    // The permissive lookup: an unconfigured host
    // is its own ssh destination behind the
    // approval above; only "local" and an
    // empty-ssh half-written entry refuse here.
    if let Err(e) = crate::shell::host_entry_or_bare(name) {
        let msg = format!("Error: {e}");
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: msg.clone(),
            },
        )
        .await?;
        crate::daemon::stats::finish_command(cmd_id, 1);
        crate::daemon::utils::log_shell_command(
            session_id,
            "s?",
            true,
            cmd,
            "host-error",
            -1,
            &msg,
        );
        return Ok(msg);
    }
    if let Err(msg) = shell_exec::ensure_connected(name, false, id.as_str(), tx, rx).await {
        let msg = format!("Error: {msg}");
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: msg.clone(),
            },
        )
        .await?;
        crate::daemon::stats::finish_command(cmd_id, 1);
        crate::daemon::utils::log_shell_command(
            session_id,
            "s?",
            true,
            cmd,
            "host-error",
            -1,
            &msg,
        );
        return Ok(msg);
    }
    let run = if *interactive {
        shell_exec::run_interactive_in_shell(
            crate::shell::registry(),
            owner.clone(),
            cmd,
            max_per_owner,
            shell.as_deref(),
            host.clone(),
            |id| {
                let (rows, cols) = crate::shell::spawn_size();
                crate::shell::spawn_ssh_host(id, name, rows, cols)
            },
        )
        .await
    } else {
        shell_exec::run_in_shell_with_prompts(
            crate::shell::registry(),
            owner.clone(),
            cmd,
            max_per_owner,
            shell.as_deref(),
            *timeout_secs,
            id.as_str(),
            tx,
            rx,
            host.clone(),
            |id| {
                let (rows, cols) = crate::shell::spawn_size();
                crate::shell::spawn_ssh_host(id, name, rows, cols)
            },
        )
        .await
    };
    finish_run(run, owner, a, tx).await
}

/// The reply for a finished run, local or named: the result text with its
/// shell line, the stats, the `ToolResult` frame and the shell log line.
async fn finish_run<W>(
    run: anyhow::Result<(crate::shell::ShellId, shell_exec::ShellRunResult)>,
    owner: Owner,
    a: ApprovedRun<'_>,
    tx: &mut W,
) -> anyhow::Result<String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let ApprovedRun {
        id,
        cmd,
        interactive,
        cmd_id,
        session_id,
        ..
    } = a;
    let text = match run {
        Ok((shell_id, result)) => {
            let text = with_shell_line(&result.text, shell_id.as_str(), *interactive);
            crate::daemon::stats::finish_command(cmd_id, result.exit_code);
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: text.clone(),
                },
            )
            .await?;
            crate::daemon::utils::log_shell_command(
                session_id,
                shell_id.as_str(),
                true,
                cmd,
                "approved",
                result.exit_code,
                &text,
            );
            text
        }
        Err(e) => {
            let shell_id = crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_else(|| "s?".to_string());
            let msg = shell_run_error_text(shell_id.as_str(), &e);
            crate::daemon::stats::finish_command(cmd_id, 1);
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: msg.clone(),
                },
            )
            .await?;
            crate::daemon::utils::log_shell_command(
                session_id,
                &shell_id,
                true,
                cmd,
                "shell-error",
                -1,
                &msg,
            );
            msg
        }
    };
    Ok(text)
}

#[cfg(test)]
mod ghost_foreground_gate_tests {
    use super::ghost_foreground_refusal;

    /// The unreadable config the `Err` arm is tested with: built in the test,
    /// never read from a real `config.toml`.
    fn unreadable() -> anyhow::Result<crate::config::Config> {
        Err(anyhow::anyhow!("toml parse error at line 3"))
    }

    fn readable(sandbox_enabled: bool) -> anyhow::Result<crate::config::Config> {
        let mut c = crate::config::Config::default();
        c.sandbox.enabled = sandbox_enabled;
        Ok(c)
    }

    #[test]
    fn ghost_foreground_refusal_refuses_a_ghost_when_the_config_is_unreadable() {
        let r = ghost_foreground_refusal(true, &unreadable());
        let text = r.expect("a ghost with an unreadable config is refused");
        assert!(
            text.contains("toml parse error at line 3"),
            "the refusal must name the load error, got: {text}"
        );
        assert!(
            text.starts_with("Error:"),
            "refusals read as errors: {text}"
        );
    }

    #[test]
    fn ghost_foreground_refusal_refuses_a_ghost_when_the_sandbox_is_on() {
        let r = ghost_foreground_refusal(true, &readable(true));
        assert!(
            r.is_some(),
            "a ghost is refused foreground while the sandbox is enabled"
        );
    }

    #[test]
    fn ghost_foreground_refusal_allows_a_ghost_when_the_sandbox_is_off() {
        assert_eq!(
            ghost_foreground_refusal(true, &readable(false)),
            None,
            "with the sandbox off, foreground is no worse for a ghost than any other command"
        );
    }

    #[test]
    fn ghost_foreground_refusal_allows_a_non_ghost_when_the_config_is_unreadable() {
        assert_eq!(
            ghost_foreground_refusal(false, &unreadable()),
            None,
            "a non-ghost keeps today's behaviour: the run falls back to the defaults"
        );
    }
}
