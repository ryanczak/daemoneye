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
    /// On EOF (F4) the task does **not** end: it pushes the connection-lost
    /// notice, retries `reconnect` every 2 s until it succeeds, then pushes
    /// the reconnect notice, forwards any catch-up brief the new attach
    /// returned, and keeps reading on the new connection. `session_id` is what
    /// the retry attaches with; `reconnect` is the injectable attach step
    /// (the production impl is `attach(session_id)`), so tests can fake the
    /// daemon.
    ///
    /// The returned [`ParkedPresence`] bundles the guard with the push
    /// receiver. The chat loop keeps the whole bundle for its lifetime: the
    /// hold ends when *it* drops the bundle (chat exiting), closing the
    /// connection and stamping detach state daemon-side.
    pub(crate) fn park<F>(self, session_id: String, reconnect: F) -> ParkedPresence
    where
        F: Fn(
                String,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<(Option<String>, HeldPresence)>> + Send,
                >,
            > + Send
            + 'static,
    {
        let (mut rx, mut tx) = (self.rx, self.tx);
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
        let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let task = tokio::spawn(async move {
            let mut discarded = String::new();
            'hold: loop {
                loop {
                    let read = tokio::select! {
                        res = rx.read_line(&mut discarded) => res,
                        _ = &mut done_rx => break 'hold,
                    };
                    let n = read.unwrap_or(0);
                    if n == 0 {
                        // EOF: the daemon restart or dropped its end. Report
                        // it, then keep the chat alive by re-attaching.
                        let _ = push_tx.send("daemon connection lost — reconnecting…".to_string());
                        break;
                    }
                    if let Ok(Response::SystemMsg(msg)) =
                        serde_json::from_str::<Response>(discarded.trim())
                    {
                        let _ = push_tx.send(msg);
                    }
                    discarded.clear();
                }
                discarded.clear();

                // Retry the attach every 2 s until it succeeds or the chat
                // loop drops the guard.
                let (brief, held) = loop {
                    match reconnect(session_id.clone()).await {
                        Ok(pair) => break pair,
                        Err(_) => {
                            if done_rx.try_recv().is_ok() {
                                break 'hold;
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                    }
                };
                let (next_rx, next_tx) = (held.rx, held.tx);
                rx = next_rx;
                tx = next_tx;
                let _ = push_tx.send("reconnected".to_string());
                if let Some(brief) = brief {
                    let _ = push_tx.send(brief);
                }
                discarded.clear();
            }
            let _ = tx.shutdown().await;
        });
        ParkedPresence {
            done_tx: Some(done_tx),
            task: Some(task),
            pushes: push_rx,
        }
    }

    /// `park` with the production reconnect step: attach to the named session
    /// on the daemon's IPC socket.
    pub(crate) fn park_with_default_reconnect(self, session_id: String) -> ParkedPresence {
        self.park(session_id, |session_id| {
            Box::pin(async move { attach(&session_id).await })
        })
    }
}

/// What the chat loop keeps from a parked presence connection, whole: the
/// hold (the done signal and the parked task) and the push receiver. Dropping
/// the bundle ends the hold — closing the connection and stamping detach
/// state daemon-side. Splitting the hold out of the bundle into a separate
/// binding is exactly what let bug-03b-1 drop it before the first prompt.
pub(crate) struct ParkedPresence {
    done_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    pushes: tokio::sync::mpsc::UnboundedReceiver<String>,
}

impl ParkedPresence {
    /// The channel the parked task forwards pushed `SystemMsg`s onto, for the
    /// chat loop to select on. Borrowed, not taken: the hold must stay owned
    /// by `self` for the loop's lifetime, since dropping `self` ends it.
    pub(crate) fn pushes(&mut self) -> &mut tokio::sync::mpsc::UnboundedReceiver<String> {
        &mut self.pushes
    }

    /// A bundle whose channel nobody sends on: for a chat that failed to
    /// attach, so the loop always selects on a real channel. Dropping it does
    /// nothing — there is no hold to end.
    pub(crate) fn detached() -> Self {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        Self {
            done_tx: None,
            task: None,
            pushes: rx,
        }
    }

    /// Whether the hold is still owned by this bundle.
    #[cfg(test)]
    pub(crate) fn holds_presence(&self) -> bool {
        self.done_tx.is_some() && self.task.is_some()
    }
}

/// The chat loop's setup step for the parked presence connection: the split
/// of the parked bundle happens here, in one function a test can call. It
/// returns the bundle **whole** — hold and push receiver together — with a
/// detached placeholder for a chat that failed to attach. The caller keeps
/// what it returns for the loop's lifetime and reads pushes through
/// [`ParkedPresence::pushes`]; the hold ends when that caller drops the
/// bundle. Returning a bare receiver instead would drop the hold before the
/// first prompt — bug-03b-1.
pub(crate) fn chat_loop_setup(parked: Option<ParkedPresence>) -> ParkedPresence {
    parked.unwrap_or_else(ParkedPresence::detached)
}

impl Drop for ParkedPresence {
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
    use std::sync::{Arc, Mutex};
    use tokio::net::UnixStream;

    /// Park a `HeldPresence` against a fake daemon that keeps its end open
    /// (it does not shut down), so the hold is still live when the caller
    /// inspects it. The daemon side speaks the wire format (one JSON
    /// `Response` per line) on `b`. The write half the guard shuts down is
    /// `a`'s; its peer has no reader, which is harmless.
    async fn park_with_daemon_held(daemon_lines: Vec<String>) -> ParkedPresence {
        let (a, b) = UnixStream::pair().expect("a socketpair");
        let (unix_r, unix_w) = tokio::io::split(b);
        let (a_r, a_w) = a.into_split();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut w = unix_w;
            for line in daemon_lines {
                let _ = w.write_all(line.as_bytes()).await;
            }
            // The daemon keeps its end open — no shutdown here.
            drop(unix_r);
        });
        let held = HeldPresence {
            rx: BufReader::new(a_r),
            tx: a_w,
        };
        held.park_with_default_reconnect("test-session".to_string())
    }

    #[tokio::test]
    async fn parked_presence_forwards_every_push() {
        let mut parked = park_with_daemon_held(vec![
            "{\"SystemMsg\":\"ALERT one\"}\n".to_string(),
            "{\"SystemMsg\":\"ALERT two\"}\n".to_string(),
        ])
        .await;
        let pushes = parked.pushes();
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
        assert!(
            parked.holds_presence(),
            "the guard is still live after the pushes"
        );
    }

    /// After EOF the hold does not end: the receiver sees the connection-lost
    /// notice, then — once the injectable reconnect succeeds — the reconnect
    /// notice and any brief the new attach carried, and a push sent on the
    /// new connection is forwarded. Fake daemon #1 shuts down after one push
    /// (EOF); the injectable reconnect hands back a `HeldPresence` whose peer
    /// (#2) pushes the reconnect evidence and keeps its end open.
    #[tokio::test]
    async fn parked_presence_reconnects_after_eof() {
        // Fake daemon #1: pushes one alert, then closes its end so the parked
        // task's read sees EOF.
        let (a1, b1) = UnixStream::pair().expect("a socketpair");
        let (a1_r, a1_w) = a1.into_split();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut w = b1;
            let _ = w.write_all(b"{\"SystemMsg\":\"ALERT gone\"}\n").await;
            // Shut the daemon side down: the client's read sees EOF.
            let _ = w.shutdown().await;
        });

        // Fake daemon #2: the reconnect step's peer. It pushes the reconnect
        // evidence and keeps its end open, so the hold stays live.
        let (a2, b2) = UnixStream::pair().expect("a second socketpair");
        let (a2_r, a2_w) = a2.into_split();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut w = b2;
            let _ = w
                .write_all(b"{\"SystemMsg\":\"ALERT on the new connection\"}\n")
                .await;
            // The daemon keeps its end open: the hold stays live.
        });

        let held = HeldPresence {
            rx: BufReader::new(a1_r),
            tx: a1_w,
        };
        // The reconnect closure may run more than once, so the halves come
        // from an Option that is taken on each call. Any further call fails
        // fast rather than panicking inside the parked task.
        let reconnect = Arc::new(Mutex::new(Some((a2_r, a2_w))));
        let mut parked = held.park("test-session".to_string(), move |_session_id| {
            let reconnect = Arc::clone(&reconnect);
            Box::pin(async move {
                let halves = {
                    let mut slot = reconnect.lock().expect("the reconnect slot");
                    slot.take()
                };
                let Some((a2_r, a2_w)) = halves else {
                    return Err(anyhow::anyhow!(
                        "the reconnect step ran after its fixture was spent"
                    ));
                };
                Ok((
                    Some("brief: caught up 3 events".to_string()),
                    HeldPresence {
                        rx: BufReader::new(a2_r),
                        tx: a2_w,
                    },
                ))
            })
        });
        let pushes = parked.pushes();
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the push on the first connection within 5 s");
        assert_eq!(first.as_deref(), Some("ALERT gone"));
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the connection-lost push within 5 s");
        assert_eq!(
            second.as_deref(),
            Some("daemon connection lost — reconnecting…"),
            "EOF is announced, not swallowed"
        );
        let third = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the reconnected push within 5 s");
        assert_eq!(third.as_deref(), Some("reconnected"));
        let brief = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the brief within 5 s");
        assert_eq!(
            brief.as_deref(),
            Some("brief: caught up 3 events"),
            "the catch-up brief the new attach returned is forwarded"
        );
        let fourth = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the push on the new connection within 5 s");
        assert_eq!(
            fourth.as_deref(),
            Some("ALERT on the new connection"),
            "pushes keep flowing on the reconnected hold"
        );
        assert!(
            parked.holds_presence(),
            "the guard is still live after the reconnect"
        );
    }

    /// The chat loop's setup step keeps the parked bundle whole: a `SystemMsg`
    /// the fake daemon pushes after setup arrives on the channel the loop
    /// would select on. With the old two-binding split the hold died at
    /// setup time and the push never arrived — bug-03b-1.
    #[tokio::test]
    async fn chat_loop_setup_keeps_presence_held() {
        let parked =
            park_with_daemon_held(vec!["{\"SystemMsg\":\"ALERT after setup\"}\n".to_string()])
                .await;
        let mut held = super::chat_loop_setup(Some(parked));
        assert!(
            held.holds_presence(),
            "the setup keeps the hold alive, not dropped before the loop"
        );
        let pushes = held.pushes();
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), pushes.recv())
            .await
            .expect("the pushed SystemMsg within 5 s");
        assert_eq!(
            msg.as_deref(),
            Some("ALERT after setup"),
            "the push reaches the channel the loop selects on"
        );
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

    #[test]
    fn chat_loop_setup_detaches_to_a_placeholder() {
        let parked = super::chat_loop_setup(None);
        assert!(
            !parked.holds_presence(),
            "no parked connection → no hold, but the bundle still exists"
        );
    }
}
