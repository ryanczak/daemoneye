# Phase 05b: sbx-signal-host — the shell host acts on the VM for a sandbox shell

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-05a
**Estimated diff:** ~330 lines (about 140 code, 190 tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

A shell host whose child is `sbx exec -it -e DE_SHELL=<tag> <sandbox> bash`
must pause, resume, cancel, close and report status **inside the VM**. The
host-side defaults act on the local `sbx` client, which freezes nothing
(measured). This phase adds `SbxBackend`, a `ShellBackend` that wraps the
existing `PtyBackend` and overrides those five operations. `run_host` uses it
whenever its child is a tagged sbx shell. Every existing caller (attach
`Ctrl-p`/`Ctrl-c`, run cancel, `close_shell`) then works for sandbox shells
with no change of its own, because they all go through the host's `Signal`,
`Status` and `Close` frames.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/phase-05a-sbx-signal-plan.md`
  § Current state: every behaviour below was measured there.
- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Measured on pinky",
  rows *Pause*, *Cancel* and *Teardown*.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### What 05a gave you (`src/daemon/executor/sbx.rs`, all `pub`)

```rust
pub struct SbxTarget { pub sandbox: String, pub tag: String }
pub fn sbx_target_from_host(program: &str, args: &[String]) -> Option<SbxTarget>
pub enum VmOp { Find, Stop, Cont, Hup }
pub fn vm_signal_args(sandbox: &str, tag: &str, op: VmOp) -> Vec<String>  // without the leading program
pub struct VmShellState { pub bash_pid: u32, pub fg_pgid: u32, pub state: char }
impl VmShellState { pub fn is_paused(&self) -> bool; pub fn has_foreground_job(&self) -> bool }
pub fn parse_vm_find(stdout: &str) -> Option<VmShellState>
pub enum VmOpOutcome { Done, NoShell, Failed(String) }
pub fn vm_op_outcome(exit_code: Option<i32>, stdout: &str, stderr: &str) -> VmOpOutcome
```

### Measured behaviour this phase must produce

| Operation on a sandbox shell | Must do | Why (measured) |
|---|---|---|
| `Signal(STOP)` | run `vm_signal_args(.., VmOp::Stop)` | stopping the host client does not freeze the VM job; the in-VM script does |
| `Signal(CONT)` | run `vm_signal_args(.., VmOp::Cont)` | |
| `Signal(INT)` and `Signal(TERM)` | **write `0x03` (`^C`) into the PTY** through the inner backend's `input` | an in-VM `kill -INT` to the foreground group does **not** end a loop typed at the prompt; `^C` through the PTY does |
| `Close` | run `VmOp::Hup`, then close the inner backend | in-VM `HUP` ends the shell and its jobs, and the host's `sbx` client exits 129; killing only the client leaves the VM job running |
| `Status` | if the inner backend reports `Exited` → `Exited`; else run `VmOp::Find`: `no-shell` → `Exited`; state `T` → `Paused`; a foreground job → `Running`; otherwise `Idle` | the host-side status sees only the `sbx` client, which is always "running" |

### `ShellBackend` (`src/shell/host.rs`)

```rust
#[async_trait]
pub trait ShellBackend: Send + Sync + 'static {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()>;
    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()>;
    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()>;
    async fn status(&self) -> ShellResponse;
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;
    fn subscribe_with_screen(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
        (Vec::new(), self.subscribe())
    }
    async fn close(&self) -> anyhow::Result<()> {
        anyhow::bail!("this shell host cannot close its shell")
    }
}
```

`serve_until_exit` in the same file polls `watched.status()` **every 250 ms**
to detect an exited shell:

```rust
() = async move {
    loop {
        tokio::time::sleep(poll).await;
        if matches!(
            watched.status().await,
            ShellResponse::Status { state: ShellState::Exited, .. }
        ) {
            break;
        }
    }
} => Ok(()),
```

**Gotcha:** for a sandbox shell `status()` runs an `sbx exec` (about 0.4 s,
measured). It must **not** run four times a second. Task 1 adds a cheap
`is_exited()` for this poll.

### `run_host` (`src/shell/launch.rs`)

```rust
let backend = crate::shell::PtyBackend::spawn(
    shell, args, rows, cols, scrollback, &cast_path(id, now), now,
)?;
let listener = crate::shell::host::bind(&socket_path(id)).await?;
crate::shell::host::serve_until_exit(listener, backend, SHELL_EXIT_POLL).await?;
```

`shell` is the program path the daemon spawned (`/usr/bin/sbx` for a sandbox
shell). `args` are its arguments (`exec -it -e DE_SHELL=s3 de-ghost-r1 bash`).

### Test helper to copy (`src/shell/backend.rs` tests)

```rust
fn spawn_backend(dir: &tempfile::TempDir) -> Arc<PtyBackend> {
    let _home = crate::test_home_guard();
    PtyBackend::spawn("bash", &[], 24, 80, 100, &cast_path(dir), 0).expect("bash available")
}
```

A real local `bash` is the right **inner** backend for tests: the inner
backend's job here is the PTY, and the VM is replaced by a fake runner
(Task 2).

## Spec

### Task 1 — `ShellBackend::is_exited` (`src/shell/host.rs`)

Add a provided method to the trait:

```rust
/// Whether the shell has exited. The exit watch in `serve_until_exit` polls
/// this, so it must be cheap; the default asks `status()`.
async fn is_exited(&self) -> bool {
    matches!(self.status().await, ShellResponse::Status { state: ShellState::Exited, .. })
}
```

In `serve_until_exit`, replace the `matches!(watched.status().await, …)`
condition with `watched.is_exited().await`. `PtyBackend` does **not**
override it, so its behaviour is unchanged.

### Task 2 — `SbxBackend` (new file `src/shell/sbx_backend.rs`)

```rust
/// The result of one `sbx` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmRun { pub code: Option<i32>, pub stdout: String, pub stderr: String }

