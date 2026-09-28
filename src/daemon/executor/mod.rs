mod bg_shell;
pub(crate) mod container;
mod file_ops;
mod foreground;
pub(crate) mod knowledge;
mod schedule;
mod shell_exec;

use crate::ai::{PendingCall, next_tool_id};
use crate::daemon::policy::GhostPolicy;
use crate::daemon::session::{SessionStore, with_sessions};
use crate::daemon::utils::send_response_split;
use crate::ipc::{PaneInfo, Request, Response};
use crate::scheduler::ScheduleStore;
use crate::shell::Owner;
use crate::tmux::cache::SessionCache;
use crate::util::UnpoisonExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Session identifiers threaded through the tool-call dispatch chain.
#[derive(Clone, Copy)]
pub struct SessionCtx<'a> {
    pub session_id: Option<&'a str>,
    pub session_name: &'a str,
    pub chat_pane: Option<&'a str>,
    pub sessions: &'a SessionStore,
}

/// Ghost shell policy context.
#[derive(Clone, Copy)]
pub(super) struct GhostCtx<'a> {
    pub policy: Option<&'a crate::daemon::policy::GhostPolicy>,
    pub is_ghost: bool,
}

struct ApprovalRequest<'a> {
    id: &'a str,
    cmd: &'a str,
    background: bool,
    target_pane_hint: Option<&'a str>,
    /// The `host` argument the call names, verbatim — `None` means the daemon
    /// host. Carried only so the prompt can classify and name the
    /// destination; the call's own host resolution is untouched.
    host: Option<&'a str>,
}

/// The outcome of a single tool call execution.
pub enum ToolCallOutcome {
    /// Normal result string to feed back to the AI.
    Result(String),
    /// The user typed a corrective message at the approval prompt.
    /// The caller must abort the current tool chain and inject this text as a
    /// new user turn so the AI can course-correct without seeing a synthetic
    /// tool error.
    UserMessage(String),
    /// A ghost shell session was created and is ready to run its turn loop.
    /// The caller (`handle_client` or `trigger_ghost_turn`) must `tokio::spawn`
    /// `trigger_ghost_turn` for the returned session ID.
    SpawnGhostSession {
        session_id: String,
        runbook_name: String,
        /// Message to return to the AI as the tool result.
        tool_result: String,
        /// Job ID of the spawned ghost (same as session_id).
        job_id: String,
    },
}

// ---------------------------------------------------------------------------
// Timing constants used by the approval gate (sub-modules have their own).
// ---------------------------------------------------------------------------

/// How long a user has to approve or deny a foreground/background tool call.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a user has to respond to a credential or write prompt (sudo password, schedule, script).
const USER_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Namespace resolution (shared by executor and prompt builder)
// ---------------------------------------------------------------------------

/// Build the memory namespace list for a session.
/// Ghost shells get agent namespace + read_namespaces + global fallback.
/// Regular sessions get only `&["global"]`.
pub fn build_memory_namespaces(
    session_id: Option<&str>,
    sessions: &SessionStore,
    is_ghost: bool,
) -> Vec<String> {
    if !is_ghost {
        return vec!["global".to_string()];
    }
    let agent_name: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.ghost_config.as_ref()?.agent.clone()
        })
    });
    let mut namespaces: Vec<String> = Vec::new();
    if let Some(name) = agent_name
        && let Ok(agent) = crate::agents::load_agent(&name)
    {
        namespaces.push(agent.memory_namespace.clone());
        for extra in &agent.read_namespaces {
            namespaces.push(extra.clone());
        }
    }
    if !namespaces.iter().any(|s| s == "global") {
        namespaces.push("global".to_string());
    }
    namespaces
}

// ---------------------------------------------------------------------------
// Main tool dispatcher
// ---------------------------------------------------------------------------

/// Everything `execute_tool_call` needs from the session entry, read in one
/// acquisition. Five separate reads of the same entry preceded this.
#[derive(Default)]
struct DispatchSnapshot {
    ghost_policy: Option<GhostPolicy>,
    tool_policy: Option<crate::agents::ToolPolicy>,
    is_ghost_shell: bool,
    spawn_depth: u8,
    effective_parent_job_id: Option<String>,
    saved_name: Option<String>,
    turn_count: usize,
}

/// Whether a ghost shell may use `send_input`.
///
/// Typing into a PTY displaces whatever the shell is doing, and no
/// `GhostPolicy` auto-approve category covers it, so the default is deny — a
/// ghost needs an explicit agent `ToolPolicy` allow. Deliberately **not**
/// routed through `prompt_and_await_approval`, whose ghost branch
/// auto-approves any non-sudo string via `GhostPolicy::is_safe`: a ghost
/// typing into the user's panes is exactly the displacement that gate exists
/// to prevent.
pub(crate) fn ghost_may_send_input(
    is_ghost: bool,
    policy: Option<&crate::agents::policy::ToolPolicy>,
) -> bool {
    !is_ghost || policy.is_some_and(|p| p.explicitly_allows("send_input")) // ghost needs an explicit allow
}

/// True when this session may run a **foreground** command.
///
/// Foreground execution is deliberately outside the container — the user is
/// present and approving it. A ghost shell has no user, so with the sandbox
/// enabled a ghost foreground command would be an unsandboxed command on the
/// host. Ghosts are background-only by design; this makes that a condition
/// rather than a line of prompt text.
pub(crate) fn ghost_may_run_foreground(is_ghost: bool, sandbox_enabled: bool) -> bool {
    !is_ghost || !sandbox_enabled // a ghost gets no unsandboxed door out
}

/// Resolve the foreground call's `host` argument **before** the approval
/// prompt.
///
/// A malformed name is refused with [`crate::host::HostId::parse`]'s own
/// message — a name that can never be a host is not a decision the user
/// should be asked to make, so it never reaches the panel. An *unconfigured*
/// well-formed name is deliberately **not** refused here: it is
/// phase-06c's `new_host` case and must reach the prompt, then connect on
/// approval using the bare name as its own ssh destination.
///
/// Returns the parsed id for the prompt's classification and the later run
/// (parsed once, not twice), or the refusal sentence for the model.
fn resolve_foreground_host(host: Option<&str>) -> Result<crate::host::HostId, String> {
    match host {
        None => Ok(crate::host::HostId::Local),
        Some(name) => crate::host::HostId::parse(name),
    }
}

/// The sentence a ghost reads when it asks for a host its runbook did not
/// grant: the refused host **and** the allowed set, so the model can correct
/// itself rather than retrying blindly.
pub(super) fn ghost_host_refusal(policy: &GhostPolicy, host: &crate::host::HostId) -> String {
    let allowed = if policy.hosts.is_empty() {
        "the runbook grants every host (its `hosts:` list is empty, which is \
         unrestricted), so this refusal should not have happened"
            .to_string()
    } else {
        format!("{} ({})", policy.hosts.join(", "), policy.hosts.len())
    };
    format!(
        "Error: this runbook's `hosts:` list does not grant host {}. \
         Hosts this runbook allows: {}. \
         Stay on the hosts the runbook names, or work on the daemon host.",
        host.as_str(),
        allowed
    )
}

