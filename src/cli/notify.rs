use anyhow::Result;
use tokio::io::BufReader;

use super::commands::{connect, recv, send_request};

/// Defense-in-depth (M3): the tmux `run-shell` bodies single-quote their
/// `#{{session_name}}` placeholder, which is inert for every shell metachar
/// except a literal `'`.  A single quote — or any control character — in a
/// session name would break the quoting, so we refuse to forward such names
/// from hook payloads.  Legal names containing spaces, parens, dots, etc.
/// pass through untouched.
fn validate_hook_session_name(name: &str) -> Option<String> {
    if name.chars().any(|c| c.is_control() || c == '\'') {
        log::warn!(
            "Ignoring tmux hook notification: session name contains unsafe characters: {:?}",
            name
        );
        return None;
    }
    Some(name.to_string())
}

pub async fn run_notify_activity(
    pane_id: String,
    hook_index: usize,
    session_name: String,
) -> Result<()> {
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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
    let Some(session_name) = validate_hook_session_name(&session_name) else {
        return Ok(());
    };
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

#[cfg(test)]
mod tests {
    use super::validate_hook_session_name;

    #[test]
    fn accepts_normal_names() {
        assert_eq!(
            validate_hook_session_name("work (client 1)").as_deref(),
            Some("work (client 1)")
        );
        assert_eq!(
            validate_hook_session_name("prod-01.dc").as_deref(),
            Some("prod-01.dc")
        );
    }

    #[test]
    fn rejects_single_quote() {
        assert_eq!(validate_hook_session_name("x'y"), None);
        assert_eq!(validate_hook_session_name("x'$(cmd)'"), None);
    }

    #[test]
    fn rejects_control_characters() {
        assert_eq!(validate_hook_session_name("a\nb"), None);
        assert_eq!(validate_hook_session_name("a\tb\x01c"), None);
        assert_eq!(validate_hook_session_name("kill\x7fsession"), None);
    }
}
