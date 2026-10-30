use super::ghost_host_refusal;
use super::prompt_and_await_approval;
use super::send_response_split;
use super::{ApprovalRequest, GhostCtx, SessionCtx, ToolCallOutcome};
use crate::daemon::session::with_sessions;
use crate::daemon::utils::log_command;
use crate::ipc::Response;

pub(super) struct FgArgs<'a> {
    pub id: &'a str,
    pub cmd: &'a str,
}

pub(super) async fn refuse_foreground<W, R>(
    args: FgArgs<'_>,
    ctx: SessionCtx<'_>,
    ghost_ctx: GhostCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let FgArgs { id, cmd } = args;
    let GhostCtx { is_ghost, .. } = ghost_ctx;
    let _ = (ctx, rx);

    // The tmux keystroke-injection path is deleted (phase 05). A call reaches
    // here only when no session PTY shell claimed it — a ghost turn, or a chat
    // session with no session id. It is refused with the reason, never run.
    let who = if is_ghost {
        "A ghost turn"
    } else {
        "This command"
    };
    let reason = format!(
        "Error: {who} has no PTY shell to run `{cmd}` in, and there is no tmux fallback. \
         Chat commands run in the session's PTY shell; a ghost runs on a configured host \
         or in its sandbox."
    );
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: reason.clone(),
        },
    )
    .await?;
    log_command(None, "foreground", "", cmd, "refused", &reason);
    Ok(ToolCallOutcome::Result(reason))
}

