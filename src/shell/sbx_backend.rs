//! The shell backend for a tagged sbx shell host: a `PtyBackend` whose
//! Stop/Cont/Hup/Find reach **inside the VM** through a side `sbx exec`,
//! whose Int/Term arrive as `^C` bytes through the inner PTY, and whose
//! status combines the host client's liveness with the VM probe.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use tokio::sync::broadcast;

use crate::daemon::executor::sbx::{SbxTarget, VmOp, parse_vm_find, vm_op_outcome, vm_signal_args};
use crate::shell::host::ShellBackend;
use crate::shell::proto::{ShellResponse, ShellSignal, ShellState};

/// The result of one `sbx` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmRun {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `sbx <args>`. Production uses [`real_runner`]; tests pass a fake.
pub type SbxRunner = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, VmRun> + Send + Sync>;

pub struct SbxBackend<B: ShellBackend> {
    inner: Arc<B>,
    target: SbxTarget,
    run: SbxRunner,
}

impl<B: ShellBackend> SbxBackend<B> {
    pub fn new(inner: Arc<B>, target: SbxTarget, run: SbxRunner) -> Self {
        Self { inner, target, run }
    }

    /// Run one in-VM signal operation against this shell's target.
    async fn vm(&self, op: VmOp) -> VmRun {
        let argv = vm_signal_args(&self.target.sandbox, &self.target.tag, op);
        (self.run)(argv).await
    }

    /// Turn a `VmOp` outcome into the host's `Result`: `NoShell` is an error
    /// that names the tag so the operator can tell which shell is gone,
    /// `Failed` carries the `sbx` detail verbatim.
    fn op_result(&self, run: &VmRun) -> anyhow::Result<()> {
        match vm_op_outcome(run.code, &run.stdout, &run.stderr) {
            crate::daemon::executor::sbx::VmOpOutcome::Done => Ok(()),
            crate::daemon::executor::sbx::VmOpOutcome::NoShell => anyhow::bail!(
                "no shell tagged {} in sandbox {}",
                self.target.tag,
                self.target.sandbox
            ),
            crate::daemon::executor::sbx::VmOpOutcome::Failed(m) => anyhow::bail!(
                "vm op for tagged shell {} in sandbox {}: {m}",
                self.target.tag,
                self.target.sandbox
            ),
        }
    }
}

#[async_trait]
impl<B: ShellBackend> ShellBackend for SbxBackend<B> {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.inner.input(bytes).await
    }

    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.inner.resize(rows, cols).await
    }

    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()> {
        match sig {
            ShellSignal::Stop => self.op_result(&self.vm(VmOp::Stop).await),
            ShellSignal::Cont => self.op_result(&self.vm(VmOp::Cont).await),
            // Measured: an in-VM `kill -INT` on the foreground group does not
            // end a loop typed at the prompt; `^C` through the PTY does.
            ShellSignal::Int | ShellSignal::Term => self.inner.input(&[0x03]).await,
        }
    }

    async fn status(&self) -> ShellResponse {
        let inner = self.inner.status().await;
        let ShellResponse::Status {
            state: inner_state,
            rows,
            cols,
            pid,
            raw_mode,
            alt_screen,
            foreground,
            ..
        } = &inner
        else {
            return inner;
        };
        let (rows, cols, pid) = (*rows, *cols, *pid);
        let (raw_mode, alt_screen, foreground) = (*raw_mode, *alt_screen, foreground.clone());
        let inner_state = *inner_state;
        if inner_state == ShellState::Exited {
            return inner;
        }
        let run = self.vm(VmOp::Find).await;
        let state = match parse_vm_find(&run.stdout) {
            Some(s) if s.is_paused() => ShellState::Paused,
            Some(s) if s.has_foreground_job() => ShellState::Running,
            Some(_) => ShellState::Idle,
            None => match vm_op_outcome(run.code, &run.stdout, &run.stderr) {
                crate::daemon::executor::sbx::VmOpOutcome::NoShell => ShellState::Exited,
                other => {
                    log::warn!(
                        "shell host: vm status probe for tagged shell {} in sandbox {}: {other:?}; reporting the host's own view",
                        self.target.tag,
                        self.target.sandbox
                    );
                    inner_state
                }
            },
        };
        ShellResponse::Status {
            state,
            rows,
            cols,
            pid,
            raw_mode,
            alt_screen,
            foreground,
        }
    }

    /// Never calls `sbx`: the host client's own liveness is the answer, and
    /// the client exits when the VM shell ends (measured: after `HUP`, after
    /// `sbx rm`). The 250 ms exit watch in `serve_until_exit` polls this.
    async fn is_exited(&self) -> bool {
        matches!(
            self.inner.status().await,
            ShellResponse::Status {
                state: ShellState::Exited,
                ..
            }
        )
    }

    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.inner.subscribe()
    }

    fn subscribe_with_screen(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
        self.inner.subscribe_with_screen()
    }

    /// Delegates to the inner backend: the sbx host's screen is the inner
    /// PTY's screen (the in-VM side renders into it).
    async fn screen_text(&self, annotated: bool, scrollback: u16) -> anyhow::Result<ShellResponse> {
        self.inner.screen_text(annotated, scrollback).await
    }

    async fn close(&self) -> anyhow::Result<()> {
        let run = self.vm(VmOp::Hup).await;
        let result = self.op_result(&run);
        // The client may already be gone — hangup is what ends it.
        let _ = self.inner.close().await;
        result
    }
}