/// What a shell-command failure tells the model. Both PTY arms render the
/// run error through this one helper so they cannot diverge.
///
/// A `ShellExited` in the chain is a different fact from a timeout: the
/// shell's program is gone, so **the command is not running there** and a
/// retry must aim at a live shell — `shell: "new"` or no `shell` at all —
/// rather than into the same dead one.
fn shell_run_error_text(shell_id: &str, e: &anyhow::Error) -> String {
    if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
        format!(
            "Error running command in shell {shell_id}: {e:#}. That shell has \
             exited, so the command is not running there. Omit `shell` (or pass \
             \"new\") to run it in a live shell."
        )
    } else {
        format!(
            "Error running command in shell {shell_id}: {e:#}. \
             The command may still be running in that shell — \
             do not assume it failed outright; retrying could \
             double-apply its effect."
        )
    }
}

pub async fn execute_tool_call<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    ctx: SessionCtx<'_>,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let SessionCtx {
        session_id,
        session_name: _,
        chat_pane,
        sessions,
    } = ctx;
    // ── Pre-fetch session state in one acquisition ───────────────────────────
    let snap: DispatchSnapshot = match session_id {
        Some(sid) => with_sessions(sessions, |store| match store.get(sid) {
            Some(e) => {
                let gc = e.ghost_config.as_ref();
                DispatchSnapshot {
                    ghost_policy: if e.is_ghost {
                        gc.map(GhostPolicy::from_config)
                    } else {
                        None
                    },
                    tool_policy: if e.is_ghost {
                        gc.and_then(|g| g.tool_policy.clone())
                    } else {
                        None
                    },
                    is_ghost_shell: e.is_ghost,
                    spawn_depth: gc.map(|g| g.spawn_depth).unwrap_or(0),
                    effective_parent_job_id: gc.map(|_| sid.to_string()),
                    saved_name: e.saved_name.clone(),
                    turn_count: e.turn_count,
                }
            }
            None => DispatchSnapshot::default(),
        }),
        None => DispatchSnapshot::default(),
    };
    let DispatchSnapshot {
        ghost_policy,
        tool_policy,
        is_ghost_shell,
        spawn_depth,
        effective_parent_job_id,
        saved_name,
        turn_count,
    } = snap;
    // Defensive guard: a ghost shell entry must always have a ghost_config.
    if is_ghost_shell && ghost_policy.is_none() {
        let msg = "Error: ghost shell has no policy configured (ghost_config missing in runbook frontmatter)".to_string();
        log::error!("{} for session {:?}", msg, session_id);
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: call.id().to_string(),
                output: msg.clone(),
            },
        )
        .await?;
        return Ok(ToolCallOutcome::Result(msg));
    }
    let is_ghost = ghost_policy.is_some();

    // ── Delegation depth tracking ────────────────────────────────────────────
    // (spawn_depth and effective_parent_job_id already loaded in DispatchSnapshot)

    // ── Tool Policy Enforcement (agent-level) ────────────────────────────────
    if let Some(policy) = &tool_policy
        && !policy.permits(call.tool_name())
    {
        let msg = format!(
            "Tool '{}' is not permitted for this agent.",
            call.tool_name()
        );
        log::info!(
            "Tool policy denied '{}' in session {:?}: {}",
            call.tool_name(),
            session_id,
            msg
        );
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: call.id().to_string(),
                output: msg.clone(),
            },
        )
        .await?;
        return Ok(ToolCallOutcome::Result(msg));
    }

    // ── Build namespace list for memory operations ───────────────────────────
    let memory_namespaces_owned = build_memory_namespaces(session_id, sessions, is_ghost_shell);
    let memory_namespaces: Vec<&str> = memory_namespaces_owned.iter().map(|s| s.as_str()).collect();
    // ── Artifact context for session_origin stamping + tracking ───────────────
    let artifact_ctx = knowledge::ArtifactCtx {
        session_id,
        sessions,
        saved_name: saved_name.as_deref(),
        turn_count,
        is_ghost,
        namespaces: &memory_namespaces,
    };
    // ──────────────────────────────────────────────────────────────────────────

    // Emit ToolStarted for silent tools so the client can show a history entry
    // and animate an elapsed timer.  Approval-gated and panel tools are excluded
    // because they have their own richer UI (ToolCallPrompt, EditFilePrompt, …).
    let emit_feedback = call.should_emit_tool_feedback();
    if emit_feedback {
        let _ = send_response_split(
            tx,
            Response::ToolStarted {
                id: call.id().to_string(),
                tool: call.tool_name().to_string(),
                summary: call.summary(),
            },
        )
        .await;
    }
    let tool_start = Instant::now();

    let outcome = match call {
        PendingCall::Foreground {
            id,
            cmd,
            shell,
            host,
            timeout_secs,
            ..
        } => {
            // Ghost gate — a ghost has no unsandboxed door out (M19 phase-03).
            if !ghost_may_run_foreground(
                is_ghost,
                crate::config::Config::load()
                    .unwrap_or_default()
                    .sandbox
                    .enabled,
            ) {
                return Ok(ToolCallOutcome::Result(
                    "Foreground execution is not sandboxed and is denied for ghost \
                     shells while the container sandbox is enabled. Re-issue this \
                     command with background=true so it runs in a container."
                        .to_string(),
                ));
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
            // The PTY shell substrate, the default. A ghost session or
            // `backend = "tmux"` (the opt-out) falls through to
            // `run_foreground` unchanged, so the opt-out behaviour is
            // byte-for-byte today's.
            let uses_pty = crate::config::Config::load()
                .unwrap_or_default()
                .execution
                .uses_pty();
            if uses_pty && !is_ghost && session_id.is_some() {
                match prompt_and_await_approval(
                    ApprovalRequest {
                        id,
                        cmd,
                        background: false,
                        target_pane_hint: None,
                        host: Some(host.as_str()),
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
                        let owner = Owner::Chat(session_id.unwrap_or_default().to_string());
                        let max_per_owner = crate::config::Config::load()
                            .unwrap_or_default()
                            .shells
                            .max_per_owner;
                        // `host` is already parsed above, before the prompt.
                        // The permissive lookup runs **after** approval: an
                        // unconfigured host resolves to its own ssh
                        // destination there, and only an empty-ssh half-written
                        // entry refuses — never routed through the shell-error
                        // wording, which would dress a host problem in
                        // shell-limit or timeout advice the model cannot act on.
                        let host = match &host {
                            crate::host::HostId::Local => crate::host::HostId::Local,
                            crate::host::HostId::Named(name) => {
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
                                    return Ok(ToolCallOutcome::Result(msg));
                                }
                                host.clone()
                            }
                        };
                        // The two arms spawn different programs, so their closure
                        // types differ and each calls `run_in_shell_with_prompts`
                        // itself (the same split `open_shell` makes).
                        let outcome = match &host {
                            crate::host::HostId::Local => {
                                match shell_exec::run_in_shell_with_prompts(
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
                                    |id| crate::shell::spawn_detached_host(id, &[], 24, 80),
                                )
                                .await
                                {
                                    Ok((shell_id, result)) => {
                                        crate::daemon::stats::finish_command(
                                            cmd_id,
                                            result.exit_code,
                                        );
                                        send_response_split(
                                            tx,
                                            Response::ToolResult {
                                                tool_call_id: id.to_string(),
                                                output: result.text.clone(),
                                            },
                                        )
                                        .await?;
                                        crate::daemon::utils::log_shell_command(
                                            session_id,
                                            shell_id.as_str(),
                                            cmd,
                                            "approved",
                                            result.exit_code,
                                            &result.text,
                                        );
                                        result.text
                                    }
                                    Err(e) => {
                                        let shell_id =
                                            crate::shell::registry_lookup(&owner, |id| {
                                                id.to_string()
                                            })
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
                                            cmd,
                                            "shell-error",
                                            -1,
                                            &msg,
                                        );
                                        msg
                                    }
                                }
                            }
                            crate::host::HostId::Named(name) => {
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
                                        cmd,
                                        "host-error",
                                        -1,
                                        &msg,
                                    );
                                    return Ok(ToolCallOutcome::Result(msg));
                                }
                                match shell_exec::run_in_shell_with_prompts(
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
                                    |id| crate::shell::spawn_ssh_host(id, name, 24, 80),
                                )
                                .await
                                {
                                    Ok((shell_id, result)) => {
                                        crate::daemon::stats::finish_command(
                                            cmd_id,
                                            result.exit_code,
                                        );
                                        send_response_split(
                                            tx,
                                            Response::ToolResult {
                                                tool_call_id: id.to_string(),
                                                output: result.text.clone(),
                                            },
                                        )
                                        .await?;
                                        crate::daemon::utils::log_shell_command(
                                            session_id,
                                            shell_id.as_str(),
                                            cmd,
                                            "approved",
                                            result.exit_code,
                                            &result.text,
                                        );
                                        result.text
                                    }
                                    Err(e) => {
                                        let shell_id =
                                            crate::shell::registry_lookup(&owner, |id| {
                                                id.to_string()
                                            })
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
                                            cmd,
                                            "shell-error",
                                            -1,
                                            &msg,
                                        );
                                        msg
                                    }
                                }
                            }
                        };
                        return Ok(ToolCallOutcome::Result(outcome));
                    }
                }
            }

            foreground::run_foreground(
                foreground::FgArgs { id, cmd },
                ctx,
                cache,
                GhostCtx {
                    policy: ghost_policy.as_ref(),
                    is_ghost,
                },
                tx,
                rx,
            )
            .await
        }

        PendingCall::Background { id, cmd, .. } => {
            // The PTY shell substrate, the default, for a **chat**
            // session's plain background command. A ghost session (no
            // substrate shell of its own and sandboxed away from the host)
            // or `backend = "tmux"` (the opt-out) falls through to
            // `run_background` unchanged, so the opt-out behaviour is
            // byte-for-byte today's.
            let uses_pty = crate::config::Config::load()
                .unwrap_or_default()
                .execution
                .uses_pty();
            if uses_pty && !is_ghost && session_id.is_some() {
                match prompt_and_await_approval(
                    ApprovalRequest {
                        id,
                        cmd,
                        background: true,
                        target_pane_hint: None,
                        host: None,
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
                        let owner = Owner::Chat(session_id.unwrap_or_default().to_string());
                        let max_per_owner = crate::config::Config::load()
                            .unwrap_or_default()
                            .shells
                            .max_per_owner;
                        match bg_shell::start_background(
                            crate::shell::registry(),
                            owner.clone(),
                            cmd,
                            max_per_owner,
                        )
                        .await
                        {
                            Ok((start, handle)) => {
                                // The command was dispatched; its real exit code is
                                // not known yet and the stats row is about
                                // dispatch, not outcome.
                                crate::daemon::stats::finish_command(cmd_id, 0);
                                let text = format!(
                                    "Running in background on shell {} (run {}). The result is \
                                     not available yet — it will be delivered later as a \
                                     [Background Task Completed] message. Continue with other \
                                     work or end your turn; do not poll for it.",
                                    start.shell_id.as_str(),
                                    start.run_id
                                );
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
                                    start.shell_id.as_str(),
                                    cmd,
                                    "approved",
                                    0,
                                    &text,
                                );
                                tokio::spawn(bg_shell::await_and_notify(
                                    sessions.clone(),
                                    session_id.map(|s| s.to_string()).unwrap_or_default(),
                                    cmd.to_string(),
                                    start,
                                    handle,
                                ));
                                return Ok(ToolCallOutcome::Result(text));
                            }
                            Err(e) => {
                                let shell_id =
                                    crate::shell::registry_lookup(&owner, |sid| sid.to_string())
                                        .unwrap_or_else(|| "s?".to_string());
                                let msg = format!(
                                    "Error starting background command in shell {shell_id}: \
                                     {e:#}. Nothing was started, so retrying is safe."
                                );
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
                                    cmd,
                                    "shell-error",
                                    -1,
                                    &msg,
                                );
                                return Ok(ToolCallOutcome::Result(msg));
                            }
                        }
                    }
                }
            }

            foreground::run_background(
                id,
                cmd,
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

        PendingCall::ScheduleCommand {
            id: call_id,
            name,
            command,
            is_script,
            run_at,
            interval,
            runbook,
            ghost_runbook,
            cron,
            ..
        } => {
            schedule::run_schedule_command(
                schedule::ScheduleArgs {
                    call_id,
                    name,
                    command,
                    is_script: *is_script,
                    run_at: run_at.as_deref(),
                    interval: interval.as_deref(),
                    runbook: runbook.as_deref(),
                    ghost_runbook: ghost_runbook.as_deref(),
                    cron: cron.as_deref(),
                },
                session_id,
                is_ghost,
                schedule_store,
                tx,
                rx,
            )
            .await
        }

        PendingCall::ListSchedules { .. } => schedule::list_schedules(schedule_store, tx).await,

        PendingCall::LoadTools { groups, .. } => {
            let mut loaded: Vec<String> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if members.is_empty() {
                    unknown.push(g.clone());
                } else {
                    for m in members {
                        loaded.push(m.to_string());
                    }
                }
            }
            // Persist into session state
            if let Some(sid) = session_id {
                with_sessions(sessions, |store| {
                    if let Some(entry) = store.get_mut(sid) {
                        for name in &loaded {
                            entry.loaded_tools.insert(name.clone());
                        }
                        entry.dirty = true;
                    }
                });
            }
            let mut result_parts = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if !members.is_empty() {
                    result_parts.push(format!("Loaded group(s): {} → {}", g, members.join(", ")));
                }
            }
            if !unknown.is_empty() {
                result_parts.push(format!(
                    "Unknown group(s): {} (valid groups: {})",
                    unknown.join(", "),
                    crate::ai::tools::deferred_catalog_text()
                ));
            }
            result_parts.push("They are now available; call them directly.".to_string());
            Ok(ToolCallOutcome::Result(result_parts.join(". ")))
        }

        PendingCall::CancelSchedule { job_id, .. } => Ok(ToolCallOutcome::Result(
            schedule::cancel_schedule(schedule_store, job_id, session_id),
        )),

        PendingCall::DeleteSchedule { job_id, .. } => Ok(ToolCallOutcome::Result(
            schedule::delete_schedule(schedule_store, job_id, session_id),
        )),

        PendingCall::WriteScript {
            id,
            script_name,
            content,
            ..
        } => knowledge::write_script(id, script_name, content, &artifact_ctx, tx, rx).await,

        PendingCall::ListScripts { .. } => knowledge::list_scripts(tx).await,

        PendingCall::ReadScript { script_name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_script(script_name)))
        }

        PendingCall::DeleteScript {
            id, script_name, ..
        } => knowledge::delete_script(id, script_name, is_ghost, session_id, tx, rx).await,

        PendingCall::ReadFile {
            path,
            offset,
            limit,
            pattern,
            host,
            ..
        } => {
            // A ghost reaching a host its runbook does not name is refused
            // before the run — the same boundary the foreground arm enforces.
            let host_id = match host.as_deref() {
                None => crate::host::HostId::Local,
                Some(name) => match crate::host::HostId::parse(name) {
                    Ok(parsed) => parsed,
                    Err(msg) => return Ok(ToolCallOutcome::Result(format!("Error: {msg}"))),
                },
            };
            if let Some(policy) = ghost_policy.as_ref()
                && !policy.may_use_host(&host_id)
            {
                let msg = ghost_host_refusal(policy, &host_id);
                crate::daemon::utils::log_event(
                    "ghost_host_refused",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "tool": "read_file",
                        "requested_host": host_id.as_str(),
                        "allowed_hosts": policy.hosts,
                    }),
                );
                return Ok(ToolCallOutcome::Result(msg));
            }
            file_ops::run_read_file(path, *offset, *limit, pattern.as_deref(), host.as_deref())
                .await
        }

        PendingCall::EditFile {
            id,
            path,
            operation,
            old_string,
            new_string,
            content,
            dest_path,
            host,
            ..
        } => {
            file_ops::run_edit_file(
                file_ops::EditArgs {
                    id,
                    path,
                    operation,
                    old_string: old_string.as_deref(),
                    new_string: new_string.as_deref(),
                    content: content.as_deref(),
                    dest_path: dest_path.as_deref(),
                    host: host.as_deref(),
                },
                session_id,
                GhostCtx {
                    policy: ghost_policy.as_ref(),
                    is_ghost,
                },
                tx,
                rx,
            )
            .await
        }

        PendingCall::WriteRunbook {
            id, name, content, ..
        } => knowledge::write_runbook(id, name, content, &artifact_ctx, tx, rx).await,

        PendingCall::DeleteRunbook { id, name, .. } => {
            knowledge::delete_runbook(id, name, is_ghost, session_id, schedule_store, tx, rx).await
        }

        PendingCall::ReadRunbook { name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_runbook(name)))
        }

        PendingCall::ListRunbooks { .. } => knowledge::list_runbooks(tx).await,

        PendingCall::AddMemory {
            key,
            value,
            category,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::add_memory(
            key,
            value,
            category,
            &artifact_ctx,
        ))),

        PendingCall::UpdateMemory {
            key,
            category,
            body,
            append,
            tags,
            summary,
            relates_to,
            expires,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::update_memory(
            knowledge::UpdateMemoryRequest {
                key,
                category,
                body: body.as_deref(),
                append: *append,
                tags: tags.as_deref(),
                summary: summary.as_deref(),
                relates_to: relates_to.as_deref(),
                expires: expires.as_deref(),
            },
            session_id,
            &memory_namespaces,
        ))),

        PendingCall::DeleteMemory { key, category, .. } => Ok(ToolCallOutcome::Result(
            knowledge::delete_memory(key, category, session_id, &memory_namespaces),
        )),

        PendingCall::ReadMemory { key, category, .. } => Ok(ToolCallOutcome::Result(
            knowledge::read_memory(key, category, &memory_namespaces),
        )),

        PendingCall::ListMemories { category, .. } => {
            knowledge::list_memories(category.as_deref(), &memory_namespaces, tx).await
        }

        PendingCall::SearchRepository { query, kind, .. } => Ok(ToolCallOutcome::Result(
            knowledge::search_repository(query, kind, artifact_ctx.namespaces),
        )),

        PendingCall::RecallContext {
            query,
            turn_start,
            turn_end,
            scope,
            ..
        } => {
            let result = if let Some(sid) = session_id {
                let args = crate::daemon::context::recall::RecallArgs {
                    query: query.clone(),
                    turn_start: *turn_start,
                    turn_end: *turn_end,
                    scope: scope.clone(),
                };
                let limits = crate::config::LimitsConfig::default();
                match crate::daemon::context::recall::recall(sid, &args, &limits) {
                    Ok(r) => r,
                    Err(e) => format!("Error: {}", e),
                }
            } else {
                "Error: recall_context requires a session".to_string()
            };
            Ok(ToolCallOutcome::Result(result))
        }

        PendingCall::GetShellContext { scope, .. } => {
            let breadth = match knowledge::resolve_context_scope(scope.as_deref()) {
                Ok(b) => b,
                Err(msg) => return Ok(ToolCallOutcome::Result(msg)),
            };
            if breadth == knowledge::ContextBreadth::ShellsOnly {
                return Ok(ToolCallOutcome::Result(knowledge::shells_only_context(
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                )));
            }
            let target_pane: Option<String> = session_id.and_then(|sid| {
                with_sessions(sessions, |store| {
                    store.get(sid)?.default_target_pane.clone()
                })
            });
            let ctx_scope = match breadth {
                knowledge::ContextBreadth::All => crate::tmux::cache::ContextScope::All,
                _ => crate::tmux::cache::ContextScope::Session,
            };
            let ctx = cache.get_labeled_context_scoped(chat_pane, chat_pane, ctx_scope);
            let pane_map = cache.pane_map_summary(chat_pane);
            let fg_line = target_pane
                .as_deref()
                .map(|tp| {
                    format!(
                        "[FOREGROUND TARGET] {tp} — the pane daemon-resolved foreground \
                         commands run in\n",
                    )
                })
                .unwrap_or_default();
            let shells_line = knowledge::shell_context_block(&Owner::Chat(
                session_id.unwrap_or_default().to_string(),
            ));
            Ok(ToolCallOutcome::Result(format!(
                "{shells_line}{fg_line}{ctx}\n{pane_map}"
            )))
        }

        PendingCall::ListShells { scope, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::list_shells(
                scope.as_deref(),
                &Owner::Chat(session_id.unwrap_or_default().to_string()),
            )))
        }

        PendingCall::ReadShell {
            shell,
            lines,
            since_command,
            filter,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::read_shell(
            shell,
            *lines,
            *since_command,
            filter.as_deref(),
        ))),

        PendingCall::FindInShells { pattern, scope, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::find_in_shells(
                pattern,
                scope.as_deref(),
                &Owner::Chat(session_id.unwrap_or_default().to_string()),
            )))
        }

        PendingCall::OpenShell { host, .. } => {
            // The same boundary the foreground arm enforces: a ghost may open
            // a shell on a host its runbook names only. Gating here too is
            // what closes the second door — `open_shell` is reachable on its
            // own, not only through `run_terminal_command`.
            if let Some(policy) = ghost_policy.as_ref() {
                let asked = match host.as_deref() {
                    None => crate::host::HostId::Local,
                    Some(name) => match crate::host::HostId::parse(name) {
                        Ok(parsed) => parsed,
                        Err(msg) => {
                            return Ok(ToolCallOutcome::Result(format!("Error: {msg}")));
                        }
                    },
                };
                if !policy.may_use_host(&asked) {
                    let msg = ghost_host_refusal(policy, &asked);
                    crate::daemon::utils::log_event(
                        "ghost_host_refused",
                        serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "tool": "open_shell",
                            "requested_host": asked.as_str(),
                            "allowed_hosts": policy.hosts,
                        }),
                    );
                    return Ok(ToolCallOutcome::Result(msg));
                }
            }
            Ok(ToolCallOutcome::Result(
                knowledge::open_shell(
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                    host.as_deref(),
                )
                .await,
            ))
        }

        PendingCall::CloseShell { shell, .. } => Ok(ToolCallOutcome::Result(
            knowledge::close_shell(
                shell,
                &Owner::Chat(session_id.unwrap_or_default().to_string()),
            )
            .await,
        )),

        PendingCall::WatchShell {
            shell,
            pattern,
            timeout_secs,
            ..
        } => Ok(ToolCallOutcome::Result(
            crate::daemon::utils::with_keepalive(
                tx,
                knowledge::watch_shell(shell, pattern.as_deref(), *timeout_secs),
            )
            .await?,
        )),

        PendingCall::SendInput {
            id,
            shell,
            text,
            enter,
            ..
        } => {
            // 1. Ghost gate — before any approval prompt, so a ghost is never
            //    shown a panel it cannot use. Default-deny: the agent
            //    `ToolPolicy` must *explicitly* allow `send_input`.
            if !ghost_may_send_input(is_ghost, tool_policy.as_ref()) {
                return Ok(ToolCallOutcome::Result(
                    "send_input is denied for ghost shells unless the agent's tool \
                     policy explicitly allows it."
                        .to_string(),
                ));
            }

            // 2. What the shell is doing, for the approval panel. `Idle`
            //    genuinely means "at its prompt" — a program waiting on stdin
            //    reports `Running` — so the note names the it-will-be-run-as-
            //    a-command hazard instead of refusing it.
            let target_note = match crate::shell::status(
                &crate::shell::socket_path(shell),
                std::time::Duration::from_secs(5),
            )
            .await
            {
                Ok(crate::shell::ShellResponse::Status { state, .. }) => {
                    knowledge::input_target_note(Some(state))
                }
                _ => knowledge::input_target_note(None),
            };

            // 3. Approval. `ghost_policy` is `None` on purpose: step 1 already
            //    made the ghost decision, and passing the live policy here
            //    would re-enter the helper's auto-approve branch.
            //    `target_pane_hint` is `None` because this is a PTY shell, not
            //    a tmux pane.
            let approval_cmd = format!(
                "send_input into shell {}: {:?}{} — {}",
                shell,
                text,
                if *enter { " (newline appended)" } else { "" },
                target_note
            );
            match prompt_and_await_approval(
                ApprovalRequest {
                    id: id.as_str(),
                    cmd: &approval_cmd,
                    background: false,
                    target_pane_hint: None,
                    host: None,
                },
                session_id,
                None,
                tx,
                rx,
            )
            .await?
            {
                Ok(_cmd_id) => {}
                Err(outcome) => return Ok(outcome),
            }

            // 4. Execute.
            Ok(ToolCallOutcome::Result(
                knowledge::send_input(
                    shell,
                    text,
                    *enter,
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                )
                .await,
            ))
        }

        PendingCall::ListShellLogs { since, limit, .. } => Ok(ToolCallOutcome::Result(
            knowledge::list_shell_logs(*since, *limit),
        )),

        PendingCall::ReadShellLog {
            log,
            command_index,
            lines,
            filter,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::read_shell_log(
            log,
            *command_index,
            *lines,
            filter.as_deref(),
        ))),

        PendingCall::SpawnGhost {
            runbook,
            message,
            agent,
            ..
        } => {
            if spawn_depth >= 2 {
                return Ok(ToolCallOutcome::Result(
                    "Delegation depth limit reached (max: coordinator + 1 level of specialists)."
                        .to_string(),
                ));
            }
            knowledge::spawn_ghost(
                runbook,
                message,
                agent.as_deref(),
                sessions,
                spawn_depth,
                effective_parent_job_id.as_deref(),
            )
            .await
        }

        PendingCall::CreateAgent {
            id,
            name,
            description,
            prompt,
            model,
            memory_namespace,
            max_turns,
            auto_approve_read_only,
            auto_approve_scripts,
            ..
        } => {
            knowledge::create_agent(
                knowledge::CreateAgentArgs {
                    id,
                    name,
                    description,
                    prompt,
                    model: model.as_deref(),
                    memory_namespace,
                    max_turns: *max_turns,
                    auto_approve_read_only: *auto_approve_read_only,
                    auto_approve_scripts,
                },
                &artifact_ctx,
                tx,
                rx,
            )
            .await
        }

        PendingCall::ReadAgent { name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_agent(name)))
        }

        PendingCall::ListAgents { .. } => knowledge::list_agents_tool(tx).await,

        PendingCall::DeleteAgent { id, name, .. } => {
            knowledge::delete_agent(id, name, is_ghost, session_id, tx, rx).await
        }

        PendingCall::AwaitAgentResult {
            job_id,
            agent_name,
            timeout_secs,
            ..
        } => {
            crate::daemon::utils::with_keepalive(
                tx,
                knowledge::await_agent_result(
                    job_id,
                    agent_name,
                    *timeout_secs,
                    &memory_namespaces,
                ),
            )
            .await?
        }

        PendingCall::RunLogTail { run_id, limit, .. } => {
            let result = match crate::daemon::run::RunId::parse(run_id) {
                Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
                Ok(id) => crate::daemon::run::run_log_tail(&id, *limit),
            };
            Ok(ToolCallOutcome::Result(result))
        }

        PendingCall::RunLogSearch {
            run_id,
            query,
            limit,
            ..
        } => {
            let result = match crate::daemon::run::RunId::parse(run_id) {
                Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
                Ok(id) => crate::daemon::run::run_log_search(&id, query, *limit),
            };
            Ok(ToolCallOutcome::Result(result))
        }

        PendingCall::GetTurn { run_id, turn, .. } => {
            let result = match crate::daemon::run::RunId::parse(run_id) {
                Err(e) => crate::daemon::run::run_log_input_rejected(&e.to_string()),
                Ok(id) => crate::daemon::run::get_turn(&id, *turn),
            };
            Ok(ToolCallOutcome::Result(result))
        }
    }?;

    if emit_feedback {
        let (ok, detail) = tool_outcome_detail(&outcome);
        let _ = send_response_split(
            tx,
            Response::ToolFinished {
                id: call.id().to_string(),
                ok,
                elapsed_ms: tool_start.elapsed().as_millis() as u64,
                detail,
            },
        )
        .await;
    }

    Ok(outcome)
}

