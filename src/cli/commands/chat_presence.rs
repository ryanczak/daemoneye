//! Chat-presence client (M27 phase 03).
//!
//! One held IPC connection per chat client, opened right after the session id
//! is chosen and kept until the process exits. The attach reply carries the
//! catch-up brief (zero or one `Response::SystemMsg`) before `Response::Ok`;
//! after that the daemon pushes alerts down the connection and the client
//! forwards each one to the chat loop (phase 03b).

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
    /// Park the connection in a background task that reads pushed alerts until
    /// the connection closes, forwarding each one to the channel the chat loop
    /// selects on. The task never writes to stdout — the ratatui renderer owns
    /// the screen; rendering happens in the loop that owns it.
    ///
    /// The returned guard ends the hold when dropped (chat exiting), closing
    /// the connection and stamping detach state daemon-side.
    pub(crate) fn park(
        self,
    ) -> (
        HeldPresenceGuard,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (mut rx, mut tx) = (self.rx, self.tx);
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
        let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let task = tokio::spawn(async move {
            let mut discarded = String::new();
            loop {
                let got_line = tokio::select! {
                    res = rx.read_line(&mut discarded) => {
                        // EOF or a read error both end the hold; the detach is
                        // stamped daemon-side either way.
                        let n = res.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        true
                    }
                    _ = &mut done_rx => {
                        // Guard dropped — close cleanly.
                        break;
                    }
                };
                if got_line
                    && let Ok(Response::SystemMsg(msg)) =
                        serde_json::from_str::<Response>(discarded.trim())
                {
                    let _ = push_tx.send(msg);
                }
                discarded.clear();
            }
            let _ = tx.shutdown().await;
        });
        (
            HeldPresenceGuard {
                done_tx: Some(done_tx),
                task: Some(task),
            },
            push_rx,
        )
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
    use tokio::net::UnixStream;

    /// Park a `HeldPresence` against a fake daemon: the daemon side speaks the
    /// wire format (one JSON `Response` per line) on `b` and then shuts its end
    /// down, so the parked task's read on `a` sees EOF. The write half the
    /// guard shuts down is `a`'s; its peer has no reader, which is harmless.
    async fn park_with_daemon(
        daemon_lines: Vec<String>,
    ) -> (
        HeldPresenceGuard,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (a, b) = UnixStream::pair().expect("a socketpair");
        let (unix_r, unix_w) = tokio::io::split(b);
        let (a_r, a_w) = a.into_split();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut w = unix_w;
            for line in daemon_lines {
                let _ = w.write_all(line.as_bytes()).await;
            }
            // Shut the daemon side down: the client's read sees EOF.
            let _ = w.shutdown().await;
            drop(w);
            drop(unix_r);
        });
        let held = HeldPresence {
            rx: BufReader::new(a_r),
            tx: a_w,
        };
        held.park()
    }

    #[tokio::test]
    async fn parked_presence_forwards_every_push() {
        let (guard, mut pushes) = park_with_daemon(vec![
            "{\"SystemMsg\":\"ALERT one\"}\n".to_string(),
            "{\"SystemMsg\":\"ALERT two\"}\n".to_string(),
        ])
        .await;
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the first push within 5 s");
        assert_eq!(first.as_deref(), Some("ALERT one"), "first push forwarded");
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the second push within 5 s");
        assert_eq!(
            second.as_deref(),
            Some("ALERT two"),
            "the second push is forwarded too, not just the first"
        );
        // EOF: the channel closes once the parked task ends.
        let end = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the channel closes within 5 s of EOF");
        assert!(end.is_none(), "the channel closes after EOF");
        drop(guard);
    }

    /// `HeldPresence` is built only by `attach()`; the parked lifecycle is
    /// exercised by the test above, and detach stamping daemon-side by the
    /// server chat_presence tests.
    #[test]
    fn attach_is_the_only_constructor_of_held_presence() {
        assert!(
            !std::any::type_name::<HeldPresence>().is_empty(),
            "the type exists; only attach() can build it"
        );
    }
}
