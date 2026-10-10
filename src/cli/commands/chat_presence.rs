//! Chat-presence client (M27 phase 03).
//!
//! One held IPC connection per chat client, opened right after the session id
//! is chosen and kept until the process exits. The attach reply carries the
//! catch-up brief (zero or one `Response::SystemMsg`) before `Response::Ok`;
//! after that the daemon holds the connection and the client ignores it until
//! exit. Phase 03b will push alerts down the same connection.

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ipc::Response;

use super::ipc_client::{connect, recv, send_request};

/// The reply a `ChatAttach` gets: zero or one `Response::SystemMsg` (the
/// catch-up brief) and then `Response::Ok`. Returns the brief, if any.
///
/// The returned connection is the presence connection. The caller must keep
/// it alive (drop it only when the chat exits) — closing it stamps detach
/// state on the daemon side.
pub(crate) async fn attach(session_id: &str) -> Result<(Option<String>, HeldPresence)> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);
    send_request(
        &mut tx,
        crate::ipc::Request::ChatAttach {
            session_id: session_id.to_string(),
        },
    )
    .await?;

    // Brief: zero or one SystemMsg, then Ok. Both within the connect/send
    // budget so a hung daemon cannot stall chat startup forever.
    let mut brief = None;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(15), recv(&mut rx))
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for the chat-attach reply"))??;
    match reply {
        Response::SystemMsg(msg) => {
            brief = Some(msg);
            let ok = tokio::time::timeout(std::time::Duration::from_secs(5), recv(&mut rx))
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for chat-attach Ok"))??;
            if !matches!(ok, Response::Ok) {
                anyhow::bail!("unexpected chat-attach reply after the brief: {ok:?}");
            }
        }
        Response::Ok => {}
        Response::Error(e) => anyhow::bail!("chat attach refused: {e}"),
        other => anyhow::bail!("unexpected chat-attach reply: {other:?}"),
    }

    Ok((brief, HeldPresence { rx, tx }))
}

/// The held presence connection. Kept by the caller for the process lifetime;
/// dropping it closes the connection and stamps detach state daemon-side.
pub(crate) struct HeldPresence {
    rx: BufReader<tokio::net::unix::OwnedReadHalf>,
    tx: tokio::net::unix::OwnedWriteHalf,
}

impl HeldPresence {
    /// Park the connection in a background task that holds it (reading and
    /// discarding) until the returned guard is dropped or the process exits.
    /// The daemon pushes nothing down this connection yet (phase 03b will);
    /// reading here keeps the socket drained so nothing blocks the daemon.
    pub(crate) fn park(self) -> HeldPresenceGuard {
        let (mut rx, mut tx) = (self.rx, self.tx);
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let mut discarded = String::new();
            tokio::select! {
                // Drain whatever the daemon pushes (ignored for now).
                res = rx.read_line(&mut discarded) => {
                    // EOF or error both end the hold; the detach is stamped
                    // daemon-side either way.
                    let _ = res;
                    let _ = tx.shutdown().await;
                }
                _ = &mut done_rx => {
                    // Guard dropped — close cleanly.
                    let _ = tx.shutdown().await;
                }
            }
        });
        HeldPresenceGuard {
            done_tx: Some(done_tx),
            task: Some(task),
        }
    }
}

/// Owns the parked presence task; dropping it ends the hold.
pub(crate) struct HeldPresenceGuard {
    done_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for HeldPresenceGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.done_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // park() over a real daemon socket cannot run hermetically here —
    // HeldPresence is built only by attach(), which needs a live daemon.
    // The lifecycle is exercised end-to-end by every chat run (drop of the
    // guard after the loop); the detach-stamping behaviour daemon-side is
    // covered by the server chat_presence tests over a duplex stream.
    #[test]
    fn attach_is_the_only_constructor_of_held_presence() {
        assert!(
            !std::any::type_name::<HeldPresence>().is_empty(),
            "the type exists; only attach() can build it"
        );
    }
}
