use super::ghost_host_refusal;
use super::prompt_and_await_approval;
use super::send_response_split;
use super::{ApprovalRequest, GhostCtx, SessionCtx, ToolCallOutcome};
use crate::daemon::session::with_sessions;
use crate::daemon::utils::log_command;
use crate::ipc::Response;

#[path = "ghost_exec_helpers.rs"]
mod helpers;
use helpers::*;

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
        sessions,
        ..
    } = ctx;
    let GhostCtx {
        policy: initial_policy,
        is_ghost,
        ..
    } = ghost_ctx;

    let PreparedGhostPolicy {
        policy: owned_policy,
        remote_script,
        remote_script_is_sudo,
    } = match rebind_and_fence_ghost_policy(id, cmd, host, session_id, is_ghost, initial_policy, tx)
        .await?
    {
        Ok(p) => p,
        Err(refusal_outcome) => return Ok(refusal_outcome),
    };
    let ghost_policy = owned_policy.as_ref();

    let resolved_cmd;
    let cmd = if let Some(policy) = ghost_policy.filter(|_| is_ghost) {
        resolved_cmd = policy.resolve_command(cmd);
        resolved_cmd.as_str()
    } else {
        cmd
    };

    if is_ghost
        && let Some(script_name) = resolved_sudo_script_name(ghost_policy, cmd)
        && let Some(notice) = local_copy_drift_notice(&script_name)
    {
        crate::bus::publish(notice);
    }

    let cmd_id = match prompt_and_await_approval(
        ApprovalRequest {
            id,
            cmd,
            background: true,
            target_pane_hint: None,
            host: None,
            user_shell: None,
            interactive: false,
            shell_label: None,
            sudo: None,
            session_id,
        },
        ghost_policy,
        tx,
        rx,
    )
    .await?
    {
        Ok(id) => id,
        Err(outcome) => return Ok(outcome),
    };

    let built_cmd = match build_remote_script_command(
        id,
        cmd,
        session_id,
        remote_script.as_ref(),
        remote_script_is_sudo,
        tx,
    )
    .await?
    {
        Ok(c) => c,
        Err(outcome) => return Ok(outcome),
    };
    let cmd = built_cmd.as_str();

    if let Some(host) = super::ghost_remote::ghost_remote_host(is_ghost, ghost_policy)
        && let crate::host::HostId::Named(name) = &host
    {
        return run_ghost_remote_branch(id, cmd, cmd_id, session_id, &host, name, tx).await;
    }

    let sandbox_cfg = crate::config::current().sandbox.clone();
    if super::ghost_sandbox::ghost_sandbox_pty(is_ghost, &sandbox_cfg, ghost_policy) {
        return run_ghost_sandbox_branch(id, cmd, cmd_id, session_id, sessions, &sandbox_cfg, tx)
            .await;
    }

    if super::ghost_remote::ghost_local_pty(
        is_ghost,
        crate::config::current().sandbox.enabled,
        ghost_policy,
    ) {
        return run_ghost_local_pty_branch(id, cmd, cmd_id, session_id, tx).await;
    }

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

/// The drift notice for a ghost sudo call that resolved to an installed
/// root-owned copy: `Some(Notice)` only when the copy is stale. The copy
/// still runs — this only makes the drift visible. `None` for a current
/// copy, an orphaned one, or no copy at all.
pub fn stale_sudo_notice(
    script_name: &str,
    drift: crate::scripts::CopyDrift,
) -> Option<crate::bus::Notice> {
    if drift != crate::scripts::CopyDrift::Stale {
        return None;
    }
    Some(
        crate::bus::Notice::new(
            crate::bus::Severity::Warning,
            crate::bus::Source::Ghost,
            "SudoCopyStale",
            format!(
                "sudo {name}: the installed copy differs from ~/.daemoneye/scripts/{name}",
                name = script_name
            ),
        )
        .body(format!(
            "The root-owned copy the sudoers rule runs is not the current \
             ~/.daemoneye/scripts/{name}. The copy still ran. Re-run \
             `daemoneye install-sudoers {name}` to update it.",
            name = script_name
        ))
        .label("script", script_name),
    )
}

/// The basename of the whitelisted script a ghost's command resolves to,
/// when that call is a local sudo call — i.e. exactly the case where
/// `resolve_command` rewrote the command to `sudo /usr/local/libexec/…`.
/// Remote calls run `~/.daemoneye/scripts/<name>` under the remote host's
/// own sudoers and are out of scope for this notice.
fn resolved_sudo_script_name(
    policy: Option<&crate::daemon::policy::GhostPolicy>,
    cmd: &str,
) -> Option<String> {
    let policy = policy?;
    if policy.ssh_target.is_some() {
        return None;
    }
    let resolved = policy.resolve_command(cmd);
    let rest = resolved.strip_prefix("sudo ")?.trim_start();
    let path = rest.split_whitespace().next()?;
    let copy = std::path::Path::new(path);
    if copy.parent() != Some(std::path::Path::new("/usr/local/libexec/daemoneye"))
        && copy
            .parent()
            .map(|p| p.starts_with("/usr/local/libexec/daemoneye"))
            != Some(true)
    {
        return None;
    }
    copy.file_name().and_then(|n| n.to_str()).map(String::from)
}

/// Drift between the installed copy and its source for one script, looked
/// up on the real filesystem (production path only; tests drive
/// `stale_sudo_notice` directly).
fn local_copy_drift_notice(script_name: &str) -> Option<crate::bus::Notice> {
    let copy = crate::scripts::root_script_path(script_name);
    let drift =
        crate::scripts::copy_drift(&copy, &crate::scripts::scripts_dir().join(script_name))?;
    stale_sudo_notice(script_name, drift)
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
                shells: crate::shell::registry(),
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
        let (_home, _dir) = crate::test_home();
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
        let (_home, _dir) = crate::test_home();
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
    use super::stale_sudo_notice;
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
            shells: crate::shell::registry(),
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

    #[test]
    fn stale_sudo_notice_is_a_warning_naming_the_script() {
        let notice = stale_sudo_notice("fix.sh", crate::scripts::CopyDrift::Stale)
            .expect("a stale copy must produce a notice");
        assert_eq!(notice.severity, crate::bus::Severity::Warning);
        assert_eq!(notice.source, crate::bus::Source::Ghost);
        assert!(
            notice.title.contains("fix.sh"),
            "the title names the script: {}",
            notice.title
        );
        assert!(
            notice.title.contains("~/.daemoneye/scripts/fix.sh"),
            "the title names the source path: {}",
            notice.title
        );
        assert_eq!(
            notice.labels.get("script").map(String::as_str),
            Some("fix.sh"),
            "the label is script=<name>"
        );

        assert_eq!(
            stale_sudo_notice("fix.sh", crate::scripts::CopyDrift::Current),
            None,
            "a current copy publishes nothing"
        );
        assert_eq!(
            stale_sudo_notice("fix.sh", crate::scripts::CopyDrift::Orphaned),
            None,
            "an orphaned copy publishes nothing"
        );
        assert_eq!(
            stale_sudo_notice("fix.sh", crate::scripts::CopyDrift::Unknown),
            None,
            "an unreadable pair publishes nothing"
        );
    }
}
