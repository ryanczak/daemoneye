use crate::ipc::Response;
use std::time::Duration;

/// Protocol liveness contract: while a turn is in flight the daemon sends
/// *something* at least this often. The CLI's dead-daemon deadlines are
/// derived from this with ≥ 6x margin — change it and they must change too
/// (src/cli/commands/stream.rs).
pub const KEEPALIVE_PERIOD_SECS: u64 = 15;

/// Drive `fut` to completion while sending `Response::KeepAlive` every
/// [`KEEPALIVE_PERIOD_SECS`]. A failed keepalive write means the client is
/// gone — the error propagates immediately, which is deliberate: it turns a
/// vanished client into a prompt turn abort instead of a much later EPIPE.
pub async fn with_keepalive<W, F, T>(tx: &mut W, fut: F) -> anyhow::Result<T>
where
    W: tokio::io::AsyncWriteExt + Unpin + ?Sized,
    F: std::future::Future<Output = T>,
{
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(Duration::from_secs(KEEPALIVE_PERIOD_SECS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // interval fires immediately on first tick; burn it so the first
    // keepalive goes out after one full period, not at once.
    tick.tick().await;
    loop {
        tokio::select! {
            out = &mut fut => return Ok(out),
            _ = tick.tick() => {
                super::response::send_response_split(tx, Response::KeepAlive).await?;
            }
        }
    }
}

/// Inline variant for poll loops that interleave their own `tx` writes:
/// call once per iteration; sends a keepalive when the last one is older
/// than the period.
pub async fn maybe_keepalive<W>(tx: &mut W, last: &mut std::time::Instant) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin + ?Sized,
{
    if last.elapsed() >= Duration::from_secs(KEEPALIVE_PERIOD_SECS) {
        super::response::send_response_split(tx, Response::KeepAlive).await?;
        *last = std::time::Instant::now();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn count_keepalives(data: &[u8]) -> usize {
        data.split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .filter(|line| {
                serde_json::from_slice::<Response>(line)
                    .map(|r| matches!(r, Response::KeepAlive))
                    .unwrap_or(false)
            })
            .count()
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_ticks_while_future_pends() {
        let (mut tx, mut rx) = tokio::io::duplex(1024);
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify_fut = std::sync::Arc::clone(&notify);
        let fut = async move {
            notify_fut.notified().await;
            42
        };
        let task =
            tokio::spawn(
                async move { with_keepalive(&mut tx, fut).await.expect("with_keepalive") },
            );
        // Paused clock: a single big advance only fires the first re-armed
        // timer; step the clock so each 15 s keepalive tick can fire. The
        // pre-loop yield lets the spawned task register its interval at t=0.
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(16)).await;
            tokio::task::yield_now().await;
        }
        notify.notify_one();
        let out = task.await.expect("task should complete");
        assert_eq!(out, 42);
        let mut data = Vec::new();
        rx.read_to_end(&mut data).await.expect("read");
        let n = count_keepalives(&data);
        assert!(
            n >= 3,
            "expected >= 3 keepalives over 46 s, got {n}: {:?}",
            String::from_utf8_lossy(&data)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_returns_future_output() {
        let (mut tx, mut rx) = tokio::io::duplex(1024);
        let out = with_keepalive(&mut tx, async { 42 })
            .await
            .expect("with_keepalive");
        assert_eq!(out, 42);
        drop(tx); // close the write half so read_to_end hits EOF
        let mut data = Vec::new();
        rx.read_to_end(&mut data).await.expect("read");
        assert_eq!(
            count_keepalives(&data),
            0,
            "no keepalive expected before the first period"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_write_failure_aborts() {
        let (mut tx, rx) = tokio::io::duplex(1024);
        drop(rx);
        let fut = std::future::pending::<()>();
        let task = tokio::spawn(async move { with_keepalive(&mut tx, fut).await });
        tokio::time::advance(Duration::from_secs(16)).await;
        tokio::task::yield_now().await;
        let res = task.await.expect("task should terminate with an error");
        assert!(
            res.is_err(),
            "dropped client must abort the wait with an error"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn maybe_keepalive_respects_period() {
        let (mut tx, mut rx) = tokio::io::duplex(1024);
        let mut last = std::time::Instant::now();
        maybe_keepalive(&mut tx, &mut last)
            .await
            .expect("maybe_keepalive");
        // std::time::Instant is not governed by tokio's paused clock, so age
        // the marker directly rather than advancing the runtime clock.
        last = std::time::Instant::now()
            .checked_sub(Duration::from_secs(16))
            .expect("age sub");
        maybe_keepalive(&mut tx, &mut last)
            .await
            .expect("maybe_keepalive");
        tokio::task::yield_now().await;
        drop(tx); // close the write half so read_to_end hits EOF
        let mut data = Vec::new();
        rx.read_to_end(&mut data).await.expect("read");
        assert_eq!(count_keepalives(&data), 1, "exactly one keepalive expected");
    }
}
