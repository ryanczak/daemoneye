mod bg_shell;
pub(crate) mod chat_sandbox;
mod dispatch_artifacts;
mod dispatch_background;
mod dispatch_context;
mod dispatch_files;
mod dispatch_foreground;
mod dispatch_memory;
mod dispatch_meta;
mod dispatch_open_shell;
mod dispatch_runs;
mod dispatch_schedule;
mod dispatch_send_input;
mod dispatch_shells;
mod file_ops;
mod foreground;
pub(crate) mod ghost_remote;
pub(crate) mod ghost_sandbox;
pub(crate) mod knowledge;
mod route;
pub mod sbx;
pub mod sbx_lifecycle;
mod schedule;
pub(crate) mod shell_exec;

use crate::ai::PendingCall;
use crate::daemon::policy::GhostPolicy;
use crate::daemon::session::{SessionStore, with_sessions};
use crate::daemon::utils::send_response_split;
use crate::ipc::{APPROVAL_TIMEOUT, Request, Response};
use crate::scheduler::ScheduleStore;
use crate::shell::Owner;
use crate::tmux::cache::SessionCache;
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
    /// The user-owned shell (`/shell`) the call is aimed at, if any. Such a
    /// prompt is classified `UserShell`, which no session grant covers.
    user_shell: Option<&'a str>,
    /// The command is a launch (`interactive: true` or an auto-upgraded
    /// recognised interactive command): it keeps the shell and nothing waits
    /// for it. Display-only — the panel names the fact; classification and
    /// the ghost `is_safe` check read the command string alone.
    interactive: bool,
}

