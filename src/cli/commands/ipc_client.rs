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

/// Send a single request, read a single response, then drop the connection.
///
/// Used by the synchronous slash commands (`/refresh`, `/model`, `/pane`,
/// `/limits`, `/session …`) which each map to one request/response round-trip
/// — unlike `Request::Ask`, which streams many responses on one connection.
pub(super) async fn request_once(req: Request) -> Result<Response> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);
    send_request(&mut tx, req).await?;
    recv(&mut rx).await
}
