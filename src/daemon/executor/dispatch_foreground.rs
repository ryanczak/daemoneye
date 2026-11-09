//! `execute_tool_call`'s `run_terminal_command` foreground arm
//! (`PendingCall::Foreground`), split into its stages: the gates and the
//! approval (`dispatch`), the approved run (`run_approved`), the named-host
//! path (`run_on_named_host`) and the reply (`finish_run`). A child module of
//! `src/daemon/executor/mod.rs`.

use super::*;

/// The gate's verdict for a foreground call: the refusal text to send when
/// the call may not go on, `None` when it may. `startup_failed` says the
/// daemon fell back to defaults because its startup `Config::load()` failed
/// (bug-19-1): a ghost running on defaults is an unsandboxed command on the
/// host, so the gate fails closed there and names the load error — a
/// non-ghost keeps the fallback-defaults behaviour every request had before
/// the handle (M32 phase-01 for the sandbox arm).
fn ghost_foreground_refusal(
    is_ghost: bool,
    config: &crate::config::Config,
    startup_failed: Option<&anyhow::Error>,
) -> Option<String> {
    if is_ghost && let Some(e) = startup_failed {
        // `{e:#}` prints the whole chain: a startup load wraps the parse
        // error in a `parsing <path>` context, and `{e}` alone would show
        // only that outer layer, dropping the error the operator needs.
        return Some(format!(
            "Error: foreground execution is refused because the running \
             configuration is fallback defaults — the configuration could \
             not be read at startup: {e:#}. Fix or remove that file, then \
             restart the daemon; nothing was run."
        ));
    }
    (!ghost_may_run_foreground(is_ghost, config.sandbox.enabled)).then(|| {
        "Foreground execution is not sandboxed and is denied for ghost \
                 shells while the container sandbox is enabled. Re-issue this \
                 command with `wait: false` so it runs in a container."
            .to_string()
    })
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
            // The handle feeds both this gate and the approved run's
            // `max_per_owner`; a reload of `[sandbox]`/`[shells]` takes
            // effect at the next call. When the daemon started on fallback
            // defaults because its startup load failed, a ghost is refused
            // here (bug-19-1): the defaults carry `sandbox.enabled = false`.
            let config = crate::config::current();
            if let Some(r) =
                ghost_foreground_refusal(is_ghost, &config, crate::config::startup_error().as_ref())
            {
                return Ok(ToolCallOutcome::Result(r));
            }

            // The host is parsed **before** the approval prompt: a malformed
            // name is refused here, without asking — a name that can never be
            // a host is not a decision to put to the user. The parsed id feeds
            // both the prompt's classification and the run, so the name is
            // parsed once. An *unconfigured* well-formed name is not refused
            // here; it classifies as `new_host` and is asked (06b), then
            // connects on approval using the bare name (06c).
            let host = match resolve_host(host.as_deref()) {
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
                && let Some(msg) =
                    refuse_ghost_host(policy, &host, "run_terminal_command", session_id)
            {
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
                            max_per_owner: config.shells.max_per_owner,
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
    let owner = chat_owner(session_id);
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
            } else if crate::daemon::utils::command_has_sudo(cmd) {
                // The sudo arm: `tx` travels inside `SudoPromptArgs` for the
                // credential round trip, so the wait cannot also tick
                // keepalives on the same transport. A long sudo command can
                // still silence the client (issue #1 leaves this gap).
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
            } else {
                // Non-sudo: the run never touches the transport (`prompts:
                // None`), so dummy halves stand in for `tx`/`rx` and the real
                // writer stays free for the keepalive ticks — a long command
                // no longer runs silent past the client's silence budget.
                let mut sink = tokio::io::sink();
                let mut eof = tokio::io::BufReader::new(tokio::io::empty());
                crate::daemon::utils::with_keepalive(
                    tx,
                    shell_exec::run_in_shell_with_prompts(
                        crate::shell::registry(),
                        owner.clone(),
                        cmd,
                        max_per_owner,
                        shell.as_deref(),
                        *timeout_secs,
                        id.as_str(),
                        &mut sink,
                        &mut eof,
                        crate::host::HostId::Local,
                        |id| {
                            let (rows, cols) = crate::shell::spawn_size();
                            crate::shell::spawn_detached_host(id, &[], rows, cols)
                        },
                    ),
                )
                .await?
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
            Some(-1),
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
            Some(-1),
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
    } else if crate::daemon::utils::command_has_sudo(cmd) {
        // The sudo arm: `tx` travels inside `SudoPromptArgs` for the
        // credential round trip, so the wait cannot also tick keepalives on
        // the same transport. A long sudo command can still silence the
        // client (issue #1 leaves this gap).
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
    } else {
        // Non-sudo: the run never touches the transport (`prompts: None`), so
        // dummy halves stand in for `tx`/`rx` and the real writer stays free
        // for the keepalive ticks — a long command no longer runs silent past
        // the client's silence budget.
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::BufReader::new(tokio::io::empty());
        crate::daemon::utils::with_keepalive(
            tx,
            shell_exec::run_in_shell_with_prompts(
                crate::shell::registry(),
                owner.clone(),
                cmd,
                max_per_owner,
                shell.as_deref(),
                *timeout_secs,
                id.as_str(),
                &mut sink,
                &mut eof,
                host.clone(),
                |id| {
                    let (rows, cols) = crate::shell::spawn_size();
                    crate::shell::spawn_ssh_host(id, name, rows, cols)
                },
            ),
        )
        .await?
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
                Some(result.exit_code),
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
                Some(-1),
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

    fn refusal(
        is_ghost: bool,
        config: &crate::config::Config,
        startup_failed: Option<&anyhow::Error>,
    ) -> Option<String> {
        ghost_foreground_refusal(is_ghost, config, startup_failed)
    }

    #[test]
    fn ghost_foreground_refusal_refuses_a_ghost_when_the_sandbox_is_on() {
        let r = refusal(true, &readable(true), None);
        assert!(
            r.is_some(),
            "a ghost is refused foreground while the sandbox is enabled"
        );
    }

    #[test]
    fn ghost_foreground_refusal_allows_a_ghost_when_the_sandbox_is_off() {
        assert_eq!(
            refusal(true, &readable(false), None),
            None,
            "with the sandbox off, foreground is no worse for a ghost than any other command"
        );
    }

    /// bug-19-1: while the daemon runs on fallback defaults because its
    /// startup load failed, a ghost foreground call is refused and the
    /// refusal names the load error.
    #[test]
    fn ghost_foreground_is_refused_while_the_daemon_runs_on_fallback_defaults() {
        let e = startup_failure();
        let r = refusal(true, &readable(false), Some(&e));
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
            "refusals read as errors: {text}"
        );
    }

    /// bug-19-1: the load-error condition is ghost-only — a non-ghost call is
    /// not refused for it, keeping the fallback-defaults behaviour every
    /// request had before the handle.
    #[test]
    fn a_non_ghost_call_is_not_refused_while_the_daemon_runs_on_fallback_defaults() {
        let e = startup_failure();
        assert_eq!(
            refusal(false, &readable(false), Some(&e)),
            None,
            "a non-ghost keeps the fallback-defaults behaviour; only the ghost gate fails closed"
        );
    }

    /// The gate's load-error arm fires only on a real startup failure: with
    /// the file read successfully the sandbox arm decides, as on the happy
    /// path.
    #[test]
    fn ghost_foregate_gate_needs_no_startup_failure_to_refuse_a_ghost_when_the_sandbox_is_on() {
        let r = refusal(true, &readable(true), None);
        assert!(r.is_some(), "sandbox-on alone refuses a ghost");
    }
}

#[cfg(test)]
mod approved_path_tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    /// A real in-process PTY host for `id`: bash on a PTY, serving the id's
    /// socket under the test `HOME`. `spawn_detached_host` cannot run under a
    /// test binary, so the session's shell is opened here first and the
    /// approved run reuses it.
    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let listener = crate::shell::bind(&crate::shell::socket_path(id.as_str()))
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    /// One `run_terminal_command` call through `execute_tool_call` for
    /// session `sid`, with `reply` as everything the client sends back.
    /// Returns the frames the daemon wrote, in order.
    async fn drive(sid: &str, call: &PendingCall, reply: &str) -> Vec<Response> {
        let store = SessionStore::new();
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = std::io::Cursor::new(reply.as_bytes().to_vec());
        execute_tool_call(
            call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some(sid),
                session_name: sid,
                chat_pane: None,
                sessions: &store,
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(ScheduleStore::new_empty()),
        )
        .await
        .expect("run_terminal_command must not error");
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe");
        String::from_utf8(raw)
            .expect("the daemon writes UTF-8 frames")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Response>(line).expect("every frame is a Response"))
            .collect()
    }

    fn foreground_call(cmd: &str, host: Option<&str>, timeout_secs: u64) -> PendingCall {
        PendingCall::Foreground {
            id: "fg-83".to_string(),
            thought_signature: None,
            cmd: cmd.to_string(),
            shell: None,
            host: host.map(str::to_string),
            timeout_secs: Some(timeout_secs),
            interactive: false,
        }
    }

    fn tool_result(frames: &[Response]) -> &str {
        frames
            .iter()
            .find_map(|f| match f {
                Response::ToolResult { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a ToolResult frame: {frames:?}"))
    }

    /// #83: an approved local call runs in the session's PTY shell. The
    /// frames are the prompt, then a result carrying the command's output
    /// and naming the shell it ran in.
    #[tokio::test]
    async fn an_approved_local_command_runs_in_the_sessions_shell() {
        let (_home, _dir) = crate::test_home();
        let sid = "sess-83-run";
        let id = crate::shell::spawn_for(
            crate::shell::registry(),
            Owner::Chat(sid.to_string()),
            5,
            false,
            |id| Box::pin(spawn_in_process_host(id)),
        )
        .await
        .expect("the session's shell opens");
        let mut reply = serde_json::to_string(&Request::ToolCallResponse {
            id: "fg-83".to_string(),
            approved: true,
            user_message: None,
        })
        .expect("ToolCallResponse serialises");
        reply.push('\n');
        let frames = drive(
            sid,
            &foreground_call("echo DE-M32-83-$((40+3))", None, 20),
            &reply,
        )
        .await;
        assert!(
            matches!(
                frames.first(),
                Some(Response::ToolCallPrompt {
                    background: false,
                    ..
                })
            ),
            "the first frame asks for approval: {frames:?}"
        );
        let out = tool_result(&frames);
        assert!(
            out.contains("DE-M32-83-43"),
            "the result carries the command's output: {out}"
        );
        assert!(
            out.contains(&format!("[ran in shell {}]", id.as_str())),
            "the result names the shell it ran in: {out}"
        );
    }

    /// Issue #1 bug 2: a foreground command that runs longer than the
    /// client's 120 s silence budget must not silence the connection — the
    /// wait sends `Response::KeepAlive` frames while the command runs, so the
    /// client never abandons it and the turn is never silently dropped. One
    /// keep-alive period (15 s) plus the command's own runtime, by design.
    #[tokio::test]
    async fn a_long_foreground_command_sends_keepalives_while_it_runs() {
        let (_home, _dir) = crate::test_home();
        let sid = "sess-83-keepalive";
        crate::shell::spawn_for(
            crate::shell::registry(),
            Owner::Chat(sid.to_string()),
            5,
            false,
            |id| Box::pin(spawn_in_process_host(id)),
        )
        .await
        .expect("the session's shell opens");
        let mut reply = serde_json::to_string(&Request::ToolCallResponse {
            id: "fg-83".to_string(),
            approved: true,
            user_message: None,
        })
        .expect("ToolCallResponse serialises");
        reply.push('\n');
        let frames = drive(
            sid,
            &foreground_call("sleep 16; echo de-ka-done", None, 30),
            &reply,
        )
        .await;
        let prompt = frames
            .iter()
            .position(|f| matches!(f, Response::ToolCallPrompt { .. }))
            .expect("the command is put to the user");
        let result = frames
            .iter()
            .position(|f| matches!(f, Response::ToolResult { .. }))
            .expect("the command produces a result");
        let keepalives = frames[prompt + 1..result]
            .iter()
            .filter(|f| matches!(f, Response::KeepAlive))
            .count();
        assert!(
            keepalives >= 1,
            "at least one KeepAlive frame must lie between the ToolCallPrompt \
             and the ToolResult while a 16 s command runs: {frames:?}"
        );
        let out = tool_result(&frames);
        assert!(
            out.contains("de-ka-done"),
            "the result carries the command's output: {out}"
        );
    }

    /// #83: a malformed host name is refused before the prompt — no
    /// `ToolCallPrompt` is written, and the refusal names the rule.
    #[tokio::test]
    async fn a_malformed_host_is_refused_without_asking() {
        let (_home, _dir) = crate::test_home();
        let frames = drive(
            "sess-83-host",
            &foreground_call("true", Some("db 01"), 20),
            "",
        )
        .await;
        assert!(
            !frames
                .iter()
                .any(|f| matches!(f, Response::ToolCallPrompt { .. })),
            "a name that can never be a host is not put to the user: {frames:?}"
        );
        let out = tool_result(&frames);
        assert!(
            out.starts_with("Error: host name \"db 01\""),
            "the refusal names the host: {out}"
        );
        assert!(
            out.contains("[A-Za-z0-9._-]"),
            "the refusal names the rule: {out}"
        );
    }
}