/// Extract a human-readable result summary from a tool outcome.
fn tool_outcome_detail(outcome: &ToolCallOutcome) -> (bool, Option<String>) {
    match outcome {
        ToolCallOutcome::UserMessage(_) => (false, Some("redirected".into())),
        ToolCallOutcome::SpawnGhostSession { .. } => (true, Some("ghost spawned".into())),
        ToolCallOutcome::Result(s) => {
            let ok = !s.starts_with("Error:") && !s.starts_with("error:");
            let n = s.lines().count();
            let detail = if n > 1 {
                Some(format!("{n} lines"))
            } else {
                None
            };
            (ok, detail)
        }
    }
}

// ---------------------------------------------------------------------------
// Approval gate — shared by foreground, background, and write-operations.
// ---------------------------------------------------------------------------

/// Send a `ToolCallPrompt` to the client, wait up to [`APPROVAL_TIMEOUT`] for
/// the user's [`Request::ToolCallResponse`], and log the outcome.
///
/// Returns `Ok(Ok(cmd_id))` when the user approves.
/// Returns `Ok(Err(ToolCallOutcome::Result(msg)))` when the user denies or
/// the wait times out — the caller should propagate this as the tool result.
/// Returns `Ok(Err(ToolCallOutcome::UserMessage(text)))` when the user typed
/// a corrective message; the caller should abort the tool chain and inject the
/// text as a new user turn.
/// Returns `Err` on connection EOF.
async fn prompt_and_await_approval<W, R>(
    req: ApprovalRequest<'_>,
    session_id: Option<&str>,
    ghost_policy: Option<&GhostPolicy>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<Result<usize, ToolCallOutcome>>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let ApprovalRequest {
        id,
        cmd,
        background,
        target_pane_hint,
        host,
    } = req;
    let mode = if background {
        "background"
    } else {
        "foreground"
    };
    // The configured host names, read once for the classification. On a load
    // error the map is empty, so every named host classifies as `new_host` —
    // the honest class for "I could not read the host table".
    let known_hosts: Vec<String> = crate::config::Config::load()
        .unwrap_or_default()
        .hosts
        .keys()
        .cloned()
        .collect();

    // ── Ghost Shell Logic ──────────────────────────────────────────────────
    if let Some(policy) = ghost_policy {
        if policy.is_safe(cmd) {
            log::info!("Ghost Shell auto-approved {}: {}", mode, cmd);
            if background {
                crate::daemon::stats::inc_commands_bg_approved();
            } else {
                crate::daemon::stats::inc_commands_fg_approved();
            }
            let cmd_id = crate::daemon::stats::start_command(cmd, mode);
            if cmd.contains(".daemoneye/scripts/") {
                crate::daemon::stats::inc_scripts_executed();
            }
            crate::daemon::utils::log_event(
                "command_approval",
                serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": mode,
                    "cmd": cmd,
                    "decision": "ghost_auto_approved",
                }),
            );
            return Ok(Ok(cmd_id));
        } else {
            log::info!(
                "Ghost Shell auto-denied (sudo command not on whitelist): {} — whitelist={:?} run_with_sudo={}",
                cmd,
                policy.auto_approve_scripts,
                policy.run_with_sudo,
            );
            let msg = format!(
                "Command denied by Ghost Policy (sudo requires a pre-approved script via install-sudoers): {}",
                cmd
            );
            if background {
                crate::daemon::stats::inc_commands_bg_denied();
            } else {
                crate::daemon::stats::inc_commands_fg_denied();
            }
            crate::daemon::utils::log_event(
                "command_approval",
                serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": mode,
                    "cmd": cmd,
                    "decision": "ghost_auto_denied",
                }),
            );
            crate::daemon::utils::log_command(session_id, mode, "", cmd, "ghost_denied", "");
            return Ok(Err(ToolCallOutcome::Result(msg)));
        }
    }
    // ──────────────────────────────────────────────────────────────────────────

    send_response_split(
        tx,
        Response::ToolCallPrompt {
            id: id.to_string(),
            command: cmd.to_string(),
            background,
            target_pane: target_pane_hint.map(|s| s.to_string()),
            classification: crate::ipc::classify(host, ghost_policy.is_some(), &known_hosts),
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(APPROVAL_TIMEOUT, rx.read_line(&mut line)).await;

    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }

    let timed_out = read_result.is_err();

    enum Parsed {
        Approved,
        Denied,
        UserMessage(String),
    }
    let parsed = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ToolCallResponse {
                id: resp_id,
                approved,
                user_message,
            }) if resp_id == id => {
                if let Some(msg) = user_message {
                    Parsed::UserMessage(msg)
                } else if approved {
                    Parsed::Approved
                } else {
                    Parsed::Denied
                }
            }
            _ => Parsed::Denied,
        },
        _ => Parsed::Denied,
    };

    match parsed {
        Parsed::Approved => {
            log::info!("{} command approved: {}", mode, cmd);
            if background {
                crate::daemon::stats::inc_commands_bg_approved();
            } else {
                crate::daemon::stats::inc_commands_fg_approved();
            }
            let cmd_id = crate::daemon::stats::start_command(cmd, mode);
            if cmd.contains(".daemoneye/scripts/") {
                crate::daemon::stats::inc_scripts_executed();
            }
            crate::daemon::utils::log_event(
                "command_approval",
                serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": mode,
                    "cmd": cmd,
                    "decision": "approved",
                }),
            );
            Ok(Ok(cmd_id))
        }
        Parsed::Denied => {
            let decision = if timed_out { "timeout" } else { "denied" };
            if background {
                crate::daemon::stats::inc_commands_bg_denied();
            } else {
                crate::daemon::stats::inc_commands_fg_denied();
            }
            log::info!("{} command {}: {}", mode, decision, cmd);
            crate::daemon::utils::log_event(
                "command_approval",
                serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": mode,
                    "cmd": cmd,
                    "decision": decision,
                }),
            );
            crate::daemon::utils::log_command(session_id, mode, "", cmd, decision, "");
            let msg = if timed_out {
                let notice = format!(
                    "Approval prompt timed out after {} s — the command was not executed. \
                     You can re-run the request if you still want it.",
                    APPROVAL_TIMEOUT.as_secs()
                );
                let _ = send_response_split(tx, Response::SystemMsg(notice.clone())).await;
                notice
            } else {
                "User denied execution".to_string()
            };
            Ok(Err(ToolCallOutcome::Result(msg)))
        }
        Parsed::UserMessage(text) => {
            log::info!("{} command redirected by user message: {}", mode, cmd);
            crate::daemon::utils::log_event(
                "command_approval",
                serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": mode,
                    "cmd": cmd,
                    "decision": "user_message",
                }),
            );
            Ok(Err(ToolCallOutcome::UserMessage(text)))
        }
    }
}

