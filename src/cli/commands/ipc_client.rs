//! IPC client helpers — connect to the daemon socket, marshal Request/Response
//! JSON over the newline-delimited protocol, and the typed `send_*` wrappers
//! that other CLI commands call to perform specific daemon operations.

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::default_socket_path;
use crate::ipc::{Request, Response};

pub(super) fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{:02x}", b)).collect();
        }
    }
    // /dev/urandom unavailable — mix nanosecond timestamp with PID.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{:08x}{:08x}", nanos ^ pid, pid.wrapping_mul(2_654_435_761))
}

/// List all configured model names and the session's current active model.
/// Returns a list of `(key_name, model_id)` pairs and the active key name.
pub(super) async fn send_list_models(session_id: &str) -> Result<(Vec<(String, String)>, String)> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::ListModels {
            session_id: session_id.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::ModelList { models, active } => Ok((models, active)),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

/// Switch the active model for the given session.
/// Returns the confirmed new model name on success.
pub(super) async fn send_set_model(session_id: &str, model: &str) -> Result<String> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::SetModel {
            session_id: session_id.to_string(),
            model: model.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::ModelChanged { model } => Ok(model),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

/// Pin the foreground target pane for the given session.
/// Returns `(pane_id, description)` on success.
pub(super) async fn send_set_pane(session_id: &str, pane_id: &str) -> Result<(String, String)> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::SetPane {
            session_id: session_id.to_string(),
            pane_id: pane_id.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::PaneChanged {
            pane_id,
            description,
        } => Ok((pane_id, description)),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

/// List targetable panes for the given session.
/// Returns `Vec<(pane_id, current_cmd, window_name, is_current_target)>`.
pub(super) async fn send_list_panes_for_session(
    session_id: &str,
) -> Result<Vec<(String, String, String, usize, bool)>> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::ListPanesForSession {
            session_id: session_id.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::PaneList { panes } => Ok(panes),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

/// Ask the daemon to re-collect system context (OS info, memory, processes, history).
pub(super) async fn send_refresh() -> Result<()> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut data = serde_json::to_vec(&Request::Refresh)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    Ok(())
}

/// Fetch effective limits config and live session counters from the daemon.
/// Returns `(limits, turn_count, tool_calls_this_session, history_len)`.
pub(super) async fn send_query_limits(
    session_id: &str,
) -> Result<(crate::ipc::LimitsSummary, usize, usize, usize)> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let req = Request::QueryLimits {
        session_id: session_id.to_string(),
    };
    let mut data = serde_json::to_vec(&req)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::LimitsInfo {
            limits,
            turn_count,
            tool_calls_this_session,
            history_len,
        } => Ok((limits, turn_count, tool_calls_this_session, history_len)),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_save_session(
    session_id: &str,
    name: &str,
    description: &str,
    force: bool,
) -> Result<String> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::SaveSession {
            session_id: session_id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            force,
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::SessionSaved { name } => Ok(name),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_load_session(
    session_id: &str,
    name: &str,
    force: bool,
) -> Result<(String, String)> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::LoadSession {
            session_id: session_id.to_string(),
            name: name.to_string(),
            force,
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::SessionLoaded { name, banner, .. } => Ok((name, banner)),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_list_saved_sessions() -> Result<Vec<crate::ipc::SessionSummary>> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(&mut tx, Request::ListSavedSessions).await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::SavedSessionList { sessions } => Ok(sessions),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_delete_saved_session(name: &str) -> Result<()> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::DeleteSavedSession {
            name: name.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::Ok => Ok(()),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_diff_sessions(name1: &str, name2: &str) -> Result<String> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::DiffSessions {
            name1: name1.to_string(),
            name2: name2.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::SessionDiff { summary } => Ok(summary),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_rename_session(old_name: &str, new_name: &str) -> Result<()> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    send_request(
        &mut tx,
        Request::RenameSavedSession {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        },
    )
    .await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::Ok => Ok(()),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub(super) async fn send_reset_session_tool_count(session_id: &str) -> Result<()> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let req = Request::ResetSessionToolCount {
        session_id: session_id.to_string(),
    };
    let mut data = serde_json::to_vec(&req)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    let mut rx = BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    match serde_json::from_str::<Response>(line.trim())? {
        Response::Ok => Ok(()),
        Response::Error(e) => anyhow::bail!("{}", e),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
}

pub async fn connect() -> Result<UnixStream> {
    let socket_path = default_socket_path();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        UnixStream::connect(&socket_path),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out connecting to daemon at {} (is it running?)",
            socket_path.display()
        )
    })?
    .with_context(|| format!("Failed to connect to daemon at {}", socket_path.display()))
}

pub async fn send_request(tx: &mut OwnedWriteHalf, req: Request) -> Result<()> {
    let mut data = serde_json::to_vec(&req)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

pub async fn recv(rx: &mut BufReader<OwnedReadHalf>) -> Result<Response> {
    let mut line = String::new();
    let n = rx.read_line(&mut line).await?;
    if n == 0 {
        anyhow::bail!("Daemon closed connection unexpectedly.");
    }
    let response: Response = serde_json::from_str(line.trim())?;
    Ok(response)
}