pub(super) async fn run_background<W, R>(
    id: &str,
    cmd: &str,
    host: Option<&str>,
    ctx: SessionCtx<'_>,
    ghost_ctx: GhostCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let SessionCtx {
        session_id,
        session_name: _,
        sessions,
        ..
    } = ctx;
    let GhostCtx {
        policy: ghost_policy,
        is_ghost,
        ..
    } = ghost_ctx;
    // A ghost may name the host a command runs on (`host`), overriding the
    // runbook's `ssh_target` binding: rebind the policy to it, so every
    // gate and route below works as if the runbook had named that host.
    // The `hosts:` fence that follows still applies — now to the named
    // host — with its existing wording and event.
    let rebound;
    let ghost_policy = match (is_ghost, host, ghost_policy) {
        (true, Some(name), Some(policy)) => {
            let refusal = match crate::host::HostId::parse(name) {
                Err(e) => Err(format!("Error: {e}")),
                Ok(h) => policy
                    .for_host(&h, &crate::config::Config::load().unwrap_or_default().hosts)
                    .map_err(|e| format!("Error: {e}")),
            };
            match refusal {
                Ok(p) => {
                    rebound = p;
                    Some(&rebound)
                }
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
                    return Ok(ToolCallOutcome::Result(msg));
                }
            }
        }
        _ => ghost_policy,
    };
    // § 2.4 remote execution: a ghost ssh_target whitelisted-script invocation ships to
    // the remote either by streaming (default, no remote disk) or — under sudo — by a
    // persistent materialize to the sudoers-authorized path.
    let remote_script = ghost_policy
        .as_ref()
        .filter(|_| is_ghost)
        .and_then(|p| p.remote_script_call(cmd)); // Option<(String, String)>
    let remote_script_is_sudo = remote_script.is_some()
        && (crate::daemon::utils::command_has_sudo(cmd)
            || ghost_policy
                .as_ref()
                .map(|p| p.run_with_sudo)
                .unwrap_or(false));

    // The runbook's `hosts:` list fences `ssh_target` before anything runs:
    // a non-empty list must grant the host the value resolved to, and a
    // value that resolved to no configured host cannot be granted at all.
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
        return Ok(ToolCallOutcome::Result(msg));
    }

    // The resolved host's `sudo_scripts` gates elevated script runs: a host
    // that declares a whitelist refuses anything it does not name.
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
        return Ok(ToolCallOutcome::Result(msg));
    }

    // Ghost shells: resolve bare/relative script names to absolute path.
    let resolved_cmd;
    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
        resolved_cmd = policy.resolve_command(cmd);
        resolved_cmd.as_str()
    } else {
        cmd
    };

    let cmd_id = match prompt_and_await_approval(
        ApprovalRequest {
            id,
            cmd,
            background: true,
            target_pane_hint: None,
            host: None,
            user_shell: None,
            interactive: false,
        },
        session_id,
        ghost_policy,
        tx,
        rx,
    )
    .await?
    {
        Ok(id) => id,
        Err(outcome) => return Ok(outcome),
    };

    // Ghost shells: build the remote command — stream by default, persist only for sudo.
    let remote_built_cmd;
    let cmd = if let Some((name, args)) = remote_script.as_ref() {
        match crate::scripts::read_script(name) {
            Ok(content) => {
                remote_built_cmd = if remote_script_is_sudo {
                    // Sudo: persistent materialize to the sudoers-authorized path, then
                    // run the resolved `sudo ~/.daemoneye/scripts/<name> …` command.
                    format!(
                        "{} && {}",
                        crate::scripts::remote_materialize_cmd(name, &content),
                        cmd
                    )
                } else {
                    // Default: stream content to the interpreter's stdin — no remote disk.
                    crate::scripts::remote_stream_cmd(&content, args)
                };
                remote_built_cmd.as_str()
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
                return Ok(ToolCallOutcome::Result(msg));
            }
        }
    } else {
        cmd
    };

    // 08c: a ghost whose `ssh_target` resolved to a configured host runs the
    // approved command in a PTY shell **on that host** — the same substrate
    // chat uses — synchronously, capped at the ghost turn budget, instead of
    // wrapping it in `ssh <target> <cmd>` in a tmux window. Everything above
    // (the two 08a gates, resolve_command, the approval, the remote-script
    // build) already ran; everything below serves a local ghost, an
    // unresolved ssh_target, and `backend = "tmux"`.
    if let Some(host) = super::ghost_remote::ghost_remote_host(is_ghost, ghost_policy)
        && let crate::host::HostId::Named(name) = &host
    {
        // Connect before any shell spawns on the host — with `BatchMode`,
        // so a host needing a credential fails here instead of hanging the
        // ghost (its transport cannot answer a prompt).
        if let Err(msg) = super::shell_exec::ensure_connected(
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
        let (output, exit_code) = super::ghost_remote::run_ghost_remote(
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
        return Ok(ToolCallOutcome::Result(output));
    }

    // A **sandboxed** local ghost runs the approved command in a PTY shell
    // inside the ghost run's own sbx sandbox: the sandbox is created on the
    // first command behind a fail-closed preflight (a failure returns the
    // reason as the tool result and the command is never run on the host).
    // The ghost's sandbox profile is its agent name.
    let sandbox_cfg = crate::config::Config::load().unwrap_or_default().sandbox;
    if super::ghost_sandbox::ghost_sandbox_pty(is_ghost, &sandbox_cfg, ghost_policy) {
        let sid = session_id.unwrap_or_default().to_string();
        let profile = with_sessions(sessions, |store| {
            store
                .get(sid.as_str())
                .and_then(|e| e.ghost_config.as_ref().and_then(|g| g.agent.clone()))
        });
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        let run = crate::shell::real_runner(program);
        let (output, exit_code) = match super::ghost_sandbox::ensure_ghost_sandbox(
            &run,
            &sandbox_cfg,
            &sid,
            profile.as_deref(),
        )
        .await
        {
            Ok(sandbox) => {
                // An approved-script invocation runs the operator's copy of
                // the script, staged into the sandbox on every call, never
                // the sandbox's own (overwritable) one. A staging failure is
                // an operator-facing reason: the command never runs.
                let staged: Result<String, String> =
                    match crate::daemon::executor::sbx::script_invocation(cmd, |n| {
                        crate::scripts::resolve_script(n).is_ok()
                    }) {
                        Some((name, args_tail)) => {
                            let host_path = crate::config::scripts_dir().join(&name);
                            match super::ghost_sandbox::stage_script(
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
                        super::ghost_sandbox::run_ghost_in_sandbox(
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
        return Ok(ToolCallOutcome::Result(output));
    }

    // A local ghost runs the approved command in a PTY shell on the daemon
    // host, owned by the ghost and reused for its later commands, as the
    // remote branch above does on its host. The sandbox keeps the tmux
    // window path below: its container is launched there.
    if super::ghost_remote::ghost_local_pty(
        is_ghost,
        crate::config::Config::load()
            .unwrap_or_default()
            .sandbox
            .enabled,
        ghost_policy,
    ) {
        let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
        let (output, exit_code) = super::ghost_remote::run_ghost_remote(
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
        return Ok(ToolCallOutcome::Result(output));
    }

    // The tmux window fallback is deleted (phase 05): a command that did not
    // take a PTY branch above has no route left. Name the unresolved
    // ssh_target when there is one so the model can fix the runbook.
    let reason = no_pty_route_refusal(is_ghost, ghost_policy);
    crate::daemon::stats::finish_command(cmd_id, 1);
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: reason.clone(),
        },
    )
    .await?;
    log_command(session_id, "background", "", cmd, "refused", &reason);
    Ok(ToolCallOutcome::Result(reason))
}

/// The refusal for a background command no PTY branch claimed. Names the
/// ghost's unresolved `ssh_target` when there is one, so the model can fix
/// the runbook instead of retrying.
fn no_pty_route_refusal(
    is_ghost: bool,
    ghost_policy: Option<&crate::daemon::policy::GhostPolicy>,
) -> String {
    match ghost_policy
        .filter(|_| is_ghost)
        .and_then(|p| p.ssh_target.as_deref())
    {
        Some(target) => format!(
            "Error: this command has no PTY route: the runbook's ssh_target {target} is not \
             a configured [hosts] entry, and there is no tmux fallback. Add a [hosts.<name>] \
             entry whose ssh is {target}, or drop ssh_target to run on the daemon host."
        ),
        None => "Error: this command has no PTY route (no session shell, host or ghost branch \
             claimed it, and there is no tmux fallback). Run it in the chat session's shell \
             instead."
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::policy::GhostPolicy;
    use crate::daemon::session::SessionStore;

    /// Drive `refuse_foreground` the way the executor does and return the tool
    /// result it produced. The refusal fires before any target is chosen, so
    /// no PTY shell or tmux session is needed.
    async fn drive_refusal(
        session_id: Option<&str>,
        sessions: &SessionStore,
        ghost_policy: Option<&GhostPolicy>,
        is_ghost: bool,
        cmd: &str,
    ) -> String {
        let mut tx = tokio::io::sink();
        let mut rx = tokio::io::BufReader::new(tokio::io::empty());
        let outcome = refuse_foreground(
            FgArgs {
                id: "fg-refuse-1",
                cmd,
            },
            SessionCtx {
                session_id,
                session_name: "de-test",
                chat_pane: None,
                sessions,
            },
            GhostCtx {
                policy: ghost_policy,
                is_ghost,
            },
            &mut tx,
            &mut rx,
        )
        .await
        .expect("refuse_foreground must not error on the refusal path");
        match outcome {
            ToolCallOutcome::Result(text) => text,
            ToolCallOutcome::UserMessage(m) => {
                panic!("the refusal path returns a result, got UserMessage {m:?}")
            }
            ToolCallOutcome::SpawnGhostSession { tool_result, .. } => {
                panic!("the refusal path returns a result, got SpawnGhostSession {tool_result:?}")
            }
        }
    }

    #[tokio::test]
    async fn ghost_turn_without_a_pty_shell_is_refused() {
        let store = SessionStore::default();
        let text = drive_refusal(Some("g1"), &store, None, true, "ls -la").await;
        assert!(
            text.contains("no PTY shell"),
            "expected a no-route refusal, got: {text}"
        );
        assert!(
            text.contains("no tmux fallback"),
            "expected the refusal to name the removed fallback, got: {text}"
        );
    }

    #[tokio::test]
    async fn chat_command_without_session_is_refused() {
        let store = SessionStore::default();
        let text = drive_refusal(None, &store, None, false, "systemctl status").await;
        assert!(
            text.contains("no PTY shell"),
            "expected a no-route refusal, got: {text}"
        );
    }

    /// With the sandbox off, a ghost whose `ssh_target` names no configured
    /// host takes none of the three PTY branches, and the tail refuses it
    /// naming the target. Before phase 05 this fell through to a tmux window.
    #[test]
    fn ghost_unresolved_ssh_target_is_refused_without_sandbox() {
        let policy = GhostPolicy::from_config(&crate::ipc::GhostConfig {
            ssh_target: Some("web9.example".to_string()),
            ..Default::default()
        });
        assert!(
            policy.remote_host.is_none(),
            "the target must be unresolved"
        );
        let sandbox_off = crate::config::SandboxConfig::default();
        assert!(!sandbox_off.enabled);

        assert!(super::super::ghost_remote::ghost_remote_host(true, Some(&policy)).is_none());
        assert!(!super::super::ghost_sandbox::ghost_sandbox_pty(
            true,
            &sandbox_off,
            Some(&policy)
        ));
        assert!(!super::super::ghost_remote::ghost_local_pty(
            true,
            sandbox_off.enabled,
            Some(&policy)
        ));

        let text = no_pty_route_refusal(true, Some(&policy));
        assert!(
            text.starts_with("Error:"),
            "a refusal is error text: {text}"
        );
        assert!(
            text.contains("web9.example"),
            "the refusal must name the unresolved target: {text}"
        );
        assert!(
            text.contains("no tmux fallback"),
            "the refusal must say there is no tmux route: {text}"
        );

        // Negative: chat (not a ghost) never names a runbook target.
        let chat = no_pty_route_refusal(false, Some(&policy));
        assert!(
            !chat.contains("web9.example"),
            "chat refusal leaked: {chat}"
        );
    }
}

#[cfg(test)]
mod refusal_gate_tests {
    use super::run_background;
    use crate::daemon::executor::{GhostCtx, SessionCtx, ToolCallOutcome};
    use crate::daemon::session::SessionStore;

    /// Drive `run_background` the way a ghost does and return the tool result
    /// it produced. Unlike `run_background_resolves_ghost_script_paths` this
    /// needs **no tmux and no script on disk**: both gates this row adds
    /// return before the approval prompt and before any tmux call, so the
    /// command is refused without ever reaching the pane. It does need an
    /// isolated `HOME`, because refusals write to the event segment.
    async fn drive_run_background_refusal(
        session_id: &str,
        session_name: &str,
        sessions: &SessionStore,
        policy: &crate::daemon::policy::GhostPolicy,
        cmd: &str,
        host: Option<&str>,
    ) -> String {
        let ctx = SessionCtx {
            session_id: Some(session_id),
            session_name,
            chat_pane: None,
            sessions,
        };
        let ghost_ctx = GhostCtx {
            policy: Some(policy),
            is_ghost: true,
        };
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let (mut daemon_tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        match run_background(
            session_id,
            cmd,
            host,
            ctx,
            ghost_ctx,
            &mut daemon_tx,
            &mut daemon_rx,
        )
        .await
        .expect("run_background must not error on the refusal path")
        {
            ToolCallOutcome::Result(t) => t,
            ToolCallOutcome::UserMessage(t) => format!("[redirected] {t}"),
            ToolCallOutcome::SpawnGhostSession { .. } => String::new(),
        }
    }

    /// A ghost on a configured host whose elevated-permission whitelist names
    /// another script may not elevate this one: the call is refused **before**
    /// approval and before any tmux call, naming the script and the allowed
    /// set, and the refusal is recorded. Without the gate this command would
    /// auto-approve and only fail later with "not available on the daemon
    /// host", so the assertion on the exact message is what makes this test
    /// go red when the gate is removed.
    #[test]
    fn an_unlisted_elevated_script_is_refused_on_a_configured_host() {
        let (_home, _dir) = crate::test_home();

        let sessions = SessionStore::new();
        let session_id = "ghost-elevation-refused";
        let policy = crate::daemon::policy::GhostPolicy {
            auto_approve_scripts: vec!["other.sh".to_string()],
            ssh_target: Some("db01".to_string()),
            hosts: Vec::new(),
            remote_host: Some(crate::host::HostId::parse("db01").expect("well-formed host")),
            remote_sudo_scripts: vec!["allowed.sh".to_string()],
            ..crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-elevation-refused",
                &sessions,
                &policy,
                "sudo other.sh",
                None,
            )
            .await
        });

        assert!(
            text.contains("other.sh") && text.contains("allowed.sh"),
            "the refusal must name the refused script and the allowed set: {text}"
        );
        assert!(
            text.contains("db01"),
            "the refusal must name the host: {text}"
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("read the event segment");
        let refused = content.lines().find_map(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return None;
            };
            (v.get("event").and_then(|e| e.as_str()) == Some("ghost_elevation_refused"))
                .then_some(v)
        });
        assert!(
            refused.is_some(),
            "the elevation refusal must be recorded in the event segment:\n{content}"
        );
    }

    /// A ghost whose `ssh_target` names no configured host cannot be granted
    /// by a non-empty `hosts:` list at all — a list that cannot name what it
    /// allows is not a fence. The refusal names the raw value and the allowed
    /// set, and is recorded with the raw value as `requested_host`.
    #[test]
    fn an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list() {
        let (_home, _dir) = crate::test_home();

        let sessions = SessionStore::new();
        let session_id = "ghost-unconfigured-target";
        let policy = crate::daemon::policy::GhostPolicy {
            ssh_target: Some("user@far".to_string()),
            hosts: vec!["db01".to_string()],
            remote_host: None,
            remote_sudo_scripts: Vec::new(),
            ..crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-unconfigured-target",
                &sessions,
                &policy,
                "uptime",
                None,
            )
            .await
        });

        assert!(
            text.contains("user@far"),
            "the refusal must name the raw ssh_target: {text}"
        );
        assert!(
            text.contains("db01"),
            "the refusal must name the allowed set: {text}"
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("read the event segment");
        let refused = content.lines().find_map(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return None;
            };
            (v.get("event").and_then(|e| e.as_str()) == Some("ghost_host_refused")).then_some(v)
        });
        let refused = refused.unwrap_or_else(|| {
            panic!("the host refusal must be recorded in the event segment:\n{content}")
        });
        assert_eq!(
            refused.get("requested_host").and_then(|h| h.as_str()),
            Some("user@far"),
            "an unresolved ssh_target is recorded under its own value, not a host key"
        );
    }

    /// A ghost command naming a host outside its runbook's `hosts:` list is
    /// refused with the same sentence the `ssh_target` fence uses: it names
    /// the refused host and the allowed set.
    #[test]
    fn a_ghost_command_host_outside_its_hosts_list_is_refused() {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[hosts.db01]\nssh = \"db01\"\n\n[hosts.db02]\nssh = \"db02\"\n",
        )
        .expect("write config");

        let sessions = SessionStore::new();
        let session_id = "ghost-command-host-unlisted";
        let policy = crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig {
            hosts: vec!["db01".to_string()],
            ..Default::default()
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-command-host-unlisted",
                &sessions,
                &policy,
                "echo hi",
                Some("db02"),
            )
            .await
        });

        assert!(
            text.contains("db02"),
            "the refusal must name the refused host: {text}"
        );
        assert!(
            text.contains("db01"),
            "the refusal must name the allowed set: {text}"
        );
        drop(guard);
    }

    /// A ghost command naming a host that is not configured at all is
    /// refused before the fence: there is nothing to rebind to.
    #[test]
    fn a_ghost_command_host_that_is_not_configured_is_refused() {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[hosts.db01]\nssh = \"db01\"\n",
        )
        .expect("write config");

        let sessions = SessionStore::new();
        let session_id = "ghost-command-host-unconfigured";
        let policy = crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig {
            hosts: vec!["db01".to_string()],
            ..Default::default()
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-command-host-unconfigured",
                &sessions,
                &policy,
                "echo hi",
                Some("nope"),
            )
            .await
        });

        assert!(
            text.contains("nope"),
            "the refusal must name the unknown host: {text}"
        );
        drop(guard);
    }
}
