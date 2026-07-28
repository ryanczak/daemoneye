use crate::daemon::server::is_valid_pane_id;
use crate::daemon::session::{SessionStore, with_sessions};
use crate::daemon::utils::send_response_split;
use crate::ipc::Response;
use crate::tmux::cache::SessionCache;
use crate::util::UnpoisonExt;
use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
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
        entry.cleanup_bg_windows();
        log::info!(
            "Cleaned up session '{}' on tmux session-closed.",
            session_name
        );
    }

    if managed_session.as_deref() == Some(session_name.as_str()) {
        let sn = session_name.clone();
        let created = crate::tmux::off_runtime("new-session", move || {
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
    let _ = crate::tmux::off_runtime("install-session-hooks", move || {
        crate::daemon::install_session_hooks(&sn, &he)
    })
    .await;
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyClientAttached hook — clear pending detach state.
pub async fn handle_notify_client_attached<W>(
    sessions: SessionStore,
    tx: &mut W,
    session_name: String,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    with_sessions(&sessions, |store| {
        for entry in store.values_mut() {
            if entry.tmux_session == session_name {
                entry.last_detach = None;
                entry.detach_time_utc = None;
            }
        }
    });
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}

/// Handle NotifyClientDetached hook — record detach time for catch-up brief.
pub async fn handle_notify_client_detached<W>(
    sessions: SessionStore,
    tx: &mut W,
    session_name: String,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    with_sessions(&sessions, |store| {
        for entry in store.values_mut() {
            if entry.tmux_session == session_name {
                entry.last_detach = Some(now);
                entry.detach_time_utc = Some(now_utc);
                entry.messages_at_detach = entry.messages.len();
            }
        }
    });
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
