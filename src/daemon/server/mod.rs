//! IPC server: client dispatch (`handle_client`) plus the catch-up brief,
//! quick-return handlers, and the `handle_ask` orchestrator.
//! Split across submodules in phase-08; the public surface is re-exported here.

mod ask;
mod catchup;
mod handlers;

pub(crate) use catchup::is_valid_pane_id;

use ask::handle_ask;
use handlers::*;

use crate::config::Config;
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;

/// Handle one client connection end-to-end.
///
/// ## Request routing
/// - `Ping` / `Shutdown` / `Refresh` are dispatched and returned immediately.
/// - `Ask` drives the full conversation turn: load history → build prompt →
///   stream AI response → collect tool calls → execute each (background or
///   foreground) → loop back for the next AI turn until no tool calls remain.
///
/// ## Tool call execution
/// Each tool call goes through an approval gate:
/// - The client is sent a `ToolCallPrompt`; the user approves or denies.
/// - **Background** (`background: true`): the daemon runs the command as a
///   subprocess (`tokio::process`). If sudo is needed a `CredentialPrompt` is sent
///   and the credential is piped to `sudo -S`.
/// - **Foreground** (`background: false`): `tmux send-keys` dispatches to the
///   user's working pane. If sudo is detected the daemon switches focus to that
///   pane and waits for `pane_current_command` to leave "sudo".
///
/// ## Session persistence
/// Message history is stored both in the in-memory `sessions` map (fast lookup
/// within the same daemon run) and in `~/.daemoneye/sessions/<id>.jsonl` (survives
/// restarts). History is trimmed to `MAX_HISTORY` messages before each save.
pub async fn handle_client(
    stream: UnixStream,
    cache: Arc<SessionCache>,
    sessions: SessionStore,
    schedule_store: Arc<ScheduleStore>,
    bg_session: Arc<std::sync::Mutex<String>>,
    managed_session: Arc<Option<String>>,
) -> Result<()> {
    let config = Config::load().unwrap_or_else(|_| {
        log::warn!("Failed to load config, using defaults");
        Config::default()
    });

    /// Maximum size of a single incoming IPC message (1 MiB).
    /// Prevents a malicious or buggy client from exhausting daemon memory by
    /// sending an arbitrarily large JSON payload without a newline.
    const MAX_IPC_MESSAGE_BYTES: usize = 1 << 20;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }
    if line.len() > MAX_IPC_MESSAGE_BYTES {
        let mut stream = reader.into_inner();
        send_response(
            &mut stream,
            Response::Error(format!(
                "Request too large ({} bytes; limit {} bytes)",
                line.len(),
                MAX_IPC_MESSAGE_BYTES
            )),
        )
        .await?;
        return Ok(());
    }

    let request: Request = match serde_json::from_str(line.trim()) {
        Ok(req) => req,
        Err(e) => {
            let mut stream = reader.into_inner();
            send_response(
                &mut stream,
                Response::Error(format!("Invalid request: {}", e)),
            )
            .await?;
            return Ok(());
        }
    };

    let (rx_half, mut tx) = reader.into_inner().into_split();
    let mut rx = BufReader::new(rx_half);

    match request {
        Request::Ping => {
            handle_ping(&mut tx).await?;
        }
        Request::Shutdown => {
            handle_shutdown(&mut tx).await?;
            return Ok(());
        }
        Request::Refresh => {
            handle_refresh(&mut tx).await?;
        }
        Request::SetModel {
            session_id,
            model: model_name,
        } => {
            handle_set_model(&mut tx, &sessions, &config, session_id, model_name).await?;
        }
        Request::ListModels { session_id } => {
            handle_list_models(&mut tx, &sessions, &config, session_id).await?;
        }
        Request::SetPane {
            session_id,
            pane_id,
        } => {
            handle_set_pane(&mut tx, &sessions, &cache, session_id, pane_id).await?;
        }
        Request::ListPanesForSession { session_id } => {
            handle_list_panes(&mut tx, &sessions, &cache, session_id).await?;
        }
        Request::Status => {
            handle_status(&mut tx, &sessions, &schedule_store, &config).await?;
        }
        Request::QueryLimits { session_id: sid } => {
            handle_query_limits(&mut tx, &sessions, &config, sid).await?;
        }
        Request::ResetSessionToolCount { session_id: sid } => {
            handle_reset_tool_count(&mut tx, &sessions, sid).await?;
        }
        Request::SaveSession {
            session_id: sid,
            name,
            description,
            force,
        } => {
            handle_save_session(&mut tx, &sessions, sid, name, description, force).await?;
        }
        Request::LoadSession {
            session_id: sid,
            name,
            force,
        } => {
            handle_load_session(&mut tx, &sessions, &config, sid, name, force).await?;
        }
        Request::ListSavedSessions => {
            handle_list_saved_sessions(&mut tx).await?;
        }
        Request::DeleteSavedSession { name } => {
            handle_delete_saved_session(&mut tx, name).await?;
        }
        Request::RenameSavedSession { old_name, new_name } => {
            handle_rename_saved_session(&mut tx, &sessions, old_name, new_name).await?;
        }

        Request::NotifyActivity { pane_id, .. } => {
            crate::daemon::hook::handle_notify_activity(&mut tx, &pane_id).await?;
        }
        Request::NotifyComplete {
            pane_id, exit_code, ..
        } => {
            crate::daemon::hook::handle_notify_complete(&mut tx, &pane_id, exit_code).await?;
        }
        Request::NotifyFocus { pane_id, .. } => {
            crate::daemon::hook::handle_notify_focus(&cache, &mut tx, &pane_id).await?;
        }
        Request::NotifyWindowChanged { .. } => {
            crate::daemon::hook::handle_notify_window_changed(&cache, &mut tx).await?;
        }
        Request::NotifySessionClosed { session_name } => {
            crate::daemon::hook::handle_notify_session_closed(
                Arc::clone(&sessions),
                Arc::clone(&cache),
                Arc::clone(&managed_session),
                Arc::clone(&bg_session),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifySessionCreated { session_name } => {
            crate::daemon::hook::handle_notify_session_created(&mut tx, session_name).await?;
        }
        Request::NotifyClientDetached { session_name } => {
            crate::daemon::hook::handle_notify_client_detached(
                Arc::clone(&sessions),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifyClientAttached { session_name } => {
            crate::daemon::hook::handle_notify_client_attached(
                Arc::clone(&sessions),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifyResize { width, height, .. } => {
            crate::daemon::hook::handle_notify_resize(&cache, &mut tx, width, height).await?;
        }
        Request::Ask {
            query,
            tmux_pane,
            session_id,
            chat_pane,
            prompt,
            chat_width,
            tmux_session,
            target_pane,
            model: _ask_model,
        } => {
            let req = ask::AskRequest {
                query,
                client_pane: tmux_pane,
                session_id,
                chat_pane,
                prompt_override: prompt,
                chat_width,
                client_tmux_session: tmux_session,
                client_target_pane: target_pane,
            };
            let ctx = ask::AskContext {
                cache,
                sessions: &sessions,
                schedule_store,
                bg_session,
                config: &config,
            };
            handle_ask(req, ctx, &mut tx, &mut rx).await?;
            return Ok(());
        }
        _ => {}
    }

    Ok(())
}