/// `program <args>` with a 10 s bound: a spawn error or the timeout yields a
/// `VmRun` with `code: None` and the problem on stderr, which
/// `vm_op_outcome` maps to `Failed`.
pub fn real_runner(program: String) -> SbxRunner {
    Arc::new(move |args: Vec<String>| {
        let program = program.clone();
        async move {
            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    return VmRun {
                        code: None,
                        stdout: String::new(),
                        stderr: format!("spawning {program}: {e}"),
                    };
                }
            };
            let output = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                child.wait_with_output(),
            )
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return VmRun {
                        code: None,
                        stdout: String::new(),
                        stderr: format!("running {program}: {e}"),
                    };
                }
                Err(_) => {
                    return VmRun {
                        code: None,
                        stdout: String::new(),
                        stderr: format!("{program} did not answer within 10 s"),
                    };
                }
            };
            VmRun {
                code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
        }
        .boxed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::backend::PtyBackend;
    use std::sync::Mutex;

    const CAST: &str = "s9.cast";

    fn cast_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join(CAST)
    }

    fn spawn_inner(dir: &tempfile::TempDir) -> Arc<PtyBackend> {
        let _home = crate::test_home_guard();
        PtyBackend::spawn("bash", &[], 24, 80, 100, &cast_path(dir), 0).expect("bash available")
    }

    /// A fake runner that records every argv and hands back canned runs,
    /// swapping the canned value between calls as the tests need.
    type CannedRuns = Arc<Mutex<Vec<VmRun>>>;

    fn fake_runner(canned: CannedRuns, calls: Arc<Mutex<Vec<Vec<String>>>>) -> SbxRunner {
        Arc::new(move |argv: Vec<String>| {
            calls.lock().unwrap().push(argv.clone());
            let mut queue = canned.lock().unwrap();
            let run = if queue.is_empty() {
                queue.last().cloned().unwrap_or_else(|| VmRun {
                    code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                queue.remove(0)
            };
            async move { run }.boxed()
        })
    }

    fn target() -> SbxTarget {
        SbxTarget {
            sandbox: "de-ghost-t".into(),
            tag: "s9".into(),
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().expect("runtime")
    }

    fn find_run(stdout: &str) -> VmRun {
        VmRun {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn stop_and_cont_run_the_vm_script() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(vec![VmRun {
            code: Some(0),
            stdout: "stopped bash=1 fg=2".into(),
            stderr: String::new(),
        }]));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(Arc::clone(&canned), Arc::clone(&calls)),
        );
        let rt = runtime();
        rt.block_on(async {
            backend
                .signal(ShellSignal::Stop)
                .await
                .expect("stop must succeed");
            backend
                .signal(ShellSignal::Cont)
                .await
                .expect("cont must succeed");
        });
        let calls = calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                vm_signal_args("de-ghost-t", "s9", VmOp::Stop),
                vm_signal_args("de-ghost-t", "s9", VmOp::Cont),
            ],
            "the recorded argv must be the vm script's Stop then Cont forms"
        );
    }

    #[test]
    fn stop_reports_a_missing_vm_shell() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(vec![VmRun {
            code: Some(3),
            stdout: "no-shell\n".into(),
            stderr: String::new(),
        }]));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(canned, Arc::new(Mutex::new(Vec::new()))),
        );
        let rt = runtime();
        let err = rt
            .block_on(backend.signal(ShellSignal::Stop))
            .expect_err("a missing vm shell must be an error");
        assert!(
            err.to_string().contains("no shell"),
            "the error must say `no shell`: {err}"
        );
        assert!(
            err.to_string().contains("s9"),
            "the error must name the tag: {err}"
        );
    }

    #[test]
    fn int_and_term_send_ctrl_c_and_never_call_sbx() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(Arc::new(Mutex::new(Vec::new())), Arc::clone(&calls)),
        );
        let rt = runtime();
        rt.block_on(async {
            backend
                .signal(ShellSignal::Int)
                .await
                .expect("Int must be a ^C write");
            backend
                .signal(ShellSignal::Term)
                .await
                .expect("Term must be a ^C write");
        });
        assert!(
            calls.lock().unwrap().is_empty(),
            "Int and Term must never call sbx"
        );
        let cast = std::fs::read_to_string(cast_path(&dir)).expect("cast readable");
        let count = cast.matches("\\u0003").count();
        assert_eq!(count, 2, "the cast must hold two ^C input payloads: {cast}");
        let i_events = cast
            .lines()
            .filter(|l| l.starts_with('['))
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| {
                v.as_array()
                    .is_some_and(|a| a.len() == 3 && a[1].as_str() == Some("i"))
            })
            .filter(|v| {
                v.as_array().unwrap()[2]
                    .as_str()
                    .is_some_and(|s| s.contains('\u{0003}'))
            })
            .count();
        assert_eq!(
            i_events, 2,
            "the cast must record two `i` events carrying ^C: {cast}"
        );
    }

    #[test]
    fn status_reports_idle_running_and_paused_from_find() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(Vec::new()));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(Arc::clone(&canned), Arc::new(Mutex::new(Vec::new()))),
        );
        let rt = runtime();
        for (find, want) in [
            ("bash=1 fg=1 state=S", ShellState::Idle),
            ("bash=1 fg=2 state=S", ShellState::Running),
            ("bash=1 fg=2 state=T", ShellState::Paused),
        ] {
            canned.lock().unwrap().push(find_run(find));
            let got = rt.block_on(backend.status());
            match got {
                ShellResponse::Status { state, .. } => assert_eq!(
                    state, want,
                    "find output `{find}` must read as {want:?}, got {state:?}"
                ),
                other => panic!("expected a Status reply, got {other:?}"),
            }
        }
    }

    #[test]
    fn status_reports_exited_when_the_vm_shell_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(vec![VmRun {
            code: Some(3),
            stdout: "no-shell".into(),
            stderr: String::new(),
        }]));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(canned, Arc::new(Mutex::new(Vec::new()))),
        );
        let rt = runtime();
        match rt.block_on(backend.status()) {
            ShellResponse::Status {
                state: ShellState::Exited,
                ..
            } => {}
            other => panic!("a gone vm shell must read Exited, got {other:?}"),
        }
    }

    #[test]
    fn status_keeps_the_host_view_when_find_fails() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(vec![VmRun {
            code: Some(1),
            stdout: String::new(),
            stderr: "error: sandbox 'de-ghost-t' not found".into(),
        }]));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(canned, Arc::new(Mutex::new(Vec::new()))),
        );
        let rt = runtime();
        let inner_status = rt.block_on(inner.status());
        let got = rt.block_on(backend.status());
        assert_eq!(
            got, inner_status,
            "a failed find must report the inner backend's own status"
        );
        match got {
            ShellResponse::Status {
                state: ShellState::Idle,
                ..
            } => {}
            other => panic!("an idle bash must read Idle, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn is_exited_never_calls_sbx() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(Arc::new(Mutex::new(Vec::new())), Arc::clone(&calls)),
        );
        assert!(
            !backend.is_exited().await,
            "a live inner bash must not read as exited"
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "is_exited must never call sbx"
        );
    }

    #[tokio::test]
    async fn close_hangs_up_the_vm_shell_then_the_client() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(vec![VmRun {
            code: Some(0),
            stdout: "hup bash=1".into(),
            stderr: String::new(),
        }]));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(canned, Arc::clone(&calls)),
        );
        backend.close().await.expect("close must succeed");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![vm_signal_args("de-ghost-t", "s9", VmOp::Hup)],
            "close must run exactly the Hup argv"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if backend.is_exited().await {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the inner bash must be exited within 10 s of close"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn close_reports_a_failed_hangup() {
        let dir = tempfile::tempdir().unwrap();
        let inner = spawn_inner(&dir);
        let canned: CannedRuns = Arc::new(Mutex::new(vec![VmRun {
            code: Some(1),
            stdout: String::new(),
            stderr: "boom".into(),
        }]));
        let backend = SbxBackend::new(
            Arc::clone(&inner),
            target(),
            fake_runner(canned, Arc::new(Mutex::new(Vec::new()))),
        );
        let rt = runtime();
        let err = rt
            .block_on(backend.close())
            .expect_err("a failed hangup must be an error");
        assert!(
            err.to_string().contains("boom"),
            "the error must carry the vm detail: {err}"
        );
    }
}