/// Runs `sbx <args>`. Production uses `real_runner`; tests pass a fake.
pub type SbxRunner = Arc<dyn Fn(Vec<String>) -> futures_util::future::BoxFuture<'static, VmRun> + Send + Sync>;

pub struct SbxBackend<B: ShellBackend> {
    inner: Arc<B>,
    target: crate::daemon::executor::sbx::SbxTarget,
    run: SbxRunner,
}

impl<B: ShellBackend> SbxBackend<B> {
    pub fn new(inner: Arc<B>, target: SbxTarget, run: SbxRunner) -> Self
    async fn vm(&self, op: VmOp) -> VmRun   // run(vm_signal_args(&target.sandbox, &target.tag, op))
}

/// `program <args>` with a 10 s bound: `tokio::process::Command`, `kill_on_drop(true)`,
/// stdin null, output captured. A spawn error, or the timeout, gives
/// `VmRun { code: None, stdout: "", stderr: "<what happened>" }`.
pub fn real_runner(program: String) -> SbxRunner
```

`futures_util` is already a dependency (`futures-util = "0.3"` in
`Cargo.toml`); `futures_util::future::BoxFuture` and `FutureExt::boxed()` come
from it. **Do not add a dependency.**

`impl<B: ShellBackend> ShellBackend for SbxBackend<B>` (`#[async_trait]`):

- `input`, `resize`, `subscribe`, `subscribe_with_screen`: delegate to `inner`.
- `signal(sig)`:
  - `Stop` → `vm(VmOp::Stop)`;
  - `Cont` → `vm(VmOp::Cont)`;
  - then map with `vm_op_outcome(code, &stdout, &stderr)`: `Done` → `Ok(())`;
    `NoShell` → `Err` whose message contains `no shell` and the tag;
    `Failed(m)` → `Err` containing `m`;
  - `Int` and `Term` → `self.inner.input(&[0x03]).await`. **No `sbx` call.**
- `status()`:
  1. `let inner = self.inner.status().await;` If it is `Status { state: Exited, .. }`,
     return it unchanged.
  2. Otherwise run `vm(VmOp::Find)`. With
     `parse_vm_find(&stdout)` → `Some(s)`: `Paused` if `s.is_paused()`, else
     `Running` if `s.has_foreground_job()`, else `Idle`. With `vm_op_outcome`
     → `NoShell`: `Exited`. Anything else (`Failed`, or `Done` with output
     that does not parse): log a `warn!` naming the tag and return `inner`
     unchanged. A broken probe must never report a live shell as exited.
  3. Return `ShellResponse::Status { state, rows, cols, pid }`, with `rows`,
     `cols` and `pid` taken from `inner`.
- `is_exited()`: **override.** Return whether `self.inner.status().await` is
  `Exited`. This is cheap, never calls `sbx`, and is true once the `sbx`
  client exits (measured: it exits after `HUP` and after `sbx rm`).
- `close()`: run `vm(VmOp::Hup)`, then call `self.inner.close().await` and
  ignore its result (the client may already be gone).
  - `Done` or `NoShell` → `Ok(())`;
  - `Failed(m)` → `Err` containing `m`, so the caller knows the VM shell may
    still be alive.

Declare `mod sbx_backend;` in `src/shell/mod.rs` and add
`pub use sbx_backend::{SbxBackend, SbxRunner, VmRun, real_runner};`.

### Task 3 — `run_host` picks the backend

In `run_host`, after `PtyBackend::spawn(...)?` and `bind(...)`:

```rust
match crate::daemon::executor::sbx::sbx_target_from_host(shell, args) {
    Some(target) => {
        let sbx = Arc::new(crate::shell::SbxBackend::new(
            backend, target, crate::shell::real_runner(shell.to_string()),
        ));
        crate::shell::host::serve_until_exit(listener, sbx, SHELL_EXIT_POLL).await?;
    }
    None => {
        crate::shell::host::serve_until_exit(listener, backend, SHELL_EXIT_POLL).await?;
    }
}
```

`shell` is the program, so the real runner calls the same `sbx` binary the
host was started with.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib shell::sbx_backend` reports **at least 9 passed;
      0 failed**: the nine tests in § Test plan.
- [ ] `cargo test --lib shell::host` and `cargo test --lib shell::backend`
      report `0 failed`; no existing test in those files is edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] `git diff --stat` shows no change to `Cargo.toml`.
- [ ] No `#[allow(`, `unsafe`, `.unwrap()` or `.expect(` added outside
      `#[cfg(test)]` code.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

All in `src/shell/sbx_backend.rs` (`#[cfg(test)] mod tests`). Build the inner
backend as `PtyBackend::spawn("bash", &[], 24, 80, 100, &<tempdir>/s9.cast, 0)`
after `crate::test_home_guard()`, exactly like `spawn_backend` above. Use a
**fake runner** that records each `Vec<String>` it is given (in an
`Arc<Mutex<Vec<Vec<String>>>>`) and returns a canned `VmRun`. The target is
`SbxTarget { sandbox: "de-ghost-t".into(), tag: "s9".into() }`.

- `stop_and_cont_run_the_vm_script`: canned `(Some(0), "stopped bash=1 fg=2", "")`.
  `signal(Stop)` and `signal(Cont)` are `Ok`. The recorded calls equal
  `vm_signal_args("de-ghost-t", "s9", VmOp::Stop)` and the `Cont` form, in
  that order.
- `stop_reports_a_missing_vm_shell`: canned `(Some(3), "no-shell\n", "")` →
  `signal(Stop)` is `Err`, and its message contains `no shell` and `s9`.
- `int_and_term_send_ctrl_c_and_never_call_sbx`: `signal(Int)` and
  `signal(Term)` are `Ok`. The runner recorded **no** calls, and the cast file
  contains two `"i"` events with the payload `"\u0003"`. Read the cast with
  `std::fs::read_to_string` and count `"\u0003"`.
- `status_reports_idle_running_and_paused_from_find`: canned outputs
  `bash=1 fg=1 state=S` → `Idle`; `bash=1 fg=2 state=S` → `Running`;
  `bash=1 fg=2 state=T` → `Paused`. Swap the canned value between calls,
  for example with an `Arc<Mutex<VmRun>>`.
- `status_reports_exited_when_the_vm_shell_is_gone`: canned
  `(Some(3), "no-shell", "")` → `Exited`.
- `status_keeps_the_host_view_when_find_fails`: canned
  `(Some(1), "", "error: sandbox 'de-ghost-t' not found")` → the state equals
  what `inner.status()` reports for an idle bash, `Idle`. It is **not**
  `Exited`.
- `is_exited_never_calls_sbx`: `is_exited()` on a live inner bash is `false`,
  and the runner recorded no calls.
- `close_hangs_up_the_vm_shell_then_the_client`: canned `(Some(0), "hup bash=1", "")`.
  `close()` is `Ok`; the recorded call is the `VmOp::Hup` argv; afterwards
  `is_exited()` becomes `true` within 10 s (poll every 50 ms).
- `close_reports_a_failed_hangup`: canned `(Some(1), "", "boom")` →
  `close()` is `Err` containing `boom`.

**Reviewer checks:** (a) a live probe on pinky, a real `daemoneye shell-host`
with a tagged sbx child: `Ctrl-p` freezes a tick loop, resume restarts it,
`close` ends it; (b) a mutation that makes `Int` go through `vm(VmOp::Stop)`
must fail `int_and_term_send_ctrl_c_and_never_call_sbx`.

## End-to-end verification

```sh
cargo test --lib shell::sbx_backend 2>&1 | grep -E '^test result'
cargo test --lib shell::host 2>&1 | grep -E '^test result'
cargo test --lib shell::backend 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.

## Authorizations

- [x] May add the provided `is_exited` method to `ShellBackend` and change the
      one condition in `serve_until_exit` (`src/shell/host.rs`).
- [x] May edit `run_host` in `src/shell/launch.rs` as Task 3 shows.
- [x] May add the new module and its exports in `src/shell/mod.rs`.

## Out of scope

- Any daemon-side change (`src/daemon/server/shells.rs`, runs, tools). They
  reach these operations through the host frames already.
- Pause-vs-deadline (Q6): row 05c.
- Creating or removing sandboxes, and the orphan-sidecar fix: row 06.
- Changing `PtyBackend` or `PtyShell`.

## Update Log

<!-- entries appended below this line -->
