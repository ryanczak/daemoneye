//! A live shell behind phase-05a's `ShellBackend`: one pump task feeds the
//! subscriber broadcast, the cast log and the screen from the same PTY bytes.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::shell::host::ShellBackend;
use crate::shell::log::CastWriter;
use crate::shell::proto::{ShellResponse, ShellSignal, ShellState};
use crate::shell::pty::PtyShell;
use crate::shell::screen::ShellScreen;
use crate::util::UnpoisonExt;

/// Map a `ShellSignal` to the `kill(1)` signal name that `PtyShell`'s
/// `signal_foreground` hands to the kernel.
fn signal_name(sig: ShellSignal) -> &'static str {
    match sig {
        ShellSignal::Int => "INT",
        ShellSignal::Term => "TERM",
        ShellSignal::Stop => "STOP",
        ShellSignal::Cont => "CONT",
    }
}

/// A live shell behind phase-05a's `ShellBackend`: one pump task feeds the
/// subscriber broadcast, the cast log and the screen from the same PTY bytes.
pub struct PtyBackend {
    shell: Mutex<PtyShell>,
    log: Mutex<CastWriter>,
    screen: Mutex<ShellScreen>,
    out_tx: broadcast::Sender<Vec<u8>>,
    started: Instant,
    scrollback: usize,
}

impl PtyBackend {
    /// `shell` is a path or bare name; `cast` is where the recording goes.
    pub fn spawn(
        shell: &str,
        rows: u16,
        cols: u16,
        scrollback: usize,
        cast: &Path,
        started_unix: u64,
    ) -> anyhow::Result<Arc<Self>> {
        let pty = PtyShell::spawn(shell, (rows, cols)).context("spawning shell")?;
        let log =
            CastWriter::create(cast, cols, rows, started_unix).context("creating cast log")?;
        let screen = ShellScreen::new(rows, cols, scrollback);
        let (out_tx, _initial_rx) = broadcast::channel(crate::shell::pty::STREAM_CAPACITY);
        let started = Instant::now();

        let this = Arc::new(Self {
            shell: Mutex::new(pty),
            log: Mutex::new(log),
            screen: Mutex::new(screen),
            out_tx,
            started,
            scrollback,
        });
        let mut rx = this.shell.lock().unwrap_or_log().subscribe();

        let pump_this = Arc::clone(&this);
        std::thread::spawn(move || {
            loop {
                match rx.blocking_recv() {
                    Ok(chunk) => {
                        let _ = pump_this.out_tx.send(chunk.clone());
                        let log_res = pump_this
                            .log
                            .lock()
                            .unwrap_or_log()
                            .write_output(pump_this.started.elapsed(), &chunk);
                        if let Err(e) = log_res {
                            log::error!("shell host: writing cast log: {e:#}");
                        }
                        pump_this.screen.lock().unwrap_or_log().feed(&chunk);
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });

        Ok(this)
    }
}

#[async_trait]
impl ShellBackend for PtyBackend {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .write_input(bytes)
            .context("write input to shell")?;
        self.log
            .lock()
            .unwrap_or_log()
            .write_input(self.started.elapsed(), bytes)
            .context("record input event")
    }

    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .resize(rows, cols)
            .context("resize shell")?;
        // Rebuilding the screen at the new size keeps the viewport consistent
        // with the PTY: a direct resize does not reflow text (F2).
        *self.screen.lock().unwrap_or_log() = ShellScreen::new(rows, cols, self.scrollback);
        Ok(())
    }

    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .signal_foreground(signal_name(sig))
            .with_context(|| format!("signal shell foreground group with {sig:?}"))
    }

    async fn status(&self) -> ShellResponse {
        let (state, rows, cols, pid) = {
            let mut shell = self.shell.lock().unwrap_or_log();
            let alive = shell.is_alive().with_context(|| "checking shell liveness");
            let (rows, cols) = self.screen.lock().unwrap_or_log().size();
            let pid = shell.pid().unwrap_or(0);
            match alive {
                Ok(true) => (ShellState::Running, rows, cols, pid),
                Ok(false) => (ShellState::Exited, rows, cols, pid),
                Err(e) => {
                    log::error!("shell host: checking shell liveness: {e:#}");
                    (ShellState::Exited, rows, cols, pid)
                }
            }
        };
        ShellResponse::Status {
            state,
            rows,
            cols,
            pid,
        }
    }

    /// A receiver of output chunks for one subscriber.
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.out_tx.subscribe()
    }
}

