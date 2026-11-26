//! `execute_tool_call`'s two halves that are not policy or feedback: the
//! session snapshot it reads (`load_snapshot`) and the tool call itself
//! (`route`). A child module of `src/daemon/executor/mod.rs`.

use super::*;

impl<'a> SessionCtx<'a> {
    pub fn new(
        session_id: Option<&'a str>,
        session_name: &'a str,
        chat_pane: Option<&'a str>,
        sessions: &'a SessionStore,
        shells: &'a crate::shell::ShellRegistry,
    ) -> Self {
        Self {
            session_id,
            session_name,
            chat_pane,
            sessions,
            shells,
        }
    }
}

/// The session state `execute_tool_call` reads, taken in one acquisition.
pub(super) fn load_snapshot(session_id: Option<&str>, sessions: &SessionStore) -> DispatchSnapshot {
    use crate::daemon::session::SessionMode;
    match session_id {
        Some(sid) => with_sessions(sessions, |store| match store.get(sid) {
            Some(e) => match &e.mode {
                SessionMode::Ghost(g) => DispatchSnapshot {
                    ghost_policy: Some(GhostPolicy::from_config(&g.config)),
                    tool_policy: g.config.tool_policy.clone(),
                    is_ghost_shell: true,
                    spawn_depth: g.config.spawn_depth,
                    effective_parent_job_id: Some(sid.to_string()),
                    saved_name: e.persistence.saved_name.clone(),
                    turn_count: e.history.turn_count,
                },
                SessionMode::Interactive(_) => DispatchSnapshot {
                    ghost_policy: None,
                    tool_policy: None,
                    is_ghost_shell: false,
                    spawn_depth: 0,
                    effective_parent_job_id: None,
                    saved_name: e.persistence.saved_name.clone(),
                    turn_count: e.history.turn_count,
                },
            },
            None => DispatchSnapshot::default(),
        }),
        None => DispatchSnapshot::default(),
    }
}

