//! `execute_tool_call`'s two halves that are not policy or feedback: the
//! session snapshot it reads (`load_snapshot`) and the tool call itself
//! (`route`). A child module of `src/daemon/executor/mod.rs`.

use super::*;

/// The session state `execute_tool_call` reads, taken in one acquisition.
pub(super) fn load_snapshot(session_id: Option<&str>, sessions: &SessionStore) -> DispatchSnapshot {
    match session_id {
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
    let DispatchSnapshot {
        ghost_policy,
        tool_policy,
        is_ghost_shell,
        spawn_depth,
        effective_parent_job_id,
        saved_name,
        turn_count,
    } = snap;
    let is_ghost = ghost_policy.is_some();
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
    match call {
        PendingCall::InvalidArgs { .. }
        | PendingCall::LoadTools { .. }
        | PendingCall::SpawnGhost { .. } => {
            dispatch_meta::dispatch(
                call,
                tx,
                session_id,
                sessions,
                &ghost_policy,
                spawn_depth,
                &effective_parent_job_id,
            )
            .await
        }

        PendingCall::Foreground { .. } => {
            dispatch_foreground::dispatch(call, tx, rx, ctx, &ghost_policy, is_ghost).await
        }

        PendingCall::Background { .. } => {
            dispatch_background::dispatch(call, tx, rx, ctx, &ghost_policy, is_ghost).await
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
            dispatch_files::dispatch(call, tx, rx, session_id, &ghost_policy, is_ghost).await
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
            dispatch_context::dispatch(call, session_id, chat_pane, cache)
        }

        PendingCall::ListShells { .. }
        | PendingCall::ReadShell { .. }
        | PendingCall::ReadShellScreen { .. }
        | PendingCall::FindInShells { .. }
        | PendingCall::CloseShell { .. }
        | PendingCall::WatchShell { .. }
        | PendingCall::ListShellLogs { .. }
        | PendingCall::ReadShellLog { .. } => dispatch_shells::dispatch(call, tx, session_id).await,

        PendingCall::OpenShell { .. } => {
            dispatch_open_shell::dispatch(call, tx, rx, session_id, &ghost_policy, is_ghost).await
        }

        PendingCall::SendInput { .. } => {
            dispatch_send_input::dispatch(
                call,
                tx,
                rx,
                session_id,
                &ghost_policy,
                &tool_policy,
                is_ghost,
            )
            .await
        }

        PendingCall::RunLogTail { .. }
        | PendingCall::RunLogSearch { .. }
        | PendingCall::GetTurn { .. } => dispatch_runs::dispatch(call),
    }
}
