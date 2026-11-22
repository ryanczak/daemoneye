//! `execute_tool_call`'s arms for the shell opener: `OpenShell`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

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
    let SessionCtx {
        session_id, shells, ..
    } = ctx;
    match call {
        PendingCall::OpenShell {
            host, rows, cols, ..
        } => 'arm: {
            // The ghost sandbox gate, before anything else — the same two
            // fail-closed conditions the foreground gate refuses on: an
            // enabled sandbox (this shell would be a plain host PTY) and the
            // startup-load fallback (bug-19-1).
            let config = crate::config::current();
            if let Some(r) =
                ghost_shell_open_refusal(is_ghost, &config, crate::config::startup_error().as_ref())
            {
                break 'arm Ok(ToolCallOutcome::Result(r));
            }
            if let Some(policy) = ghost_policy.as_ref() {
                let asked = match resolve_host(host.as_deref()) {
                    Ok(parsed) => parsed,
                    Err(msg) => {
                        break 'arm Ok(ToolCallOutcome::Result(format!("Error: {msg}")));
                    }
                };
                if let Some(msg) = refuse_ghost_host(policy, &asked, "open_shell", session_id) {
                    break 'arm Ok(ToolCallOutcome::Result(msg));
                }
            }
            if let Err(msg) = connect_named_host(host.as_deref(), is_ghost, call.id(), tx, rx).await
            {
                crate::daemon::utils::log_event(
                    "ssh_connect_failed",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "open_shell",
                        "requested_host": host.as_deref().unwrap_or("local"),
                        "error": msg,
                    }),
                );
                break 'arm Ok(ToolCallOutcome::Result(format!("Error: {msg}")));
            }
            Ok(ToolCallOutcome::Result(
                knowledge::open_shell(
                    shells,
                    &call_owner(is_ghost, session_id),
                    host.as_deref(),
                    *rows,
                    *cols,
                )
                .await,
            ))
        }

        other => anyhow::bail!("{} is not one of the shell opener", other.tool_name()),
    }
}

#[cfg(test)]
mod gate_tests {
    use super::ghost_shell_open_refusal;

    fn readable(sandbox_enabled: bool) -> crate::config::Config {
        let mut c = crate::config::Config::default();
        c.sandbox.enabled = sandbox_enabled;
        c
    }

    /// A startup load failure as `Config::load` wraps it: the parse error
    /// under a `parsing <path>` context.
    fn startup_failure() -> anyhow::Error {
        anyhow::anyhow!("expected `=`, found newline at line 3")
            .context("parsing /home/op/.daemoneye/etc/config.toml")
    }

    #[test]
    fn ghost_open_shell_refusal_refuses_a_ghost_when_the_sandbox_is_on() {
        let r = ghost_shell_open_refusal(true, &readable(true), None);
        assert!(
            r.is_some(),
            "a ghost is refused open_shell while the sandbox is enabled"
        );
    }

    #[test]
    fn ghost_open_shell_refusal_allows_a_ghost_when_the_sandbox_is_off() {
        assert_eq!(
            ghost_shell_open_refusal(true, &readable(false), None),
            None,
            "with the sandbox off, open_shell is no worse for a ghost than any other command"
        );
    }

    #[test]
    fn ghost_open_shell_refusal_allows_a_chat_call_with_the_sandbox_on() {
        assert_eq!(
            ghost_shell_open_refusal(false, &readable(true), None),
            None,
            "the sandbox gate is a ghost gate; a chat session opens shells as before"
        );
    }