// ---------------------------------------------------------------------------
// Pane selection — shared by the foreground execution path.
// ---------------------------------------------------------------------------

/// Build the user-facing pane-selection list, excluding the chat pane (a command
/// must never be offered to run in the conversation pane). Sorted by pane id for a
/// stable display order.
fn exclude_chat_pane(mut panes: Vec<PaneInfo>, chat_pane: Option<&str>) -> Vec<PaneInfo> {
    panes.retain(|p| chat_pane != Some(p.id.as_str()));
    panes.sort_by(|a, b| a.id.cmp(&b.id));
    panes
}

async fn find_best_target_pane<W, R>(
    chat_pane: Option<&str>,
    cache: &Arc<SessionCache>,
    sessions: &SessionStore,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    // Check for a user-selected default target pane in the session.
    let default_target: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.default_target_pane.clone()
        })
    });
    if let Some(dtp) = default_target
        && chat_pane != Some(dtp.as_str())
        && cache.is_home_pane(&dtp)
    {
        return Ok(dtp);
    }

    let home = cache.session_name.read().unwrap_or_log().clone();
    let pane_list: Vec<PaneInfo> = {
        let panes = cache.panes.read().unwrap_or_log();
        let raw: Vec<PaneInfo> = panes
            .iter()
            .filter(|(_, state)| state.session_name == home)
            .map(|(pid, state)| PaneInfo {
                id: pid.clone(),
                idx: state.pane_index,
                window: state.window_name.clone(),
                session: state.session_name.clone(),
                cmd: state.current_cmd.clone(),
                cwd: state.current_path.clone(),
                title: crate::ai::filter::mask_sensitive(&state.pane_title),
                status: state.status.to_string(),
                activity_age_secs: None,
                is_target: false,
                preview: crate::ai::filter::mask_sensitive(&crate::tmux::status::summarize(
                    state.status,
                    &state.buffer,
                )),
            })
            .collect();
        exclude_chat_pane(raw, chat_pane)
    };

    if pane_list.is_empty() {
        send_response_split(tx, Response::Error("No tmux panes available".to_string())).await?;
        return Err(anyhow::anyhow!("No active pane found."));
    }

    let prompt_id = next_tool_id();
    send_response_split(
        tx,
        Response::PaneSelectPrompt {
            id: prompt_id.clone(),
            panes: pane_list,
        },
    )
    .await?;

    let mut pane_line = String::new();
    match tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut pane_line)).await {
        Ok(r) => {
            r?;
        }
        Err(_elapsed) => {
            send_response_split(
                tx,
                Response::Error("Pane selection timed out after 120s".to_string()),
            )
            .await?;
            return Err(anyhow::anyhow!("Pane selection timed out after 120s"));
        }
    };
    match serde_json::from_str::<Request>(pane_line.trim()) {
        Ok(Request::PaneSelectResponse { pane_id, .. }) => {
            if let Some(sid) = session_id {
                with_sessions(sessions, |store| {
                    if let Some(entry) = store.get_mut(sid) {
                        entry.default_target_pane = Some(pane_id.clone());
                    }
                });
            }
            Ok(pane_id)
        }
        _ => {
            send_response_split(
                tx,
                Response::Error("Expected PaneSelectResponse".to_string()),
            )
            .await?;
            Err(anyhow::anyhow!("User aborted or invalid response"))
        }
    }
}

