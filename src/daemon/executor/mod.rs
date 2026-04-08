mod file_ops;
mod foreground;
mod knowledge;
mod schedule;

use crate::ai::{PendingCall, next_tool_id};
use crate::daemon::policy::GhostPolicy;
use crate::daemon::session::SessionStore;
use crate::daemon::utils::send_response_split;
use crate::ipc::{PaneInfo, Request, Response};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use crate::util::UnpoisonExt;
use std::sync::Arc;
use std::time::Duration;

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
// Main tool dispatcher
// ---------------------------------------------------------------------------

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
        session_name,
        chat_pane,
        sessions,
    } = ctx;
    // ── Pre-fetch Ghost Policy ───────────────────────────────────────────────
    let ghost_policy: Option<GhostPolicy> = if let Some(sid) = session_id {
        if let Ok(store) = sessions.lock() {
            store.get(sid).and_then(|e| {
                if e.is_ghost {
                    e.ghost_config.as_ref().map(GhostPolicy::from_config)
                } else {
                    None
                }
            })
        } else {
            None
        }
    } else {
        None
    };
    // Defensive guard: a ghost shell entry must always have a ghost_config.
    let is_ghost_shell: bool = if let Some(sid) = session_id {
        if let Ok(store) = sessions.lock() {
            store.get(sid).map(|e| e.is_ghost).unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };
    if is_ghost_shell && ghost_policy.is_none() {
        let msg = "Error: ghost shell has no policy configured (ghost_config missing in runbook frontmatter)".to_string();
        log::error!("{} for session {:?}", msg, session_id);
        send_response_split(tx, Response::ToolResult(msg.clone())).await?;
        return Ok(ToolCallOutcome::Result(msg));
    }
    let is_ghost = ghost_policy.is_some();
    // ──────────────────────────────────────────────────────────────────────────

    match call {
        PendingCall::Foreground {
            id, cmd, target, ..
        } => {
            foreground::run_foreground(
                foreground::FgArgs {
                    id,
                    cmd,
                    target: target.as_deref(),
                },
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

        PendingCall::Background {
            id,
            cmd,
            retry_pane,
            ..
        } => {
            foreground::run_background(
                id,
                cmd,
                retry_pane.as_deref(),
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
        } => knowledge::write_script(id, script_name, content, is_ghost, tx, rx).await,

        PendingCall::ListScripts { .. } => knowledge::list_scripts(tx).await,

        PendingCall::ReadScript { script_name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_script(script_name)))
        }

        PendingCall::DeleteScript {
            id, script_name, ..
        } => knowledge::delete_script(id, script_name, is_ghost, session_id, tx, rx).await,

        PendingCall::WatchPane {
            pane_id,
            timeout_secs,
            pattern,
            ..
        } => Ok(ToolCallOutcome::Result(knowledge::watch_pane(
            pane_id,
            *timeout_secs,
            pattern.as_deref(),
            session_id,
            session_name,
            sessions,
        ))),

        PendingCall::ReadFile {
            path,
            offset,
            limit,
            pattern,
            target_pane,
            ..
        } => {
            file_ops::run_read_file(
                path,
                *offset,
                *limit,
                pattern.as_deref(),
                target_pane.as_deref(),
            )
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
            target_pane,
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
                    target_pane: target_pane.as_deref(),
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
        } => knowledge::write_runbook(id, name, content, is_ghost, session_id, tx, rx).await,

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
            key, value, category, session_id,
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
            key,
            category,
            body.as_deref(),
            *append,
            tags.as_deref(),
            summary.as_deref(),
            relates_to.as_deref(),
            expires.as_deref(),
            session_id,
        ))),

        PendingCall::DeleteMemory { key, category, .. } => Ok(ToolCallOutcome::Result(
            knowledge::delete_memory(key, category, session_id),
        )),

        PendingCall::ReadMemory { key, category, .. } => Ok(ToolCallOutcome::Result(
            knowledge::read_memory(key, category),
        )),

        PendingCall::ListMemories { category, .. } => {
            knowledge::list_memories(category.as_deref(), tx).await
        }

        PendingCall::SearchRepository { query, kind, .. } => Ok(ToolCallOutcome::Result(
            knowledge::search_repository(query, kind),
        )),

        PendingCall::GetTerminalContext { .. } => {
            let target_pane: Option<String> = session_id.and_then(|sid| {
                sessions
                    .lock()
                    .ok()?
                    .get(sid)?
                    .default_target_pane
                    .clone()
            });
            let ctx = cache.get_labeled_context(chat_pane, chat_pane);
            let pane_map = cache.pane_map_summary(chat_pane);
            let fg_line = target_pane
                .as_deref()
                .map(|tp| {
                    format!(
                        "[FOREGROUND TARGET] {} — target_pane=\"{}\" for run_terminal_command(background=false)\n",
                        tp, tp
                    )
                })
                .unwrap_or_default();
            Ok(ToolCallOutcome::Result(format!(
                "{fg_line}{ctx}\n{pane_map}"
            )))
        }

        PendingCall::CloseBackgroundWindow { pane_id, .. } => Ok(ToolCallOutcome::Result(
            knowledge::close_bg_window(pane_id, session_id, sessions),
        )),

        PendingCall::ListPanes { .. } => Ok(ToolCallOutcome::Result(knowledge::list_panes(
            cache, chat_pane,
        ))),

        PendingCall::SpawnGhost {
            runbook, message, ..
        } => knowledge::spawn_ghost(runbook, message, sessions).await,
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
    } = req;
    let mode = if background {
        "background"
    } else {
        "foreground"
    };

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

async fn find_best_target_pane<W, R>(
    specified_pane: Option<&str>,
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
    let ai_target = specified_pane.and_then(|tp| {
        if chat_pane == Some(tp) {
            return None;
        }
        let panes = cache.panes.read().unwrap_or_log();
        if panes.contains_key(tp) {
            Some(tp.to_string())
        } else {
            None
        }
    });

    if let Some(tp) = ai_target {
        return Ok(tp);
    }

    // Check for a user-selected default target pane in the session.
    if let Some(sid) = session_id
        && let Ok(store) = sessions.lock()
        && let Some(entry) = store.get(sid)
        && let Some(ref dtp) = entry.default_target_pane
        && chat_pane != Some(dtp.as_str())
    {
        let panes = cache.panes.read().unwrap_or_log();
        if panes.contains_key(dtp) {
            return Ok(dtp.clone());
        }
    }

    let pane_list: Vec<PaneInfo> = {
        let panes = cache.panes.read().unwrap_or_log();
        let mut v: Vec<PaneInfo> = panes
            .iter()
            .map(|(pid, state)| PaneInfo {
                id: pid.clone(),
                current_cmd: state.current_cmd.clone(),
                summary: state.summary.clone(),
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
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
    rx.read_line(&mut pane_line).await?;
    match serde_json::from_str::<Request>(pane_line.trim()) {
        Ok(Request::PaneSelectResponse { pane_id, .. }) => {
            if let Some(sid) = session_id
                && let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(sid)
            {
                entry.default_target_pane = Some(pane_id.clone());
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
