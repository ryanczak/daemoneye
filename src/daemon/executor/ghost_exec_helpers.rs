//! Helper routines for `run_background`: ghost host-rebinding and policy
//! fencing (`may_use_ssh_target`, `may_elevate_script`), remote script
//! command preparation, and the three PTY execution branches (remote named
//! host, local `sbx` sandbox, and local detached host).

use super::*;
use crate::daemon::executor::{ghost_remote, ghost_sandbox, shell_exec};
use crate::daemon::policy::GhostPolicy;

pub(super) struct PreparedGhostPolicy {
    pub policy: Option<GhostPolicy>,
    pub remote_script: Option<(String, String)>,
    pub remote_script_is_sudo: bool,
}

pub(super) async fn rebind_and_fence_ghost_policy<W>(
    id: &str,
    cmd: &str,
    host: Option<&str>,
    session_id: Option<&str>,
    is_ghost: bool,
    initial_policy: Option<&GhostPolicy>,
    tx: &mut W,
) -> anyhow::Result<Result<PreparedGhostPolicy, ToolCallOutcome>>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let ghost_policy: Option<GhostPolicy> = match (is_ghost, host, initial_policy) {
        (true, Some(name), Some(policy)) => {
            let refusal = match crate::host::HostId::parse(name) {
                Err(e) => Err(format!("Error: {e}")),
                Ok(h) => policy
                    .for_host(&h, &crate::config::current().hosts)
                    .map_err(|e| format!("Error: {e}")),
            };
            match refusal {
                Ok(p) => Some(p),
                Err(msg) => {
                    crate::daemon::utils::log_event(
                        "ghost_host_refused",
                        serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "tool": "run_terminal_command",
                            "requested_host": name,
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
                    return Ok(Err(ToolCallOutcome::Result(msg)));
                }
            }
        }
        _ => initial_policy.cloned(),
    };

    let remote_script = ghost_policy
        .as_ref()
        .filter(|_| is_ghost)
        .and_then(|p| p.remote_script_call(cmd));
    let remote_script_is_sudo = remote_script.is_some()
        && (crate::daemon::utils::command_has_sudo(cmd)
            || ghost_policy
                .as_ref()
                .map(|p| p.run_with_sudo)
                .unwrap_or(false));

    if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost)
        && !policy.may_use_ssh_target()
    {
        let (requested_host, msg) = match policy.remote_host.as_ref() {
            Some(h) => (h.as_str().to_string(), ghost_host_refusal(policy, h)),
            None => {
                let target = policy.ssh_target.clone().unwrap_or_default();
                let list = policy.hosts.join(", ");
                let msg = format!(
                    "Error: this runbook's ssh_target {target} is not a configured host, \
                     so its `hosts:` list cannot grant it. Hosts this runbook allows: \
                     {list} ({}). Configure a [hosts.<name>] entry whose ssh is {target}, \
                     or work on the daemon host.",
                    policy.hosts.len()
                );
                (target, msg)
            }
        };
        crate::daemon::utils::log_event(
            "ghost_host_refused",
            serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "tool": "run_terminal_command",
                "requested_host": requested_host,
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
        return Ok(Err(ToolCallOutcome::Result(msg)));
    }

    if remote_script_is_sudo
        && let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost)
        && let Some((name, _)) = remote_script.as_ref()
        && !policy.may_elevate_script(name)
    {
        let host = policy
            .remote_host
            .as_ref()
            .map(|h| h.as_str().to_string())
            .unwrap_or_default();
        let list = policy.remote_sudo_scripts.join(", ");
        let msg = format!(
            "Error: script {name} is not in host {host}'s sudo_scripts ({list}), so it may \
             not run under sudo there. Add it to [hosts.{host}] sudo_scripts, or run it \
             without sudo."
        );
        crate::daemon::utils::log_event(
            "ghost_elevation_refused",
            serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "host": host,
                "script": name,
                "allowed_scripts": policy.remote_sudo_scripts,
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
        return Ok(Err(ToolCallOutcome::Result(msg)));
    }

    Ok(Ok(PreparedGhostPolicy {
        policy: ghost_policy,
        remote_script,
        remote_script_is_sudo,
    }))
}

pub(super) async fn build_remote_script_command<W>(
    id: &str,
    cmd: &str,
    session_id: Option<&str>,
    remote_script: Option<&(String, String)>,
    remote_script_is_sudo: bool,
    tx: &mut W,
) -> anyhow::Result<Result<String, ToolCallOutcome>>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let Some((name, args)) = remote_script else {
        return Ok(Ok(cmd.to_string()));
    };
    match crate::scripts::read_script(name) {
        Ok(content) => {
            let built = if remote_script_is_sudo {
                format!(
                    "{} && {}",
                    crate::scripts::remote_materialize_cmd(name, &content),
                    cmd
                )
            } else {
                crate::scripts::remote_stream_cmd(&content, args)
            };
            Ok(Ok(built))
        }
        Err(e) => {
            let msg = format!(
                "Error: cannot run script '{}' on the remote host — it is not \
                 available on the daemon host: {}. Use write_script to create it first.",
                name, e
            );
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: msg.clone(),
                },
            )
            .await?;
            log_command(session_id, "background", "", cmd, "transfer-failed", &msg);
            Ok(Err(ToolCallOutcome::Result(msg)))
        }
    }
}