#[cfg(test)]
mod send_input_gate_tests {
    use super::ghost_may_send_input;
    use crate::agents::policy::ToolPolicy;

    #[test]
    fn ghost_may_send_input_requires_an_explicit_allow() {
        assert!(
            ghost_may_send_input(false, None),
            "non-ghost needs no policy"
        );
        assert!(
            !ghost_may_send_input(true, None),
            "a ghost with no policy is denied"
        );
        let deny_list = ToolPolicy {
            allow: None,
            deny: Some(vec!["read_shell".to_string()]),
        };
        // The tool is permitted by the policy, but not *explicitly allowed* —
        // `permits` would get this case wrong.
        assert!(deny_list.permits("send_input"));
        assert!(!ghost_may_send_input(true, Some(&deny_list)));
        let allow_list = ToolPolicy {
            allow: Some(vec!["send_input".to_string()]),
            deny: None,
        };
        assert!(ghost_may_send_input(true, Some(&allow_list)));
    }
}

#[cfg(test)]
mod ghost_foreground_gate_tests {
    use super::ghost_may_run_foreground;

    #[test]
    fn ghost_may_run_foreground_allows_non_ghosts_with_the_sandbox_on() {
        assert!(ghost_may_run_foreground(false, true));
    }

    #[test]
    fn ghost_may_run_foreground_allows_non_ghosts_with_the_sandbox_off() {
        assert!(ghost_may_run_foreground(false, false));
    }