/// The tool call itself: the per-call memory namespaces and artifact
/// context, then one arm per tool family. `execute_tool_call` runs the
/// policy refusals and the `ToolStarted` / `ToolFinished` feedback around it.
pub(super) async fn route<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    ctx: SessionCtx<'_>,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
    snap: DispatchSnapshot,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let SessionCtx {
        session_id,
        chat_pane,
        sessions,
        ..
    } = ctx;
    let is_ghost = snap.ghost_policy.is_some();
    let memory_namespaces_owned =
        build_memory_namespaces(session_id, sessions, snap.is_ghost_shell);
    let memory_namespaces: Vec<&str> = memory_namespaces_owned.iter().map(|s| s.as_str()).collect();
    let artifact_ctx = knowledge::ArtifactCtx {
        session_id,
        sessions,
        saved_name: snap.saved_name.as_deref(),
        turn_count: snap.turn_count,
        is_ghost,
        namespaces: &memory_namespaces,
    };
    match call {
        PendingCall::InvalidArgs { .. }
        | PendingCall::LoadTools { .. }
        | PendingCall::SpawnGhost { .. } => {
            dispatch_meta::dispatch(
                call,
                tx,
                rx,
                session_id,
                sessions,
                &snap.ghost_policy,
                &snap.tool_policy,
                snap.spawn_depth,
                &snap.effective_parent_job_id,
            )
            .await
        }
        PendingCall::Foreground { .. } => {
            dispatch_foreground::dispatch(call, tx, rx, ctx, &snap.ghost_policy, is_ghost).await
        }
        PendingCall::Background { .. } => {
            dispatch_background::dispatch(call, tx, rx, ctx, &snap.ghost_policy, is_ghost).await
        }
        PendingCall::ScheduleCommand { .. }
        | PendingCall::ListSchedules { .. }
        | PendingCall::CancelSchedule { .. }
        | PendingCall::DeleteSchedule { .. } => {
            dispatch_schedule::dispatch(call, tx, rx, session_id, is_ghost, schedule_store).await
        }
        PendingCall::WriteScript { .. }
        | PendingCall::ListScripts { .. }
        | PendingCall::ReadScript { .. }
        | PendingCall::DeleteScript { .. }
        | PendingCall::WriteRunbook { .. }
        | PendingCall::DeleteRunbook { .. }
        | PendingCall::ReadRunbook { .. }
        | PendingCall::ListRunbooks { .. }
        | PendingCall::CreateAgent { .. }
        | PendingCall::ReadAgent { .. }
        | PendingCall::ListAgents { .. }
        | PendingCall::DeleteAgent { .. } => {
            dispatch_artifacts::dispatch(
                call,
                tx,
                rx,
                session_id,
                is_ghost,
                schedule_store,
                &artifact_ctx,
            )
            .await
        }
        PendingCall::ReadFile { .. } | PendingCall::EditFile { .. } => {
            dispatch_files::dispatch(call, tx, rx, session_id, &snap.ghost_policy, is_ghost).await
        }
        PendingCall::AddMemory { .. }
        | PendingCall::UpdateMemory { .. }
        | PendingCall::DeleteMemory { .. }
        | PendingCall::ReadMemory { .. }
        | PendingCall::ListMemories { .. }
        | PendingCall::SearchRepository { .. }
        | PendingCall::RecallContext { .. }
        | PendingCall::AwaitAgentResult { .. } => {
            dispatch_memory::dispatch(call, tx, session_id, &artifact_ctx, &memory_namespaces).await
        }
        PendingCall::GetShellContext { .. } => {
            dispatch_context::dispatch(call, session_id, chat_pane, cache, ctx.shells, is_ghost)
        }
        PendingCall::ListShells { .. }
        | PendingCall::ReadShell { .. }
        | PendingCall::ReadShellScreen { .. }
        | PendingCall::FindInShells { .. }
        | PendingCall::CloseShell { .. }
        | PendingCall::WatchShell { .. }
        | PendingCall::ListShellLogs { .. }
        | PendingCall::ReadShellLog { .. } => {
            dispatch_shells::dispatch(call, tx, session_id, ctx.shells, is_ghost).await
        }
        PendingCall::OpenShell { .. } => {
            dispatch_open_shell::dispatch(call, tx, rx, ctx, &snap.ghost_policy, is_ghost).await
        }
        PendingCall::SendInput { .. } => {
            dispatch_send_input::dispatch(
                call,
                tx,
                rx,
                session_id,
                &snap.ghost_policy,
                &snap.tool_policy,
                is_ghost,
            )
            .await
        }
        PendingCall::RunLogTail { .. }
        | PendingCall::RunLogSearch { .. }
        | PendingCall::GetTurn { .. } => dispatch_runs::dispatch(call),
    }
}

/// Whether a ghost shell may use `send_input`.
pub(crate) fn ghost_may_send_input(
    is_ghost: bool,
    policy: Option<&crate::agents::policy::ToolPolicy>,
) -> bool {
    !is_ghost || policy.is_some_and(|p| p.explicitly_allows("send_input"))
}

/// Whether a ghost may type into a shell held by `fg`.
pub(crate) fn ghost_may_type_into(fg: &crate::shell::Foreground, allow_tui_input: bool) -> bool {
    match fg {
        crate::shell::Foreground::Prompt => true,
        crate::shell::Foreground::Program { .. } => allow_tui_input,
        crate::shell::Foreground::Paused { .. }
        | crate::shell::Foreground::Exited
        | crate::shell::Foreground::Unknown => false,
    }
}

/// The tool result when the ghost gate refuses `send_input`.
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
pub(crate) fn ghost_may_run_foreground(is_ghost: bool, sandbox_enabled: bool) -> bool {
    !is_ghost || !sandbox_enabled
}

/// Connect first for a named host; `None`, `"local"` and a malformed name
/// pass through untouched.
pub(super) async fn connect_named_host<W, R>(
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

/// Does the `edit_file` arm run its connect step?
pub(super) fn edit_file_connects_first(is_ghost: bool, host: Option<&str>) -> bool {
    !is_ghost
        && matches!(
            host.map(crate::host::HostId::parse),
            Some(Ok(crate::host::HostId::Named(_)))
        )
}
