use anyhow::Result;
use tokio::io::BufReader;

use super::commands::{connect, recv, send_request};

pub async fn run_notify_activity(
    pane_id: String,
    hook_index: usize,
    session_name: String,
) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()), // Silently abort if daemon is not running (e.g. hook fires but daemon was killed)
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyActivity {
                    pane_id,
                    hook_index,
                    session_name,
                },
            )
            .await?;
            let _ = recv(&mut rx).await; // Consume Response::Ok
            Ok(())
        }
    }
}

pub async fn run_notify_complete(
    pane_id: String,
    exit_code: i32,
    session_name: String,
) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()), // Silently abort if daemon is not running
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyComplete {
                    pane_id,
                    exit_code,
                    session_name,
                },
            )
            .await?;
            let _ = recv(&mut rx).await; // Consume Response::Ok
            Ok(())
        }
    }
}

/// Notify the daemon that a pane received focus (`pane-focus-in` hook, N1).
pub async fn run_notify_focus(pane_id: String, session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyFocus {
                    pane_id,
                    session_name,
                },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}

/// Notify the daemon that the active window changed (`session-window-changed` hook, N2).
pub async fn run_notify_window_changed(session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyWindowChanged { session_name },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}

/// Notify the daemon that a new tmux session was created (`after-new-session` hook, N14).
pub async fn run_notify_session_created(session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifySessionCreated { session_name },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}

/// Notify the daemon that a tmux session was destroyed (`session-closed` hook, A6).
pub async fn run_notify_session_closed(session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifySessionClosed { session_name },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}

/// Notify the daemon that a tmux client attached to a session (`client-attached` hook, N15).
pub async fn run_notify_client_attached(session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyClientAttached { session_name },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}

/// Notify the daemon that a tmux client detached from a session (`client-detached` hook, N15).
pub async fn run_notify_client_detached(session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyClientDetached { session_name },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}

/// Notify the daemon that the terminal was resized (`client-resized` hook, N8).
pub async fn run_notify_resize(width: u16, height: u16, session_name: String) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()),
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyResize {
                    width,
                    height,
                    session_name,
                },
            )
            .await?;
            let _ = recv(&mut rx).await;
            Ok(())
        }
    }
}