    #[test]
    fn ghost_may_run_foreground_allows_ghosts_when_the_sandbox_is_off() {
        // Nothing is containerized in this configuration, so foreground
        // execution is no worse for a ghost than any other command.
        assert!(ghost_may_run_foreground(true, false));
    }

    #[test]
    fn ghost_may_run_foreground_denies_ghosts_when_the_sandbox_is_on() {
        assert!(!ghost_may_run_foreground(true, true));
    }
}

#[cfg(test)]
mod host_gate_tests {
    use super::resolve_foreground_host;

    // ── resolve_foreground_host ordering (M21 phase-06c) ────────────────────

    #[test]
    fn a_malformed_host_is_refused_without_prompting() {
        let err = resolve_foreground_host(Some("bad host!"))
            .expect_err("a name that can never be a host is refused before any prompt");
        assert!(
            err.contains("bad host!"),
            "the refusal echoes the name the model sent: {err}"
        );
        assert!(
            err.contains("[A-Za-z0-9._-]"),
            "the refusal is HostId::parse's own shape message: {err}"
        );
    }

    #[test]
    fn a_malformed_host_refusal_names_the_character_class() {
        for bad in ["bad host!", "", "web/01", "../etc"] {
            let err =
                resolve_foreground_host(Some(bad)).expect_err("every malformed name is refused");
            assert!(
                err.contains("A-Za-z0-9._-"),
                "the refusal must carry the accepted character class, not shell-limit or \
                 timeout advice: {err}"
            );
            assert!(
                !err.contains("per-owner shell limit") && !err.contains("timed out"),
                "the refusal is a host-shape sentence only: {err}"
            );
        }
    }

