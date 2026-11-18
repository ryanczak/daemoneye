//! The notice-subscription handler: one IPC connection held open, each
//! matching bus notice forwarded as `Response::Notice` until the client
//! hangs up (M38 phase 10b).

use anyhow::Result;

use crate::bus::{Bus, Filter};
use crate::daemon::utils::send_response_split;
use crate::ipc::Response;

/// Capacity of the subscription's queue, in notices. Beyond it, the bus
/// drops for this subscriber only.
const SUBSCRIBE_CAPACITY: usize = 256;

/// Subscribe the connection to the bus: ack first, then forward every
/// matching notice until the client's read half reaches EOF or errors, or a
/// write fails. Returning is never an error — the client hanging up is the
/// normal unsubscribe (the bus prunes the dropped receiver on the next
/// publish).
pub(super) async fn handle_subscribe<R, W>(
    rx: &mut R,
    tx: &mut W,
    bus: &Bus,
    filter: Filter,
) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut queue = bus.subscribe("ipc-subscriber", filter, SUBSCRIBE_CAPACITY);
    send_response_split(tx, Response::Ok).await?;

    let mut client = rx.lines();
    loop {
        tokio::select! {
            notice = queue.recv() => match notice {
                Some(notice) => {
                    if send_response_split(tx, Response::Notice { notice: (*notice).clone() })
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                // All senders are gone — impossible while this connection
                // holds the only receiver; treat it as the end anyway.
                None => return Ok(()),
            },
            line = client.next_line() => match line {
                Ok(Some(_)) => {}
                // The client hung up; stop streaming.
                Ok(None) | Err(_) => return Ok(()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Notice, Severity, Source};

    fn notice(name: &str) -> Notice {
        Notice::new(Severity::Warning, Source::Webhook, name, "cpu high")
    }

    async fn read_frame(rx: &mut tokio::io::DuplexStream) -> Response {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        // Read one newline-delimited JSON frame.
        loop {
            let mut byte = [0u8; 1];
            let n = rx.read(&mut byte).await.expect("read frame byte");
            assert_eq!(n, 1, "the stream closed mid-frame");
            if byte[0] == b'\n' {
                break;
            }
            buf.push(byte[0]);
        }
        serde_json::from_slice(&buf).expect("parse response frame")
    }

    #[tokio::test]
    async fn subscribe_acks_then_streams_matching_notices() {
        let bus = std::sync::Arc::new(crate::bus::Bus::with_journal(|_| {}));
        let filter = Filter {
            sources: vec![Source::Webhook],
            ..Filter::default()
        };
        let (mut client, server_side) = tokio::io::duplex(4096);
        let (server_rx, mut server_tx) = tokio::io::split(server_side);
        let mut server_rx = tokio::io::BufReader::new(server_rx);

        let bus_for_handler = std::sync::Arc::clone(&bus);
        let handler = tokio::spawn(async move {
            handle_subscribe(&mut server_rx, &mut server_tx, &bus_for_handler, filter)
                .await
                .expect("handler runs");
        });

        let ack = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut client))
            .await
            .expect("the ack arrives in time");
        assert!(
            matches!(ack, Response::Ok),
            "the first frame is the ack, got {ack:?}"
        );

        // Queued subscribers only receive notices published after they
        // subscribed — after the ack, every publish reaches the client. The
        // bus filters: the scheduler notice never enters the queue, so the
        // only forwarded notice is the matching webhook one.
        let non_matching = Notice::new(Severity::Info, Source::Scheduler, "JobDone", "done");
        bus.publish(non_matching);
        bus.publish(notice("HighCPU"));

        let stream =
            tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut client))
                .await
                .expect("the streamed notice arrives in time");
        match stream {
            Response::Notice { notice } => {
                assert_eq!(notice.name, "HighCPU", "the matching notice is forwarded");
            }
            other => panic!("expected Response::Notice, got {other:?}"),
        }
        handler.abort();
    }

    #[tokio::test]
    async fn subscribe_ends_when_the_client_disconnects() {
        let bus = std::sync::Arc::new(crate::bus::Bus::with_journal(|_| {}));
        let (mut client, server_side) = tokio::io::duplex(4096);
        let (server_rx, mut server_tx) = tokio::io::split(server_side);
        let mut server_rx = tokio::io::BufReader::new(server_rx);

        let handler = tokio::spawn(async move {
            handle_subscribe(&mut server_rx, &mut server_tx, &bus, Filter::default())
                .await
                .expect("handler runs")
        });

        // Read the ack, then hang up: EOF on the server's read half.
        let ack = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut client))
            .await
            .expect("ack arrives");
        assert!(matches!(ack, Response::Ok));
        drop(client);

        tokio::time::timeout(std::time::Duration::from_secs(5), handler)
            .await
            .expect("the handler ends within the timeout, not an error")
            .expect("the handler task did not panic");
    }
}