pub(super) async fn run_ghost_remote_branch<W>(
    id: &str,
    cmd: &str,
    cmd_id: usize,
    session_id: Option<&str>,
    host: &crate::host::HostId,
    name: &str,
    tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Err(msg) = shell_exec::ensure_connected(
        name,
        true,
        id,
        &mut tokio::io::sink(),
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    {
        let output = format!("Error: {msg}");
        crate::daemon::stats::finish_command(cmd_id, 1);
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: output.clone(),
            },
        )
        .await?;
        log_command(
            session_id,
            "background",
            name,
            cmd,
            "connect-failed",
            &output,
        );
        return Ok(ToolCallOutcome::Result(output));
    }
    let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
    let (output, exit_code) = ghost_remote::run_ghost_remote(
        crate::shell::registry(),
        owner,
        host.clone(),
        cmd,
        id,
        |shell_id| {
            let (rows, cols) = crate::shell::spawn_size();
            crate::shell::spawn_ghost_ssh_host(shell_id, name, rows, cols)
        },
    )
    .await;
    crate::daemon::stats::finish_command(cmd_id, exit_code);
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: output.clone(),
        },
    )
    .await?;
    log_command(session_id, "background", name, cmd, "approved", &output);
    Ok(ToolCallOutcome::Result(output))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_ghost_sandbox_branch<W>(
    id: &str,
    cmd: &str,
    cmd_id: usize,
    session_id: Option<&str>,
    sessions: &crate::daemon::session::SessionStore,
    sandbox_cfg: &crate::config::SandboxConfig,
    tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let sid = session_id.unwrap_or_default().to_string();
    let profile = with_sessions(sessions, |store| {
        store
            .get(sid.as_str())
            .and_then(|e| e.ghost_config().and_then(|g| g.agent.clone()))
    });
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    let run = crate::shell::real_runner(program);
    let (output, exit_code) = match ghost_sandbox::ensure_ghost_sandbox(
        &run,
        sandbox_cfg,
        &sid,
        profile.as_deref(),
    )
    .await
    {
        Ok(sandbox) => {
            let staged: Result<String, String> =
                match crate::daemon::executor::sbx::script_invocation(cmd, |n| {
                    crate::scripts::resolve_script(n).is_ok()
                }) {
                    Some((name, args_tail)) => {
                        let host_path = crate::config::scripts_dir().join(&name);
                        match ghost_sandbox::stage_script(
                            &run,
                            &sandbox,
                            &name,
                            &host_path.to_string_lossy(),
                        )
                        .await
                        {
                            Ok(()) => Ok(crate::daemon::executor::sbx::staged_script_command(
                                &name, &args_tail,
                            )),
                            Err(reason) => Err(reason),
                        }
                    }
                    None => Ok(cmd.to_string()),
                };
            match staged {
                Ok(cmd_to_run) => {
                    let spawn_sandbox = sandbox.clone();
                    ghost_sandbox::run_ghost_in_sandbox(
                        crate::shell::registry(),
                        crate::shell::Owner::Ghost(sid.clone()),
                        &sandbox,
                        &cmd_to_run,
                        id,
                        move |shell_id| {
                            let (rows, cols) = crate::shell::spawn_size();
                            let s = spawn_sandbox.clone();
                            async move {
                                crate::shell::spawn_sbx_host(shell_id, &s, rows, cols).await
                            }
                        },
                    )
                    .await
                }
                Err(reason) => (reason, 1),
            }
        }
        Err(reason) => (reason, 1),
    };
    crate::daemon::stats::finish_command(cmd_id, exit_code);
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: output.clone(),
        },
    )
    .await?;
    log_command(session_id, "background", "", cmd, "approved", &output);
    Ok(ToolCallOutcome::Result(output))
}

pub(super) async fn run_ghost_local_pty_branch<W>(
    id: &str,
    cmd: &str,
    cmd_id: usize,
    session_id: Option<&str>,
    tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
    let (output, exit_code) = ghost_remote::run_ghost_remote(
        crate::shell::registry(),
        owner,
        crate::host::HostId::Local,
        cmd,
        id,
        |shell_id| {
            let (rows, cols) = crate::shell::spawn_size();
            crate::shell::spawn_detached_host(shell_id, &[], rows, cols)
        },
    )
    .await;
    crate::daemon::stats::finish_command(cmd_id, exit_code);
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: output.clone(),
        },
    )
    .await?;
    log_command(session_id, "background", "", cmd, "approved", &output);
    Ok(ToolCallOutcome::Result(output))
}