    #[test]
    fn an_unconfigured_host_is_not_refused_before_the_prompt() {
        // The whole point of the row: a well-formed name `[hosts]` does not
        // configure must pass this gate and reach the panel. (Whether it is in
        // `[hosts]` is the classification's question, not the parser's.)
        let parsed = resolve_foreground_host(Some("nope"))
            .expect("an unconfigured well-formed host is not refused here");
        assert_eq!(
            parsed,
            crate::host::HostId::Named("nope".to_string()),
            "the parsed id is carried on, so the name is parsed once"
        );
    }

    #[test]
    fn an_omitted_host_resolves_to_local_before_the_prompt() {
        let parsed = resolve_foreground_host(None).expect("no host argument is the daemon host");
        assert_eq!(parsed, crate::host::HostId::Local);
    }
}

#[cfg(test)]
mod ghost_host_gate_tests {
    use super::{ghost_host_refusal, resolve_foreground_host};
    use crate::daemon::policy::GhostPolicy;

    fn ghost_with_hosts(hosts: &[&str]) -> GhostPolicy {
        GhostPolicy {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        }
    }

    /// The refusal sentence names the host the ghost asked for **and** the
    /// hosts the runbook allows — the model needs both to correct itself
    /// instead of retrying blindly.
    #[test]
    fn a_ghost_naming_an_unlisted_host_is_refused_with_both_lists() {
        let policy = ghost_with_hosts(&["pinky"]);
        let host = resolve_foreground_host(Some("web01")).expect("web01 is well-formed");
        let msg = ghost_host_refusal(&policy, &host);
        assert!(
            msg.contains("web01"),
            "the refusal must name the requested host: {msg}"
        );
        assert!(
            msg.contains("pinky"),
            "the refusal must name the allowed host(s): {msg}"
        );
        assert!(
            msg.starts_with("Error:"),
            "a refusal is error text the model reads as a failed call: {msg}"
        );
    }

    /// A chat session has no ghost policy at all, so the gate must be a
    /// no-op for it — both the Foreground and OpenShell doors consult the
    /// policy only when one exists.
    #[test]
    fn a_non_ghost_session_is_not_gated_by_hosts() {
        // `execute_tool_call` derives `ghost_policy` from the session entry:
        // a non-ghost session yields `None`, and every gate below is behind
        // `if let Some(policy) = ghost_policy.as_ref()`. That condition,
        // evaluated at the seam the dispatcher consults, is what "not gated"
        // means here: with no policy the gate expression is false for every
        // host, so a chat session reaches any host untouched.
        let policy: Option<&GhostPolicy> = None;
        let gate_fires = policy
            .is_some_and(|p| !p.may_use_host(&crate::host::HostId::Named("any-host".to_string())));
        assert!(
            !gate_fires,
            "no policy means no gate: a chat session reaches any host"
        );
        // Even a ghost whose runbook grants nothing named is unrestricted.
        let unrestricted = ghost_with_hosts(&[]);
        assert!(
            unrestricted.may_use_host(&crate::host::HostId::Named("any-host".to_string())),
            "an empty hosts list stays unrestricted for the gate too"
        );
    }

    /// The common case: a ghost working on the daemon host is untouched by
    /// the boundary, however restrictive its `hosts:` list is.
    #[test]
    fn a_ghost_may_still_use_the_daemon_host() {
        let policy = ghost_with_hosts(&["pinky"]);
        let local = resolve_foreground_host(None).expect("an omitted host is local");
        assert_eq!(local, crate::host::HostId::Local);
        assert!(
            policy.may_use_host(&local),
            "the daemon host is never a hosts: question: a ghost running local \
             commands is the default ghost workflow"
        );
    }