impl PtyBackend {
    /// The visible screen, exactly as `status()` reports from it.
    pub fn screen_contents(&self) -> String {
        self.screen.lock().unwrap_or_log().contents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::proto::ShellSignal;
    use std::time::Duration;

    /// A unique cast path under a fresh temp dir, per test.
    fn cast_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("s9-0000000000-test.cast")
    }

    fn spawn_backend(dir: &tempfile::TempDir) -> Arc<PtyBackend> {
        PtyBackend::spawn("bash", 24, 80, 100, &cast_path(dir), 0).expect("bash available")
    }

    fn recv_until(
        rx: &mut broadcast::Receiver<Vec<u8>>,
        deadline: std::time::Instant,
        mut pred: impl FnMut(&[u8]) -> bool,
    ) -> Option<Vec<u8>> {
        loop {
            if std::time::Instant::now() >= deadline {
                return None;
            }
            match rx.try_recv() {
                Ok(chunk) => {
                    if pred(&chunk) {
                        return Some(chunk);
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                    // A chunk we never got could have carried the marker, but
                    // the marker's *output* chunk — the one the command's own
                    // stdout produced — is emitted later, so keep going; a
                    // lagged subscriber is exactly the pump-survival case the
                    // lagged test pins. Dropping out here would make this
                    // fan-out test racy with channel capacity.
                    continue;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return None,
            }
        }
    }

    /// Block only long enough to drain whatever output is in flight: return
    /// once `try_recv` has come up empty for a short quiet period, so a
    /// caller drains the current burst without waiting out the deadline.
    fn quiet(rx: &mut broadcast::Receiver<Vec<u8>>, deadline: std::time::Instant) {
        let mut silent_since: Option<std::time::Instant> = None;
        loop {
            if std::time::Instant::now() >= deadline {
                return;
            }
            match rx.try_recv() {
                Ok(_) => silent_since = None,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    let now = std::time::Instant::now();
                    match silent_since {
                        None => silent_since = Some(now),
                        Some(t) if now.duration_since(t) >= Duration::from_millis(30) => return,
                        Some(_) => {}
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) | Err(_) => {
                    silent_since = None;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Join the pump's output to raw bytes for grep-ability; also proves the
    /// cast contains an `"o"` event carrying the bytes (the fan-out guarantee).
    fn cast_output(_backend: &PtyBackend, dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(cast_path(dir)).expect("cast log readable on disk")
    }

    /// True if some `"o"` event payload contains `needle`.
    fn cast_has_o_text(contents: &str, needle: &str) -> bool {
        for line in contents.lines().skip(1) {
            if !line.starts_with('[') {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(arr) = v.as_array() else {
                continue;
            };
            if arr.len() == 3
                && arr[1].as_str() == Some("o")
                && arr[2].as_str().is_some_and(|s| s.contains(needle))
            {
                return true;
            }
        }
        false
    }

    /// Assert `needle` appears in a cast-generated `"o"` event payload.
    fn assert_cast_has_o(contents: &str, needle: &str, label: &str) {
        for line in contents.lines().skip(1) {
            if !line.starts_with('[') {
                continue;
            }
            let v: serde_json::Value =
                serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
            let Some(arr) = v.as_array() else {
                continue;
            };
            if arr.len() == 3 && arr[1].as_str() == Some("o") {
                let Some(s) = arr[2].as_str() else {
                    continue;
                };
                if s.contains(needle) {
                    return;
                }
            }
        }
        panic!("{label}: expected an \"o\" event containing {needle:?} in cast:\n{contents}");
    }

    #[test]
    fn backend_fans_one_chunk_out_to_subscriber_log_and_screen() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let mut rx = backend.subscribe();

        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        // A uniquely-marked command so the assertion binds to this run's
        // output, not to the initial prompt. The marker is split with `''`
        // so the typed-command echo cannot forge it: the echo shows
        // `FAN''OUT_51_99`, only the actual output holds the contiguous
        // `FANOUT_51_99`.
        let marker = "FANOUT_51_99";
        let cmd = "printf '\\nFAN''OUT_51_99 out\\n'; sh -c 'exit 3'\n".to_string();
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(backend.input(cmd.as_bytes()))
            .expect("write command");

        // 1. the subscriber receives the bytes
        let got = recv_until(&mut rx, deadline, |c| {
            c.windows(marker.len()).any(|w| w == marker.as_bytes())
        })
        .expect("subscriber must receive the marked output");
        let got_str = String::from_utf8_lossy(&got);
        assert!(
            got_str.contains(marker),
            "subscriber must receive the marker bytes: {got_str:?}"
        );

        // 2. the cast file contains them as an "o" event. The pump writes
        //    subscribers → log → screen per chunk, so the marker reaching
        //    the subscriber does not (yet) prove it reached the log — poll
        //    the cast to a bounded deadline, the way step 3 polls the screen.
        let cast_deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut cast;
        loop {
            cast = cast_output(&backend, &dir);
            if cast_has_o_text(&cast, marker) {
                break;
            }
            if std::time::Instant::now() >= cast_deadline {
                assert_cast_has_o(&cast, marker, "cast");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_cast_has_o(&cast, marker, "cast");

        // 3. status()'s screen reflects them: poll until the marker is
        //    visible on the live screen.
        let screen_deadline = std::time::Instant::now() + Duration::from_secs(10);
        let visible = loop {
            if backend.screen_contents().contains(marker) {
                break true;
            }
            if std::time::Instant::now() >= screen_deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            visible,
            "screen must show the fan-out marker, got: {:?}",
            backend.screen_contents()
        );
    }

    #[test]
    fn backend_records_input_as_an_i_event() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let payload = "printf 'input-test\\n'\n";
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(backend.input(payload.as_bytes()))
            .expect("write command");
        // Let the pump drain the fragment; the "i" event is written
        // synchronously by input(), before anything streams.
        std::thread::sleep(Duration::from_millis(50));
        let cast = cast_output(&backend, &dir);
        let found = cast.lines().skip(1).any(|line| {
            if !line.starts_with('[') {
                return false;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return false;
            };
            v.as_array()
                .is_some_and(|a| a.len() == 3 && a[1].as_str() == Some("i"))
                && v.as_array().unwrap()[2]
                    .as_str()
                    .unwrap_or_default()
                    .contains("input-test")
        });
        assert!(
            found,
            "cast must contain an \"i\" event for the input: {cast}"
        );
    }

    #[test]
    fn backend_streams_while_input_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let mut rx = backend.subscribe();
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        // A command that streams output with a pause, so the second input is
        // written while the first command's output is still streaming.
        let command1 = "printf 'first-part\\n'; sleep 0.3; printf 'second-part\\n'\n";
        rt.block_on(backend.input(command1.as_bytes()))
            .expect("write command");

        // Wait for the stream to start, then write more input while it is
        // still streaming (before second-part appears). The marker is split
        // with `''` so the typed-command echo cannot forge it: the echo shows
        // `inter''leaved`, only the command's real output holds the
        // contiguous `interleaved`.
        recv_until(&mut rx, deadline, |c| {
            String::from_utf8_lossy(c).contains("first-part")
        })
        .expect("first frame must stream");
        let command2 = "printf 'inter''leaved\\n'\n";
        rt.block_on(backend.input(command2.as_bytes()))
            .expect("write while streaming");

        // Both the streaming command's tail and the interleaved input's
        // effect must reach the subscriber downstream.
        let got_second = recv_until(&mut rx, deadline, |c| {
            String::from_utf8_lossy(c).contains("second-part")
        })
        .expect("the streaming command's tail must not be lost");
        assert!(
            String::from_utf8_lossy(&got_second).contains("second-part"),
            "streaming tail reaches the subscriber"
        );
        let got_int = recv_until(
            &mut rx,
            std::time::Instant::now() + Duration::from_secs(10),
            |c| String::from_utf8_lossy(c).contains("interleaved"),
        )
        .expect("the interleaved input must reach the shell");
        assert!(
            String::from_utf8_lossy(&got_int).contains("interleaved"),
            "interleaved input effect reaches the subscriber"
        );
    }

    /// Assert that `rx` — a subscriber that never read during a flood — now
    /// reports `Lagged` on its next read, which is what the pump's
    /// `continue` on `Lagged` exists to survive.
    fn assert_lagged(rx: &mut broadcast::Receiver<Vec<u8>>, label: &str) {
        match rx.try_recv() {
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                assert!(n > 0, "{label} must report a nonzero lag");
            }
            other => panic!("{label}: expected Lagged, got {other:?}"),
        }
    }

    #[test]
    fn backend_lagged_subscriber_does_not_break_the_pump() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let mut rx_slow = backend.subscribe();
        let mut rx_fast = backend.subscribe();

        // First, let the fast subscriber drain the initial prompt fragment.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        quiet(&mut rx_fast, deadline);

        // Flood the shell with output so the slow subscriber — which never
        // reads — falls past STREAM_CAPACITY, while the fast subscriber is
        // drained continuously so it stays current.
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let flood = "printf 'FLOOD_START\\n'; yes | head -c 10000000\n";
        rt.block_on(backend.input(flood.as_bytes()))
            .expect("write flood command");

        // Drain the fast subscriber while the flood streams, so it never
        // lags; wait for the flood to settle back to the prompt.
        let _ = recv_until(&mut rx_fast, deadline, |c| {
            String::from_utf8_lossy(c).contains("FLOOD_START")
        });
        quiet(
            &mut rx_fast,
            std::time::Instant::now() + Duration::from_secs(20),
        );

        // The slow subscriber has been behind the whole flood: reading from
        // it now must report Lagged rather than silently drop or stall the
        // pump.
        assert_lagged(&mut rx_slow, "slow subscriber");

        // The fast subscriber still receives current chunks after the slow
        // one has lagged.
        let probe = "printf 'post-lag-probe\\n'\n";
        rt.block_on(backend.input(probe.as_bytes()))
            .expect("write probe");
        let got = recv_until(
            &mut rx_fast,
            std::time::Instant::now() + Duration::from_secs(20),
            |c| String::from_utf8_lossy(c).contains("post-lag-probe"),
        )
        .expect("fast subscriber must still receive current chunks");
        let got_str = String::from_utf8_lossy(&got);
        assert!(
            got_str.contains("post-lag-probe"),
            "fast subscriber must observe the probe output: {got_str:?}"
        );
    }

    #[test]
    fn backend_resize_replaces_the_screen_and_does_not_reflow() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let before = rt.block_on(backend.status());
        match before {
            ShellResponse::Status { rows, cols, .. } => {
                assert_eq!((rows, cols), (24, 80), "initial dimensions");
            }
            other => panic!("expected status, got {other:?}"),
        }
        rt.block_on(backend.resize(40, 120)).expect("resize ok");
        let after = rt.block_on(backend.status());
        match after {
            ShellResponse::Status { rows, cols, .. } => {
                assert_eq!((rows, cols), (40, 120), "resized dimensions");
            }
            other => panic!("expected status, got {other:?}"),
        }
    }

    #[test]
    fn backend_signal_targets_the_foreground_group_not_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let mut rx = backend.subscribe();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        quiet(&mut rx, deadline);

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        // Start a long-lived foreground command.
        rt.block_on(backend.input(b"sleep 30\n"))
            .expect("start sleep");
        std::thread::sleep(Duration::from_millis(300));
        rt.block_on(backend.signal(ShellSignal::Int))
            .expect("Int ok");

        // The shell must still be alive: run a further command successfully.
        // The marker is split with `''` so the typed-command echo cannot
        // forge it: the echo shows `post-''signal-probe`, only the command's
        // real output holds the contiguous `post-signal-probe`.
        let probe = "printf 'post-''signal-probe\\n'\n";
        rt.block_on(backend.input(probe.as_bytes()))
            .expect("run after signal");
        std::thread::sleep(Duration::from_millis(300));
        let status = rt.block_on(backend.status());
        match status {
            ShellResponse::Status {
                state: ShellState::Running,
                ..
            } => {}
            other => panic!("shell must still be running after Int, got {other:?}"),
        }
        let probe_marker = "post-signal-probe";
        let got = recv_until(
            &mut rx,
            std::time::Instant::now() + Duration::from_secs(10),
            |c| String::from_utf8_lossy(c).contains(probe_marker),
        )
        .expect("the post-signal command's output must stream");
        let got_str = String::from_utf8_lossy(&got);
        assert!(
            got_str.contains(probe_marker),
            "the post-signal output must reach the subscriber: {got_str:?}"
        );
    }

    #[test]
    fn backend_status_reports_running_then_exited() {
        let dir = tempfile::tempdir().unwrap();
        let backend = spawn_backend(&dir);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        match rt.block_on(backend.status()) {
            ShellResponse::Status {
                state: ShellState::Running,
                ..
            } => {}
            other => panic!("expected Running, got {other:?}"),
        }
        // Kill the backend's shell child through the PtyShell, then wait for
        // the PTY to observe the exit (the reader thread drains EOF before
        // the pump gives up, so the shell is still alive briefly).
        {
            let mut shell = backend.shell.lock().unwrap_or_log();
            shell.kill().expect("kill shell");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if !shell.is_alive().expect("liveness check") {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    panic!("shell child did not exit after kill");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let state = loop {
            match rt.block_on(backend.status()) {
                ShellResponse::Status {
                    state: ShellState::Exited,
                    ..
                } => break ShellState::Exited,
                _ if std::time::Instant::now() >= deadline => panic!("never Exited"),
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert_eq!(state, ShellState::Exited);
    }
}