    /// A ghost running on fallback defaults because its startup load failed
    /// is refused, and the refusal names the load error.
    #[test]
    fn ghost_open_shell_is_refused_while_the_daemon_runs_on_fallback_defaults() {
        let e = startup_failure();
        let r = ghost_shell_open_refusal(true, &readable(false), Some(&e));
        let text = r.expect("a ghost on fallback defaults is refused");
        assert!(
            text.contains("expected `=`, found newline at line 3"),
            "the refusal must name the load error, got: {text}"
        );
        assert!(
            text.contains("/home/op/.daemoneye/etc/config.toml"),
            "the refusal must name the file under its context, got: {text}"
        );
        assert!(
            text.contains("fallback defaults"),
            "the refusal must say the daemon is on defaults, got: {text}"
        );
        assert!(
            text.starts_with("Error:"),
            "the refusal is an error the model reads, got: {text}"
        );
        assert!(
            text.contains("open_shell"),
            "the refusal names the tool, got: {text}"
        );
    }
}

/// Drives `execute_tool_call` for one tool call against a fixture registry
/// and session store, returning the tool result text. `session_id` is the
/// ghost's session id; the caller owns the registry fixture and the store.
#[cfg(test)]
pub(super) mod harness {
    use super::*;

    pub(super) async fn run_call(
        call: &PendingCall,
        session_id: &str,
        store: &SessionStore,
        reg: &crate::shell::ShellRegistry,
    ) -> String {
        let (mut tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let (_client_tx, rx) = tokio::io::duplex(64 * 1024);
        let mut rx = tokio::io::BufReader::new(rx);
        let outcome = execute_tool_call(
            call,
            &mut tx,
            &mut rx,
            SessionCtx {
                session_id: Some(session_id),
                session_name: session_id,
                chat_pane: None,
                sessions: store,
                shells: reg,
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await
        .expect("the tool call must not error");
        match outcome {
            ToolCallOutcome::Result(t) => t,
            ToolCallOutcome::UserMessage(t) => format!("[redirected] {t}"),
            ToolCallOutcome::SpawnGhostSession { tool_result, .. } => tool_result,
        }
    }

    pub(super) fn ghost_session() -> crate::daemon::session::SessionEntry {
        crate::daemon::session::SessionEntry::new_ghost(
            "test",
            crate::ipc::GhostConfig::default(),
            crate::daemon::ghost::GhostTrigger::Webhook,
            vec![],
        )
    }

    pub(super) fn open_shell_call() -> PendingCall {
        PendingCall::OpenShell {
            id: "os-1".to_string(),
            thought_signature: None,
            host: None,
            rows: None,
            cols: None,
        }
    }
}

/// The end-to-end owner rule (sandbox off): a ghost's `open_shell` lands a
/// shell owned by `Owner::Ghost(<session id>)`, the ghost lists it and closes
/// it through the executor without an "is not yours" refusal, and the
/// teardown's `close_owner_shells` finds nothing left.
///
/// `close_shell` and `send_input` read the daemon's **global** registry
/// (`crate::shell::registry()`), not `SessionCtx.shells`, so both the open
/// and the close go through the executor with the global registry as the
/// context's — the way a real run reaches it, and the way
/// `approved_path_tests` in `dispatch_foreground.rs` drives the pair.
#[cfg(test)]
mod owner_tests {
    use super::super::{PendingCall, SessionStore, with_sessions};
    use super::harness::{ghost_session, open_shell_call, run_call};
    use crate::shell::Owner;

    /// The end-to-end owner rule (sandbox off): a ghost's `open_shell` lands a
    /// shell owned by `Owner::Ghost(<session id>)`, the ghost lists it and
    /// closes it through the executor without an "is not yours" refusal, and
    /// the teardown's `close_owner_shells` finds nothing left.
    ///
    /// `close_shell` and `send_input` read the daemon's **global** registry
    /// (`crate::shell::registry()`), not `SessionCtx.shells`, so both the
    /// open and the close go through the executor with the global registry as
    /// the context's — the way a real run reaches it, and the way
    /// `approved_path_tests` in `dispatch_foreground.rs` drives the pair.
    #[tokio::test]
    async fn ghost_open_shell_is_owned_by_the_ghost_and_usable_by_it() {
        let (_guard, _home) = crate::test_home();
        let reg = crate::shell::registry();
        let store = SessionStore::new();
        with_sessions(&store, |s| {
            s.insert("gsid-open-e2e".to_string(), ghost_session());
        });

        let text = run_call(&open_shell_call(), "gsid-open-e2e", &store, reg).await;
        assert!(
            text.contains("Opened shell"),
            "with the sandbox off the ghost still opens a shell, got: {text}"
        );
        let owned = crate::shell::with_shells(reg, |s| {
            s.values()
                .find(|r| r.owner == Owner::Ghost("gsid-open-e2e".to_string()))
                .map(|r| r.id.as_str().to_string())
        });
        let id = owned.expect("the opened shell must be owned by Owner::Ghost(gsid-open-e2e)");

        let list = run_call(
            &PendingCall::ListShells {
                id: "ls-1".to_string(),
                thought_signature: None,
                scope: None,
            },
            "gsid-open-e2e",
            &store,
            reg,
        )
        .await;
        assert!(
            list.contains(&id),
            "list_shells (default scope) must list the ghost's shell {id}, got: {list}"
        );
        assert!(
            !list.contains("is not yours"),
            "listing must not be refused, got: {list}"
        );

        let close = run_call(
            &PendingCall::CloseShell {
                id: "cs-1".to_string(),
                thought_signature: None,
                shell: id.clone(),
            },
            "gsid-open-e2e",
            &store,
            reg,
        )
        .await;
        assert!(
            !close.contains("is not yours"),
            "close_shell on the ghost's own shell must not be refused, got: {close}"
        );
        assert!(
            !close.contains("Error:"),
            "the close must not fail: {close}"
        );
        // The close is one-way (`Sent TERM`) or observed (`Closed shell`);
        // both name the id and neither refuses. Ownership is what this test
        // pins: the refusal path would have said "is not yours".
        assert!(
            close.contains(&format!("shell {id}")),
            "the close answers about the ghost's shell: {close}"
        );
        crate::daemon::executor::knowledge::close_owner_shells(
            reg,
            &Owner::Ghost("gsid-open-e2e".to_string()),
            std::time::Duration::from_secs(2),
        )
        .await;
    }

    /// The teardown contract: after `close_owner_shells(Owner::Ghost(<id>))`,
    /// no shell the ghost opened remains. Opens two shells through the
    /// executor so the teardown has more than one record to close.
    #[tokio::test]
    async fn teardown_closes_every_shell_the_ghost_opened() {
        let (_guard, _home) = crate::test_home();
        let reg = crate::shell::ShellRegistry::default();
        let store = SessionStore::new();
        with_sessions(&store, |s| {
            s.insert("gsid-teardown-e2e".to_string(), ghost_session());
        });

        let first = run_call(&open_shell_call(), "gsid-teardown-e2e", &store, &reg).await;
        let second = run_call(&open_shell_call(), "gsid-teardown-e2e", &store, &reg).await;
        assert!(
            first.contains("Opened shell") && second.contains("Opened shell"),
            "both opens must land, got: {first} | {second}"
        );
        let count = crate::shell::with_shells(&reg, |s| {
            s.values()
                .filter(|r| r.owner == Owner::Ghost("gsid-teardown-e2e".to_string()))
                .count()
        });
        assert_eq!(count, 2, "both shells belong to the ghost");

        let closed = crate::daemon::executor::knowledge::close_owner_shells(
            &reg,
            &Owner::Ghost("gsid-teardown-e2e".to_string()),
            std::time::Duration::from_secs(2),
        )
        .await;
        assert_eq!(closed.len(), 2, "the teardown closes both, got: {closed:?}");
        let live = crate::shell::with_shells(&reg, |s| {
            s.values()
                .filter(|r| r.exited_unix.is_none())
                .map(|r| r.id.as_str().to_string())
                .collect::<Vec<_>>()
        });
        assert!(
            live.is_empty(),
            "no live shell the ghost opened remains (the reaper owns the exited records): {live:?}"
        );
    }
}