    /// The "recorded" half: the refusal reaches the event log with the
    /// requested host in its fields. `log_event` is fire-and-forget (it
    /// swallows write errors by design), so the test drives the same
    /// helper the gate calls and asserts the segment file gained a record.
    #[test]
    fn an_unlisted_host_refusal_is_recorded() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        crate::config::Config::ensure_dirs().ok();

        let policy = ghost_with_hosts(&["pinky"]);
        let host = resolve_foreground_host(Some("web01")).expect("web01 is well-formed");
        let msg = ghost_host_refusal(&policy, &host);
        crate::daemon::utils::log_event(
            "ghost_host_refused",
            serde_json::json!({
                "session": "ghost-test",
                "tool": "run_terminal_command",
                "requested_host": host.as_str(),
                "allowed_hosts": policy.hosts,
            }),
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("the event segment was written");
        let found = content.lines().any(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return false;
            };
            v.get("event").and_then(|e| e.as_str()) == Some("ghost_host_refused")
                && v.get("requested_host").and_then(|h| h.as_str()) == Some("web01")
                && v.get("allowed_hosts").is_some()
        });
        assert!(
            found,
            "the refusal must be recorded naming the requested host and the              allowed list — segment:\n{content}"
        );
        assert!(
            msg.contains("web01"),
            "the refusal text is the model-visible half: {msg}"
        );

        match saved_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_non_ghost_is_global_only() {
        let store: SessionStore = SessionStore::new();
        let ns = build_memory_namespaces(Some("any-sid"), &store, false);
        assert_eq!(ns, vec!["global".to_string()]);
    }

    #[test]
    fn excludes_chat_pane_from_selection() {
        let panes = vec![
            PaneInfo {
                id: "%1".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            PaneInfo {
                id: "%2".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            PaneInfo {
                id: "%3".into(),
                idx: 2,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
        ];
        let result = exclude_chat_pane(panes, Some("%2"));
        let ids: Vec<&str> = result.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["%1", "%3"]);
    }

    #[test]
    fn keeps_all_panes_when_no_chat_pane() {
        let panes = vec![
            PaneInfo {
                id: "%1".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            PaneInfo {
                id: "%2".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            PaneInfo {
                id: "%3".into(),
                idx: 2,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
        ];
        let result = exclude_chat_pane(panes, None);
        let ids: Vec<&str> = result.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["%1", "%2", "%3"]);
    }

    #[test]
    fn exclude_chat_pane_sorts_by_id() {
        let panes = vec![
            PaneInfo {
                id: "%3".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            PaneInfo {
                id: "%1".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            PaneInfo {
                id: "%2".into(),
                idx: 2,
                window: "main".into(),
                session: "de".into(),
                cmd: String::new(),
                cwd: String::new(),
                title: String::new(),
                status: String::new(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
        ];
        let result = exclude_chat_pane(panes, None);
        let ids: Vec<&str> = result.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["%1", "%2", "%3"]);
    }

    #[test]
    fn namespaces_ghost_excludes_foreign_namespace() {
        use crate::agents::AgentConfig;
        use crate::daemon::session::SessionEntry;
        use std::collections::HashMap;

        let _guard = crate::test_home_guard();
        let tmp = std::env::temp_dir().join(format!("de_ns_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp);
        }

        crate::agents::save_agent(&AgentConfig {
            name: "analyst".to_string(),
            description: String::new(),
            prompt: String::new(),
            model: None,
            memory_namespace: "analyst".to_string(),
            max_turns: None,
            auto_approve_read_only: false,
            auto_approve_scripts: Vec::new(),
            read_namespaces: vec!["shared".to_string()],
            tools: None,
        })
        .unwrap();

        let entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: true,
            ghost_config: Some(crate::ipc::GhostConfig {
                agent: Some("analyst".to_string()),
                ..Default::default()
            }),
            ghost_bg_prefix: "",
            ghost_runbook_name: None,
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };
        let store: SessionStore = SessionStore::new();
        with_sessions(&store, |s| {
            s.insert("gsid".to_string(), entry);
        });

        let ns = build_memory_namespaces(Some("gsid"), &store, true);

        match old {
            Some(v) => unsafe {
                std::env::set_var("HOME", v);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(
            ns.iter().any(|n| n == "analyst"),
            "own namespace present: {ns:?}"
        );
        assert!(
            ns.iter().any(|n| n == "shared"),
            "declared read_namespace present: {ns:?}"
        );
        assert!(
            ns.iter().any(|n| n == "global"),
            "global fallback present: {ns:?}"
        );
        assert!(
            !ns.iter().any(|n| n == "victim"),
            "agent must NOT reach an unrelated namespace it was never granted: {ns:?}"
        );
    }

    #[test]
    fn build_memory_namespaces_does_not_hold_the_lock_across_load_agent() {
        use crate::agents::AgentConfig;
        use crate::daemon::session::SessionEntry;
        use std::collections::HashMap;

        let _guard = crate::test_home_guard();
        let tmp = std::env::temp_dir().join(format!("de_lock_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &tmp);
        }

        crate::agents::save_agent(&AgentConfig {
            name: "test-agent".to_string(),
            description: String::new(),
            prompt: String::new(),
            model: None,
            memory_namespace: "test-agent-ns".to_string(),
            max_turns: None,
            auto_approve_read_only: false,
            auto_approve_scripts: Vec::new(),
            read_namespaces: vec![],
            tools: None,
        })
        .unwrap();

        let entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: true,
            ghost_config: Some(crate::ipc::GhostConfig {
                agent: Some("test-agent".to_string()),
                ..Default::default()
            }),
            ghost_bg_prefix: "",
            ghost_runbook_name: None,
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };
        let store: SessionStore = SessionStore::new();
        with_sessions(&store, |s| {
            s.insert("gsid".to_string(), entry);
        });

        let ns = build_memory_namespaces(Some("gsid"), &store, true);

        // The lock must be released after the call — try_lock must succeed.
        assert!(
            store.try_lock().is_ok(),
            "store should be immediately lockable after build_memory_namespaces returns"
        );

        // Result still contains "global" as the fallback.
        assert!(
            ns.iter().any(|n| n == "global"),
            "global fallback present: {ns:?}"
        );

        match old {
            Some(v) => unsafe {
                std::env::set_var("HOME", v);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── shell-exit detection (M21 phase-08b) ─────────────────────────────────

    /// The model must never be told a command "may still be running" when the
    /// shell it was sent to has exited — that caveat would invite a retry into
    /// a dead shell. The timeout wording keeps it: for a real timeout the
    /// command genuinely may still be running.
    #[test]
    fn a_dead_shell_error_does_not_say_the_command_may_still_be_running() {
        let exited = anyhow::Error::new(crate::shell::ShellExited {
            cmd: "echo hi".to_string(),
        });
        let text = shell_run_error_text("s7", &exited);
        assert!(
            text.contains("s7"),
            "the text names the shell the command was sent to: {text}"
        );
        assert!(text.contains("exited"), "the text says exited: {text}");
        assert!(
            !text.contains("may still be running"),
            "a dead shell runs nothing; the caveat must be gone: {text}"
        );
        assert!(
            text.contains("shell"),
            "the text points at the shell to retry in: {text}"
        );

        let timeout =
            anyhow::anyhow!("timed out after 45s waiting for command output: \"sleep 60\"");
        let text = shell_run_error_text("s7", &timeout);
        assert!(
            text.contains("may still be running"),
            "a real timeout keeps the may-still-be-running caveat: {text}"
        );
        assert!(
            !text.contains("has exited"),
            "a real timeout is not an exit: {text}"
        );
    }
}
