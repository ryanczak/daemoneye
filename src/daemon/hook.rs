use crate::daemon::server::is_valid_pane_id;
use crate::daemon::session::{SessionStore, with_sessions};
use crate::daemon::utils::send_response_split;
use crate::ipc::Response;
use crate::tmux::cache::SessionCache;
use crate::util::UnpoisonExt;
use anyhow::Result;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Handle NotifyActivity hook — broadcast pane activity on the BG_DONE_TX channel.
pub async fn handle_notify_activity<W>(tx: &mut W, pane_id: &str) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if is_valid_pane_id(pane_id) {
        if let Some(sender) = crate::daemon::session::BG_DONE_TX.get() {
            let _ = sender.send(pane_id.to_string());
        }
    } else {
        log::warn!("NotifyActivity: rejected invalid pane_id {:?}", pane_id);
    }
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyComplete hook — finish background command and broadcast completion.
pub async fn handle_notify_complete<W>(tx: &mut W, pane_id: &str, exit_code: i32) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if is_valid_pane_id(pane_id) {
        if let Ok(mut map) = crate::daemon::background::BG_COMMAND_MAP
            .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
            .lock()
            && let Some(cmd_id) = map.remove(pane_id)
        {
            crate::daemon::stats::finish_command(cmd_id, exit_code);
        }
        let sender = crate::daemon::session::COMPLETE_TX.get_or_init(|| {
            let (tx, _) = tokio::sync::broadcast::channel(32);
            tx
        });
        let _ = sender.send((pane_id.to_string(), exit_code));
    } else {
        log::warn!("NotifyComplete: rejected invalid pane_id {:?}", pane_id);
    }
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyFocus hook — update active pane in cache instantly.
pub async fn handle_notify_focus<W>(cache: &SessionCache, tx: &mut W, pane_id: &str) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if is_valid_pane_id(pane_id) {
        cache.set_active_pane(pane_id);
    } else {
        log::warn!("NotifyFocus: rejected invalid pane_id {:?}", pane_id);
    }
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyWindowChanged hook — refresh window topology in cache.
pub async fn handle_notify_window_changed<W>(cache: &SessionCache, tx: &mut W) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    cache.refresh_windows();
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifySessionClosed hook — clean up daemon state and recreate managed session.
pub async fn handle_notify_session_closed<W>(
    sessions: SessionStore,
    cache: Arc<SessionCache>,
    managed_session: Arc<Option<String>>,
    bg_session: Arc<std::sync::Mutex<String>>,
    tx: &mut W,
    session_name: String,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let closed: Vec<crate::daemon::session::SessionEntry> = with_sessions(&sessions, |store| {
        let matching: Vec<String> = store
            .iter()
            .filter(|(_, entry)| entry.tmux_session == session_name)
            .map(|(k, _)| k.clone())
            .collect();

        let mut closed = Vec::with_capacity(matching.len());
        for key in matching {
            if let Some(entry) = store.remove(&key) {
                closed.push(entry);
            }
        }
        closed
    });

    // Unlocked phase: everything blocking happens out here.
    for entry in &closed {
        let teardown = entry.bg_teardown();
        let _ = crate::process::off_runtime("bg-teardown", move || {
            crate::daemon::session::run_bg_teardown(teardown)
        })
        .await;
        log::info!(
            "Cleaned up session '{}' on tmux session-closed.",
            session_name
        );
    }

    if managed_session.as_deref() == Some(session_name.as_str()) {
        let sn = session_name.clone();
        let created = crate::process::off_runtime("new-session", move || {
            std::process::Command::new("tmux")
                .args(["new-session", "-d", "-s", &sn])
                .output()
        })
        .await;
        match created {
            Some(Ok(o)) if o.status.success() => {
                log::info!(
                    "Recreated managed tmux session '{}' after close.",
                    session_name
                );
                *bg_session.lock().unwrap_or_log() = session_name.clone();
                cache.set_session(&session_name);
            }
            Some(Ok(o)) => {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                log::warn!(
                    "Failed to recreate managed session '{}': {}",
                    session_name,
                    stderr
                );
            }
            Some(Err(e)) => {
                log::warn!(
                    "tmux new-session for managed session '{}' failed: {}",
                    session_name,
                    e
                );
            }
            None => {
                log::warn!(
                    "tmux new-session for managed session '{}' timed out",
                    session_name
                );
            }
        }
    }

    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifySessionCreated hook — auto-install per-session hooks.
pub async fn handle_notify_session_created<W>(tx: &mut W, session_name: String) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let hook_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());
    let sn = session_name.clone();
    let he = hook_exe.clone();
    let _ = crate::process::off_runtime("install-session-hooks", move || {
        crate::daemon::install_session_hooks(&sn, &he)
    })
    .await;
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyClientAttached hook — reply `Ok` and do nothing else.
///
/// Detach state is owned by chat presence (M27 phase 03): the daemon stamps
/// it when the last `ChatAttach` connection for a session closes and clears
/// it on the next attach. The tmux client-attached hook no longer touches it.
pub async fn handle_notify_client_attached<W>(tx: &mut W, session_name: String) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let _ = session_name;
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyClientDetached hook — reply `Ok` and do nothing else.
///
/// Detach state is owned by chat presence (M27 phase 03), not by this hook;
/// see `handle_notify_client_attached`.
pub async fn handle_notify_client_detached<W>(tx: &mut W, session_name: String) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let _ = session_name;
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyResize hook — update cached viewport dimensions.
pub async fn handle_notify_resize<W>(
    cache: &SessionCache,
    tx: &mut W,
    width: u16,
    height: u16,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    cache.set_client_size(width, height);
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

// The module's tests follow; they are the only place in this file that
// names detach state — the handlers above no longer touch it (M27 phase 03).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session::{SessionEntry, SessionStore, with_sessions};
    use std::collections::HashSet;
    use std::time::Instant;
    use tokio::io::AsyncBufReadExt;

    fn entry_with_detach(session_name: &str) -> SessionEntry {
        SessionEntry {
            messages: Vec::new(),
            last_accessed: Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: session_name.to_string(),
            last_detach: Some(Instant::now()),
            detach_time_utc: Some(chrono::Utc::now()),
            messages_at_detach: 3,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            ghost_runbook_name: None,
            started_at: chrono::Utc::now(),
            turn_count: 1,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: Vec::new(),
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        }
    }

    #[tokio::test]
    async fn tmux_client_hooks_no_longer_touch_detach_state() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            store.insert("s1".to_string(), entry_with_detach("dev"));
            store.insert("s2".to_string(), entry_with_detach("dev"));
        });

        for attach in [true, false] {
            let (client, server) = tokio::io::duplex(1 << 16);
            let (server_r, mut server_w) = tokio::io::split(server);
            let name = "dev".to_string();
            let task = tokio::spawn(async move {
                let mut rx = tokio::io::BufReader::new(server_r);
                let _ = &mut rx;
                if attach {
                    handle_notify_client_attached(&mut server_w, name).await
                } else {
                    handle_notify_client_detached(&mut server_w, name).await
                }
            });
            let mut lines = tokio::io::BufReader::new(client);
            let mut reply_line = String::new();
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                AsyncBufReadExt::read_line(&mut lines, &mut reply_line),
            )
            .await
            .expect("a reply within 5 s")
            .expect("the handler replies");
            assert!(read > 0, "the handler wrote a non-empty reply");
            let reply: Response =
                serde_json::from_str(reply_line.trim()).expect("a response parses");
            assert!(
                matches!(reply, Response::Ok),
                "hook replies Ok, got {reply:?} (attach={attach})"
            );
            task.await.unwrap().unwrap();
        }

        with_sessions(&sessions, |store| {
            for (id, e) in store.iter() {
                assert!(
                    e.last_detach.is_some(),
                    "{id}: hooks must not clear last_detach"
                );
                assert!(e.detach_time_utc.is_some());
                assert_eq!(
                    e.messages_at_detach, 3,
                    "{id}: hooks must not touch messages_at_detach"
                );
            }
        });
    }
}