/// The classification an approval prompt carries: `UserShell` when the call
/// is aimed at a shell the user owns, otherwise `ipc::classify`'s answer.
fn prompt_classification(
    host: Option<&str>,
    is_ghost: bool,
    known_hosts: &[String],
    user_shell: Option<&str>,
) -> crate::ipc::Classification {
    match user_shell {
        Some(shell) => crate::ipc::Classification::UserShell {
            shell: shell.to_string(),
        },
        None => crate::ipc::classify(host, is_ghost, known_hosts),
    }
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

impl ToolCallOutcome {
    /// Whether this outcome is a successful tool result — the one success
    /// rule every tally feeds from. `None` when the outcome is not a tool
    /// result at all: a `UserMessage` redirect aborts the chain and is not
    /// a call the tool surface answered, so it counts nothing.
    ///
    /// The `Error:` test is a **prefix** test. A result that merely contains
    /// the substring — `grep: found "Error: x"` — is a success; only a
    /// result that *is* an error line is a failure.
    pub fn succeeded(&self) -> Option<bool> {
        match self {
            ToolCallOutcome::Result(s) => {
                Some(!s.starts_with("Error:") && !s.starts_with("error:"))
            }
            ToolCallOutcome::SpawnGhostSession { .. } => Some(true),
            ToolCallOutcome::UserMessage(_) => None,
        }
    }
}

/// Feed one executed tool call's outcome into the run's tally: one tool
/// call, one success when [`ToolCallOutcome::succeeded`] says it succeeded,
/// and one parse failure when the call was refused for malformed arguments
/// (`PendingCall::InvalidArgs`). A non-tool outcome — the `UserMessage`
/// redirect — counts nothing at all.
///
/// Only an outcome that came back from `execute_tool_call` belongs here: a
/// call the caps refused **before** dispatching never reaches the executor
/// and must not be tallied.
pub fn tally_tool_outcome(
    m: &mut crate::daemon::run::RunMetrics,
    call: &PendingCall,
    outcome: &ToolCallOutcome,
) {
    let Some(ok) = outcome.succeeded() else {
        return;
    };
    m.tool_calls = m.tool_calls.saturating_add(1);
    if ok {
        m.tool_successes = m.tool_successes.saturating_add(1);
    }
    if matches!(call, PendingCall::InvalidArgs { .. }) {
        m.parse_failures = m.parse_failures.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Timing constants used by the approval gate (sub-modules have their own).
// ---------------------------------------------------------------------------

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

/// Whether a ghost may type into a shell held by `fg`.
///
/// A ghost that answered "yes" is allowed at a prompt always, and into a
/// running program only when its runbook opted in with `allow_tui_input`:
/// there the bytes land inside whatever holds the shell, which is exactly
/// what the runbook author asked for. The `Foreground` argument is what the
/// live probe classified, so the gate follows the terminal's actual state
/// at the moment of the call, not a cached one.
pub(crate) fn ghost_may_type_into(fg: &crate::shell::Foreground, allow_tui_input: bool) -> bool {
    match fg {
        crate::shell::Foreground::Prompt => true,
        crate::shell::Foreground::Program { .. } => allow_tui_input,
        // A paused job is not reading input, an exited shell is gone and an
        // unknown state may be either; the opt-in does not reach these, so
        // the answer stays no.
        crate::shell::Foreground::Paused { .. }
        | crate::shell::Foreground::Exited
        | crate::shell::Foreground::Unknown => false,
    }
}

/// The tool result when the ghost gate refuses: it names the shell, what
/// holds it, and that nothing was sent. Deliberately informative — the ghost
/// reads the `Foreground` wording and can answer with `list_shells` or a
/// fresh shell rather than retrying blind. For a `Program` foreground the
/// refusal names the runbook field that lifts it; for `Paused`, `Exited`
/// and `Unknown` it does not, because no field does.
pub(crate) fn ghost_input_refusal(shell: &str, fg: &crate::shell::Foreground) -> String {
    let lift = match fg {
        crate::shell::Foreground::Program { .. } => {
            " A runbook that sets `allow_tui_input: true` may type into a \
             running program."
        }
        _ => "",
    };
    format!(
        "Error: shell {shell} is {fg}, not a shell at its prompt. A ghost may \
         type only into a shell at its prompt; typing here would reach whatever \
         holds the shell now, and nothing was sent. Use list_shells to find a \
         shell at its prompt, or open one with open_shell.{lift}"
    )
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
fn ghost_host_refusal(policy: &GhostPolicy, host: &crate::host::HostId) -> String {
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

/// Connect first for a named host; `None`, `"local"` and a malformed name
/// pass through untouched — the tool itself refuses a malformed one with
/// its existing wording.
async fn connect_named_host<W, R>(
    host: Option<&str>,
    batch: bool,
    id: &str,
    tx: &mut W,
    rx: &mut R,
) -> Result<(), String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let Ok(crate::host::HostId::Named(name)) = host
        .map(crate::host::HostId::parse)
        .unwrap_or(Ok(crate::host::HostId::Local))
    else {
        return Ok(());
    };
    shell_exec::ensure_connected(&name, batch, id, tx, rx).await
}

/// Does the `edit_file` arm run its connect step? `edit_file` refuses every
/// ghost **inside** the operation ("cannot run in a Ghost Shell"), so a
/// ghost's call must never reach the connect step — it would open a
/// BatchMode ControlMaster to a host its runbook never named before being
/// refused. Chat (`is_ghost = false`) with a well-formed name connects
/// first; `None`, `"local"` and a malformed name pass through (the
/// operation refuses a malformed one with its own wording).
fn edit_file_connects_first(is_ghost: bool, host: Option<&str>) -> bool {
    !is_ghost
        && matches!(
            host.map(crate::host::HostId::parse),
            Some(Ok(crate::host::HostId::Named(_)))
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
    let chain = error_chain(e);
    if e.chain().any(|c| c.is::<crate::shell::PromptGivenUp>()) {
        format!(
            "Error running command in shell {shell_id}: {chain}. The password prompt \
             was cancelled with Ctrl-C, so the command did not run with elevated \
             privileges. Fix what the error names before retrying."
        )
    } else if e.chain().any(|c| c.is::<shell_exec::RefusedBeforeWrite>()) {
        // The pre-write refusals raised by the run path carry their own
        // `Error: ` prefix (F10); the cap refusal raised by `registry.rs`
        // does not. Strip exactly one leading one so the sentence reads
        // `Error: …` once, whichever refusal produced it.
        let chain = chain.strip_prefix("Error: ").unwrap_or(&chain).to_string();
        format!(
            "Error: {chain}. Nothing was written, so it is safe to act on the \
             message above."
        )
    } else if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
        format!(
            "Error running command in shell {shell_id}: {chain}. That shell has \
             exited, so the command is not running there. Omit `shell` (or pass \
             \"new\") to run it in a live shell."
        )
    } else {
        format!(
            "Error running command in shell {shell_id}: {chain}. \
             The command may still be running in that shell — \
             do not assume it failed outright; retrying could \
             double-apply its effect."
        )
    }
}

/// An error chain as one sentence fragment: `{e:#}` with any trailing full
/// stop (and whitespace) removed, so a caller can append `. <guidance>`
/// without printing `..` when the innermost reason already ended a sentence.
fn error_chain(e: &anyhow::Error) -> String {
    let chain = format!("{e:#}");
    chain.trim_end().trim_end_matches('.').to_string()
}

/// Append the shell the command ran in to a waited `run_terminal_command`
/// result, so the model does not have to guess which shell answered. Used
/// only where the tool result is built: the same text feeds remote
/// `read_file`/`edit_file` through `finish_run`, and there it is the file's
/// contents, which a shell line would corrupt.
fn with_shell_line(text: &str, shell_id: &str, launched: bool) -> String {
    if launched {
        text.to_string()
    } else {
        format!("{text}\n[ran in shell {shell_id}]")
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
        sessions,
        ..
    } = ctx;
    // ── Pre-fetch session state in one acquisition ───────────────────────────
    let snap = route::load_snapshot(session_id, sessions);
    // Defensive guard: a ghost shell entry must always have a ghost_config.
    if snap.is_ghost_shell && snap.ghost_policy.is_none() {
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
    // ── Tool Policy Enforcement (agent-level) ────────────────────────────────
    if let Some(policy) = &snap.tool_policy
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

    // Emit ToolStarted for silent tools so the client can show a history entry
    // and animate an elapsed timer.  Approval-gated and panel tools are excluded
    // because they have their own richer UI (ToolCallPrompt, EditFilePrompt, …).
    let emit_feedback = call.should_emit_tool_feedback();
    if emit_feedback
        && let Err(e) = send_response_split(
            tx,
            Response::ToolStarted {
                id: call.id().to_string(),
                tool: call.tool_name().to_string(),
                summary: call.summary(),
            },
        )
        .await
    {
        log::debug!("ToolStarted frame not sent: {e}");
    }
    let tool_start = Instant::now();

    let outcome = route::route(call, tx, rx, ctx, cache, schedule_store, snap).await?;

    if emit_feedback {
        let (ok, detail) = tool_outcome_detail(&outcome);
        if let Err(e) = send_response_split(
            tx,
            Response::ToolFinished {
                id: call.id().to_string(),
                ok,
                elapsed_ms: tool_start.elapsed().as_millis() as u64,
                detail,
            },
        )
        .await
        {
            log::debug!("ToolFinished frame not sent: {e}");
        }
    }

    Ok(outcome)
}

/// Extract a human-readable result summary from a tool outcome.
fn tool_outcome_detail(outcome: &ToolCallOutcome) -> (bool, Option<String>) {
    // `ok` comes from `succeeded()` so this feedback and the run tally
    // agree by construction (bug-01-1: two copies of the rule drift).
    let ok = outcome.succeeded().unwrap_or(false);
    let detail = match outcome {
        ToolCallOutcome::UserMessage(_) => Some("redirected".into()),
        ToolCallOutcome::SpawnGhostSession { .. } => Some("ghost spawned".into()),
        ToolCallOutcome::Result(s) => {
            let n = s.lines().count();
            if n > 1 {
                Some(format!("{n} lines"))
            } else {
                None
            }
        }
    };
    (ok, detail)
}

// ---------------------------------------------------------------------------
// Approval gate — shared by foreground, background, and write-operations.
// ---------------------------------------------------------------------------

/// Whether this approval is for a launch — a command that keeps the shell it
/// runs in, so nothing waits for it to exit. True when the call asked for
/// `interactive: true` **or** the command itself is a recognised interactive
/// program, so an auto-upgraded `ssh host` is labelled the same way a
/// `claude` the model asked for is. Display-only: classification and the
/// ghost `is_safe` check read the command string alone.
fn is_launch(interactive: bool, cmd: &str) -> bool {
    interactive || crate::daemon::utils::is_interactive_command(cmd)
}

/// daemon.log line for an approval decision, in 2.0 terms: a command is
/// waited or run with `wait: false`, never "foreground" or "background".
fn approval_log_line(background: bool, decision: &str, cmd: &str) -> String {
    if background {
        format!("command {decision} (wait: false): {cmd}")
    } else {
        format!("command {decision} (waited): {cmd}")
    }
}

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
        user_shell,
        interactive,
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
            classification: prompt_classification(
                host,
                ghost_policy.is_some(),
                &known_hosts,
                user_shell,
            ),
            interactive,
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
            log::info!("{}", approval_log_line(background, "approved", cmd));
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
            log::info!("{}", approval_log_line(background, decision, cmd));
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

#[cfg(test)]
mod approval_log_line_tests {
    use super::approval_log_line;

    #[test]
    fn the_approval_log_line_speaks_2_0() {
        assert_eq!(
            approval_log_line(false, "approved", "ls"),
            "command approved (waited): ls"
        );
        assert_eq!(
            approval_log_line(true, "denied", "rm x"),
            "command denied (wait: false): rm x"
        );
        for line in [
            approval_log_line(false, "approved", "ls"),
            approval_log_line(true, "denied", "rm x"),
        ] {
            assert!(!line.contains("foreground"), "no 1.x mode word: {line}");
            assert!(!line.contains("background"), "no 1.x mode word: {line}");
        }
    }
}

// ---------------------------------------------------------------------------
#[cfg(test)]
mod send_input_gate_tests {
    use super::{ghost_input_refusal, ghost_may_send_input, ghost_may_type_into};
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

    #[test]
    fn ghost_may_type_into_a_program_only_with_allow_tui_input() {
        assert!(
            ghost_may_type_into(&crate::shell::Foreground::Prompt, false),
            "a prompt is always a ghost's typing target"
        );
        assert!(
            ghost_may_type_into(&crate::shell::Foreground::Prompt, true),
            "a prompt is always a ghost's typing target"
        );
        let programs = [
            crate::shell::Foreground::Program {
                cmd: "claude".to_string(),
                tui: true,
            },
            crate::shell::Foreground::Program {
                cmd: "grep".to_string(),
                tui: false,
            },
        ];
        for fg in &programs {
            assert!(
                !ghost_may_type_into(fg, false),
                "{fg} is refused without the runbook opt-in"
            );
            assert!(
                ghost_may_type_into(fg, true),
                "{fg} is allowed when the runbook sets allow_tui_input"
            );
        }
        for fg in [
            crate::shell::Foreground::Paused {
                cmd: "sleep".to_string(),
            },
            crate::shell::Foreground::Exited,
            crate::shell::Foreground::Unknown,
        ] {
            assert!(
                !ghost_may_type_into(&fg, false) && !ghost_may_type_into(&fg, true),
                "{fg} is never a ghost's typing target, opt-in or not"
            );
        }
    }

    #[test]
    fn ghost_input_refusal_names_the_shell_and_what_holds_it() {
        let refusal = ghost_input_refusal(
            "s4",
            &crate::shell::Foreground::Program {
                cmd: "vim".to_string(),
                tui: true,
            },
        );
        assert!(
            refusal.starts_with("Error:"),
            "the refusal is the tool's error form: {refusal}"
        );
        assert!(
            refusal.contains("s4") && refusal.contains("running vim (TUI)"),
            "the refusal names the shell and what holds it: {refusal}"
        );
        assert!(
            refusal.contains("nothing was sent"),
            "the refusal says nothing was sent: {refusal}"
        );
        assert!(
            refusal.contains("allow_tui_input"),
            "the Program refusal names the runbook field that lifts it: {refusal}"
        );
        let paused = ghost_input_refusal(
            "s5",
            &crate::shell::Foreground::Paused {
                cmd: "vim".to_string(),
            },
        );
        assert!(
            !paused.contains("allow_tui_input"),
            "the field does not lift a paused shell, so the refusal names it not: {paused}"
        );
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
    use super::{edit_file_connects_first, ghost_host_refusal, resolve_foreground_host};
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

    // ── bug-08e-2: a ghost's edit_file never connects ───────────────────────

    /// The decision the EditFile arm consults: a ghost is refused inside
    /// `run_edit_file`, so its call must never reach the connect step for
    /// any host argument at all — including a name that is not configured
    /// (the BatchMode ControlMaster bug-08e-2 describes) and including a
    /// well-formed one. Chat connects first for every well-formed name.
    #[test]
    fn a_ghost_edit_file_never_connects() {
        for host in [None, Some("nope"), Some("pinky"), Some("local")] {
            assert!(
                !edit_file_connects_first(true, host),
                "a ghost's edit_file never connects: host = {host:?}"
            );
        }
    }

    #[test]
    fn a_chat_edit_file_connects_first_for_a_well_formed_name() {
        assert!(edit_file_connects_first(false, Some("pinky")));
        assert!(edit_file_connects_first(false, Some("not-configured")));
        // `None` and `"local"` are the daemon host — nothing to connect to.
        assert!(!edit_file_connects_first(false, None));
        assert!(!edit_file_connects_first(false, Some("local")));
        // A malformed name passes through; the operation refuses it with
        // HostId::parse's own wording.
        assert!(!edit_file_connects_first(false, Some("../etc")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::RunMetrics;

    /// A plain result is a success; `Error:` and `error:` prefixes are
    /// failures; the prefix rule does not catch a result that merely
    /// *contains* the substring; a spawned ghost is a success; a
    /// `UserMessage` redirect counts nothing; an `InvalidArgs` call counts
    /// one call and one parse failure but no success.
    #[test]
    fn tally_tool_outcome_classifies_each_outcome() {
        let invalid = PendingCall::InvalidArgs {
            id: "t-1".to_string(),
            name: "run_terminal_command",
            arguments: "{\"cmd\":}".to_string(),
            error: "missing value".to_string(),
            thought_signature: None,
        };
        let valid = PendingCall::ListSchedules {
            id: "t-2".to_string(),
            thought_signature: None,
        };

        let mut m = RunMetrics::default();
        tally_tool_outcome(&mut m, &valid, &ToolCallOutcome::Result("ls output".into()));
        assert_eq!(m.tool_calls, 1, "a plain result counts one call");
        assert_eq!(m.tool_successes, 1, "a plain result is a success");
        assert_eq!(m.parse_failures, 0, "a plain result is not a parse failure");

        let mut m = RunMetrics::default();
        tally_tool_outcome(
            &mut m,
            &valid,
            &ToolCallOutcome::Result("Error: no disk".into()),
        );
        assert_eq!(m.tool_calls, 1, "an Error: result still counts one call");
        assert_eq!(m.tool_successes, 0, "an `Error:` result is a failure");
        assert_eq!(
            m.parse_failures, 0,
            "an `Error:` result is not a parse failure"
        );

        let mut m = RunMetrics::default();
        tally_tool_outcome(
            &mut m,
            &valid,
            &ToolCallOutcome::Result("error: bad flag".into()),
        );
        assert_eq!(
            m.tool_successes, 0,
            "a lowercase `error:` result is a failure too"
        );

        let mut m = RunMetrics::default();
        tally_tool_outcome(
            &mut m,
            &valid,
            &ToolCallOutcome::Result("grep: found \"Error: x\"".into()),
        );
        assert_eq!(
            m.tool_successes, 1,
            "the prefix rule is a prefix — a result containing the substring succeeds"
        );

        let mut m = RunMetrics::default();
        tally_tool_outcome(
            &mut m,
            &valid,
            &ToolCallOutcome::SpawnGhostSession {
                session_id: "gs-1".into(),
                runbook_name: "investigate-disk".into(),
                tool_result: "ghost started".into(),
                job_id: "gs-1".into(),
            },
        );
        assert_eq!(
            m.tool_successes, 1,
            "a spawned ghost counts one call and one success"
        );

        let mut m = RunMetrics::default();
        tally_tool_outcome(
            &mut m,
            &valid,
            &ToolCallOutcome::UserMessage("use the other shell".into()),
        );
        assert_eq!(
            (m.tool_calls, m.tool_successes, m.parse_failures),
            (0, 0, 0),
            "a UserMessage redirect is not a tool result and counts nothing"
        );

        let mut m = RunMetrics::default();
        tally_tool_outcome(
            &mut m,
            &invalid,
            &ToolCallOutcome::Result("Error: {\"cmd\":}".into()),
        );
        assert_eq!(m.tool_calls, 1, "an InvalidArgs call counts one call");
        assert_eq!(
            m.tool_successes, 0,
            "an InvalidArgs call with an Error: result counts no success"
        );
        assert_eq!(
            m.parse_failures, 1,
            "an InvalidArgs call counts one parse failure"
        );
    }

    /// PE decision 1, at the tool: a caller outside tmux gets the daemon's own
    /// shells only — no pane blocks, no pane map, no foreground-target line
    /// naming a pane it is not in. Drives the GetShellContext arm of
    /// `execute_tool_call` with `chat_pane: None` over a fixture registry.
    #[tokio::test]
    async fn get_shell_context_without_chat_pane_is_shells_only() {
        let _guard = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let reg = crate::shell::ShellRegistry::default();
        let _swap = crate::daemon::executor::knowledge::open_shell_test_seam::swap_in(&reg);
        crate::shell::with_shells(&reg, |store| {
            store.insert(
                "s0".to_string(),
                crate::shell::ShellRecord {
                    id: crate::shell::ShellId::from_index(0),
                    owner: crate::shell::Owner::Chat("sess-a".to_string()),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 1_788_470_000,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });
        let store: SessionStore = SessionStore::new();
        let call = PendingCall::GetShellContext {
            id: "t1".to_string(),
            thought_signature: None,
            scope: None,
        };
        let (mut daemon_tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let outcome = execute_tool_call(
            &call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some("sess-a"),
                session_name: "sess-a",
                chat_pane: None,
                sessions: &store,
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await
        .expect("get_shell_context must not error");
        let text = match outcome {
            ToolCallOutcome::Result(t) => t,
            ToolCallOutcome::UserMessage(t) => format!("[redirected] {t}"),
            ToolCallOutcome::SpawnGhostSession { tool_result, .. } => tool_result,
        };
        assert!(
            text.contains("[SHELLS]"),
            "the shells-only block replaces the pane context: {text}"
        );
        assert!(
            !text.contains("PANE %"),
            "no pane data may reach a pane-less caller: {text}"
        );
        assert!(
            !text.contains("[FOREGROUND TARGET]"),
            "no foreground-target line may reach a pane-less caller: {text}"
        );
    }

    /// #73: a silent tool whose arm exits early must still emit `ToolFinished`
    /// after `ToolStarted`, or the client's history entry never gets its ✓/✗
    /// and timing label. `get_shell_context` from outside tmux is one such
    /// early exit — the arm returns the shells-only block straight from the
    /// match. Every frame the daemon wrote is read off the client half of the
    /// pipe: the daemon's write half is dropped first, or the read never sees
    /// EOF and never returns.
    #[tokio::test]
    async fn silent_tool_early_exit_sends_tool_finished() {
        use tokio::io::AsyncReadExt;

        let _guard = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let reg = crate::shell::ShellRegistry::default();
        let _swap = crate::daemon::executor::knowledge::open_shell_test_seam::swap_in(&reg);
        let store: SessionStore = SessionStore::new();
        let call = PendingCall::GetShellContext {
            id: "t1".to_string(),
            thought_signature: None,
            scope: None,
        };
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let _outcome = execute_tool_call(
            &call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some("sess-a"),
                session_name: "sess-a",
                chat_pane: None,
                sessions: &store,
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await
        .expect("get_shell_context must not error");
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe must not error");
        let events: Vec<(String, String)> = String::from_utf8(raw)
            .expect("the daemon writes UTF-8 frames")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Response>(line)
                    .expect("every frame the daemon wrote is a Response")
            })
            .filter_map(|r| match r {
                Response::ToolStarted { id, .. } => Some(("ToolStarted".to_string(), id)),
                Response::ToolFinished { id, .. } => Some(("ToolFinished".to_string(), id)),
                _ => None,
            })
            .collect();
        assert_eq!(
            events,
            vec![
                ("ToolStarted".to_string(), "t1".to_string()),
                ("ToolFinished".to_string(), "t1".to_string()),
            ],
            "an early exit must still emit ToolFinished after ToolStarted for the \
             call's id; the client saw: {events:?}"
        );
    }

    #[test]
    fn prompt_classification_marks_the_users_shell() {
        assert_eq!(
            prompt_classification(None, false, &[], Some("s7")),
            crate::ipc::Classification::UserShell { shell: "s7".into() },
            "a call aimed at the user's shell is always asked"
        );
        assert_eq!(
            prompt_classification(None, false, &[], None),
            crate::ipc::Classification::Command,
            "anything else is classified as before"
        );
        assert_eq!(
            prompt_classification(Some("pinky"), false, &["pinky".to_string()], None),
            crate::ipc::Classification::RemoteShell {
                host: "pinky".into()
            }
        );
    }

    #[test]
    fn namespaces_non_ghost_is_global_only() {
        let store: SessionStore = SessionStore::new();
        let ns = build_memory_namespaces(Some("any-sid"), &store, false);
        assert_eq!(ns, vec!["global".to_string()]);
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

    #[test]
    fn a_given_up_prompt_error_does_not_say_the_command_may_still_be_running() {
        let gave_up = anyhow::Error::new(crate::shell::PromptGivenUp {
            reason: "nobody is attached to answer it".to_string(),
        })
        .context("the password prompt in shell s7 was cancelled");
        let text = shell_run_error_text("s7", &gave_up);
        assert!(
            text.contains("s7"),
            "the text names the shell the command was sent to: {text}"
        );
        assert!(
            text.contains("cancelled"),
            "the text says the prompt was cancelled: {text}"
        );
        assert!(
            text.contains("nobody is attached to answer it"),
            "the give-up reason reaches the model verbatim: {text}"
        );
        assert!(
            !text.contains("may still be running"),
            "a cancelled prompt runs nothing; the caveat must be gone: {text}"
        );
    }

    /// A give-up reason that itself ends with a full stop must not come out
    /// doubled (`..`) when the guidance follows it — in any of the three
    /// branches.
    #[test]
    fn a_run_error_never_doubles_its_full_stop() {
        let exited = anyhow::Error::new(crate::shell::ShellExited {
            cmd: "echo hi".to_string(),
        });
        let timeout =
            anyhow::anyhow!("timed out after 45s waiting for command output: \"sleep 60\"");
        let gave_up = anyhow::Error::new(crate::shell::PromptGivenUp {
            reason: "nobody is attached to answer it — run it from a chat session.".to_string(),
        })
        .context("the password prompt in shell s7 was cancelled");

        for e in [&exited, &timeout, &gave_up] {
            let text = shell_run_error_text("s7", e);
            assert!(
                !text.contains(".."),
                "no run error doubles its full stop: {text}"
            );
        }
        let text = shell_run_error_text("s7", &gave_up);
        assert!(
            text.contains("chat session. The password prompt"),
            "the reason's sentence survives once and the guidance follows it: {text}"
        );
    }

    /// A busy-shell refusal happened before anything was written, so it must
    /// not arrive wrapped in the generic run-error caveat ("may still be
    /// running … retrying could double-apply") that contradicts the refusal
    /// (F3).
    #[test]
    fn a_busy_refusal_says_nothing_was_written() {
        let refused = anyhow::Error::new(shell_exec::RefusedBeforeWrite).context(
            "Error: shell s1 is running claude (TUI); a command written there \
             would be typed into it. Use send_input to type into that program, \
             read_shell_screen to see it, or shell: \"new\" for a fresh shell.",
        );
        let text = shell_run_error_text("s1", &refused);
        assert!(
            text.contains("Nothing was written"),
            "the refusal says nothing was written: {text}"
        );
        assert!(
            text.contains("running claude"),
            "the refusal still names the program holding the terminal: {text}"
        );
        assert!(
            !text.contains("double-apply"),
            "nothing was written, so there is no double-apply risk to warn about: {text}"
        );
        assert!(
            !text.contains("may still be running"),
            "nothing was written, so nothing may still be running: {text}"
        );
    }

    /// F7: the per-owner shell-cap refusal is raised by `registry.rs` before
    /// anything is written, so it takes the same pre-write text as the busy
    /// refusal — and, having never touched a shell, must not name one.
    #[test]
    fn a_cap_refusal_says_nothing_was_written_and_names_no_shell() {
        let refused = anyhow::Error::new(shell_exec::RefusedBeforeWrite)
            .context("owner 'chat:x' already holds 5 shells (cap 5)");
        let text = shell_run_error_text("s1", &refused);
        assert!(
            text.contains("Nothing was written"),
            "the cap refusal says nothing was written: {text}"
        );
        assert!(
            text.contains("cap 5"),
            "the cap refusal names the cap and the count held: {text}"
        );
        assert!(
            !text.contains("double-apply"),
            "nothing was written, so there is no double-apply risk to warn about: {text}"
        );
        assert!(
            !text.contains("s1"),
            "the refusal names no shell it never used: {text}"
        );
    }

    /// F10: the busy refusal's own message already starts with `Error: `, so
    /// the pre-write branch must not print a second one. A pre-write refusal
    /// whose chain has no prefix (the cap refusal) still gets exactly one.
    #[test]
    fn a_refusal_starts_with_exactly_one_error_prefix() {
        let busy = anyhow::Error::new(shell_exec::RefusedBeforeWrite).context(
            "Error: shell s1 is running claude (TUI); a command written there \
             would be typed into it. Use send_input to type into that program, \
             read_shell_screen to see it, or shell: \"new\" for a fresh shell.",
        );
        let text = shell_run_error_text("s1", &busy);
        assert!(
            text.starts_with("Error: shell s1"),
            "the busy refusal opens with the shell it refuses: {text}"
        );
        assert!(
            !text.contains("Error: Error:"),
            "the refusal must not double its Error prefix: {text}"
        );

        let cap = anyhow::Error::new(shell_exec::RefusedBeforeWrite)
            .context("owner 'chat:x' already holds 5 shells (cap 5)");
        let text = shell_run_error_text("s1", &cap);
        assert!(
            text.starts_with("Error: owner"),
            "an unprefixed pre-write refusal still gets exactly one: {text}"
        );
        assert!(
            !text.contains("Error: Error:"),
            "the cap refusal must not double its Error prefix either: {text}"
        );
    }

    /// D6's panel: the launch fact is `interactive: true` **or** a command
    /// `is_interactive_command` recognises — so an auto-upgraded `ssh host`
    /// is labelled a launch the same way a `claude` the model asked for is.
    #[test]
    fn a_launch_is_the_flag_or_a_recognised_interactive_command() {
        assert!(
            is_launch(true, "vim"),
            "the explicit flag alone makes it a launch"
        );
        assert!(
            is_launch(false, "ssh host"),
            "a recognised interactive command is a launch without the flag"
        );
        assert!(
            !is_launch(false, "ls"),
            "an ordinary command is not a launch"
        );
        assert!(!is_launch(false, ""), "an empty command is not a launch");
    }

    #[test]
    fn a_waited_result_names_its_shell() {
        assert_eq!(
            with_shell_line("hi\n[command exited with status 3]", "s2", false),
            "hi\n[command exited with status 3]\n[ran in shell s2]",
            "the shell line follows the exit-status note"
        );
        assert_eq!(
            with_shell_line("hi", "s1", false),
            "hi\n[ran in shell s1]",
            "a bare output gets the shell line as its final line"
        );
        assert_eq!(
            with_shell_line("[Interactive session started] …", "s1", true),
            "[Interactive session started] …",
            "a launch already names its shell and is returned unchanged"
        );
    }
}
