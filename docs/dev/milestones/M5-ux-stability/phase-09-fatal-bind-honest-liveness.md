# Phase 09: Fatal Webhook Bind + Honest Liveness Reporting

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-08 (instance lock) — the PID file this phase reads is
created there
**Estimated diff:** ~210 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Stop swallowing the second duplicate-instance signal (a webhook port that is
already bound), and make `ping` / `status` able to say *"the daemon is alive but
not answering"* instead of collapsing that into "not running". The wedged-vs-dead
distinction is the diagnostic that was missing during the 2026-07-25 incident.

## Architecture references

Read before starting:

- `docs/design/daemon-instance.md` § 1.1 — the four situations that
  `daemon_is_running()` currently collapses into one `false`. This phase splits
  them.
- `docs/design/daemon-instance.md` § 4.1 — why wedged-vs-dead matters, and the
  hard rule that liveness reporting must never again gate a destructive action.
- `docs/design/daemon-instance.md` § 4.2 — the swallowed `EADDRINUSE`.
- `docs/design/daemon-stalls.md` § 1.5b–1.5c — the confirmed stall this
  reporting is meant to make visible: every thread `futex`-parked, socket still
  listening, nothing answered.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phase 08 landed: `src/daemon/instance.rs` exists and exports
   `read_pid`, and `crate::config::default_pid_path()` resolves.
6. Verify the starting state:

```bash
grep -c "daemon_is_running" src/daemon/mod.rs            # expect 1 (definition only; 08 deleted its call)
grep -c "pub fn read_pid" src/daemon/instance.rs         # expect 1
grep -c "pub fn default_pid_path" src/config/load.rs     # expect 1
grep -c "pub async fn start" src/webhook/server.rs       # expect 1  (becomes 0: bind + serve)
grep -c "pub use server::\*" src/webhook/mod.rs          # expect 1  (a glob — see task 6)
grep -n "tempfile" Cargo.toml                            # expect line 44, tempfile = "3"
cargo test 2>&1 | grep "^test result" | head -3   # expect 928 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
on 2026-07-29, immediately before dispatch.** If one differs, **stop and report a
blocker**.

> **Use `cargo test`, not `cargo test --lib`.** The full command prints **three**
> `test result` lines; `--lib` prints only the first.

## Current state

### `daemon_is_running()` — `src/daemon/mod.rs:293`

> **⚠ Line numbers in this section were refreshed 2026-07-29 before dispatch.** The
> phase was drafted 2026-07-26; phases 06h and 08 have since edited
> `src/daemon/mod.rs`. Every code quote below is byte-identical to the tree as of
> the refresh — only the numbers moved. Re-derive with the grep beside each.

```rust
/// Returns true if a daemon is already listening and responding on the socket.
/// Uses a 2-second timeout so a hung process doesn't block startup.
pub async fn daemon_is_running() -> bool {
    let Ok(stream) = tokio::net::UnixStream::connect(default_socket_path()).await else {
        return false;
    };
    let (rx_half, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx_half);

    let Ok(mut data) = serde_json::to_vec(&Request::Ping) else {
        return false;
    };
    data.push(b'\n');
    if tx.write_all(&data).await.is_err() {
        return false;
    }

    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(2), rx.read_line(&mut line)).await {
        Ok(Ok(_)) => matches!(
            serde_json::from_str::<Response>(line.trim()),
            Ok(Response::Ok)
        ),
        _ => false,
    }
}
```

After phase 08 this function has **zero call sites** (it stays live because
`src/lib.rs:10` re-exports `pub mod daemon`). This phase gives it real callers.

### `run_ping` — `src/cli/commands/lifecycle.rs:46`

```rust
pub async fn run_ping() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Ping).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon is running."),
                _ => {
                    println!("Daemon is not responding.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
```

`run_stop` (`lifecycle.rs:24`) and `run_status` (`src/cli/status.rs:154`, whose
`Daemon is not running.` line is `:157`) have
the same `Err(_) => "Daemon is not running."` shape. Note that `connect()`
(`src/cli/commands/ipc_client.rs:30`) already carries a 5-second timeout, so its
`Err` conflates "no socket" with "connect timed out".

### The webhook bind — `src/webhook/server.rs:83`

```rust
pub async fn start(
    config: Config,
    sessions: SessionStore,
    cache: Arc<crate::tmux::cache::SessionCache>,
    schedule_store: Arc<crate::scheduler::ScheduleStore>,
) -> anyhow::Result<()> {
    let port = config.webhook.port;
    let bind_ip: std::net::IpAddr = config
        .webhook
        .bind_addr
        .parse()
        .unwrap_or_else(|_| std::net::Ipv4Addr::LOCALHOST.into());
    let state = Arc::new(WebhookState { … });

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::new(bind_ip, port)).await?;
    log::info!("Webhook server listening on {}:{}", bind_ip, port);
    axum::serve(listener, app).await?;
    Ok(())
}
```

Its caller in `run_daemon` (`src/daemon/mod.rs:707`, `if startup_config.webhook.enabled {`)
wraps the whole thing in `supervise(...)`, whose contract is to restart the factory
forever with backoff (`mod.rs:82`). So `EADDRINUSE` is logged once per attempt and retried
indefinitely.

## Spec

### 1. `DaemonLiveness` enum in `src/daemon/mod.rs`

Replace `daemon_is_running()`'s `bool` return with a four-case enum declared
directly above it:

```rust
/// What a liveness probe against the daemon socket found.
///
/// This is a *report*, never an authorization. Nothing may unlink a socket,
/// remove a file, or otherwise act destructively on the strength of a variant
/// here — instance ownership is decided solely by the `InstanceLock`
/// (`docs/design/daemon-instance.md` § 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonLiveness {
    /// No socket file, or nothing listening on it.
    NotRunning,
    /// Connected, but the daemon did not answer `Ping` within the timeout.
    /// A live process that is wedged looks like this.
    Unresponsive,
    /// Connected and answered `Ping` with something other than `Response::Ok`.
    Confused,
    /// Connected and answered `Ping` with `Response::Ok`.
    Running,
}
```

Rewrite `daemon_is_running()` as:

```rust
pub async fn daemon_liveness() -> DaemonLiveness
```

mapping as follows — the mapping *is* the spec, so follow it exactly:

| Probe outcome | Variant |
|---|---|
| `UnixStream::connect` returns `Err` (any error, including `ENOENT` and `ECONNREFUSED`) | `NotRunning` |
| `serde_json::to_vec(&Request::Ping)` fails | `Confused` |
| `tx.write_all` fails | `NotRunning` |
| `timeout(2s, read_line)` elapses | `Unresponsive` |
| `read_line` returns `Ok(0)` (EOF — peer closed) | `NotRunning` |
| reply parses to `Response::Ok` | `Running` |
| reply is unparsable, or parses to any other `Response` | `Confused` |

Note the two behavior changes hidden in that table. The timeout branch now
yields `Unresponsive` instead of `false`, and a `write_all` failure is
`NotRunning` rather than being lumped with the timeout — a peer that has gone
away breaks the pipe, it does not hang.

Delete `daemon_is_running()`. It has no callers after phase 08, so there is
nothing to update; do **not** keep a `bool`-returning wrapper for
compatibility (STANDARDS § 2.2 forbids back-compat shims).

Add `is_running()` to the enum returning `matches!(self, Self::Running)` **only
if** a task below actually needs it. It does not — do not add it.

### 2. A shared reporting helper for the three CLI commands

In `src/cli/commands/lifecycle.rs`, add a private helper the three commands use
so the wording exists in exactly one place:

```rust
/// One line describing what a probe found, for `ping` / `stop` / `status`.
/// `pid` is the PID-file payload, used only to distinguish a wedged daemon from
/// an absent one.
fn liveness_line(liveness: DaemonLiveness, pid: Option<u32>) -> String
```

Exact strings — these are pinned, match them character for character:

| `liveness` | `pid` | Returned line |
|---|---|---|
| `NotRunning` | `Some(p)` | `Daemon is not running (stale PID file names PID {p}).` |
| `NotRunning` | `None` | `Daemon is not running.` |
| `Unresponsive` | `Some(p)` | `Daemon PID {p} is alive but not answering — it may be wedged. Check ~/.daemoneye/var/log/daemon.log.` |
| `Unresponsive` | `None` | `Daemon is listening but not answering — it may be wedged. Check ~/.daemoneye/var/log/daemon.log.` |
| `Confused` | `Some(p)` | `Daemon PID {p} answered with an unexpected reply.` |
| `Confused` | `None` | `Daemon answered with an unexpected reply.` |
| `Running` | any | `Daemon is running.` |

The `NotRunning` + `Some(p)` case is worth its own row rather than falling back
to the bare string: a PID file naming a dead process is exactly the state after
an unclean kill, and saying so distinguishes "never started" from "died".

### 3. Rewire `run_ping`

Replace the body of `run_ping` (`lifecycle.rs:46`) with a probe through
`daemon_liveness()` plus `instance::read_pid(&default_pid_path())`:

- Print `liveness_line(...)` to **stdout** on `Running`, to **stderr**
  otherwise.
- Exit `0` on `Running`, exit `1` on every other variant.

The existing "connect then hand-roll a Ping" code goes away — `daemon_liveness()`
does the probe now.

### 4. Rewire `run_stop`'s not-running branch

In `run_stop` (`lifecycle.rs:24`), replace the `Err(_) => { println!("Daemon is
not running."); exit(1) }` arm with a `daemon_liveness()` probe so a wedged
daemon is described as wedged. **Keep the success path exactly as it is**: on a
successful `connect()` it must still send `Request::Shutdown` and print
`Daemon stopped.`

Do not probe before connecting on the happy path — that would double the round
trips on every `stop`. Probe only in the error arm.

### 5. Rewire `run_status`'s not-running branch

In `run_status` (`src/cli/status.rs:156-159`), replace the
`Err(_) => { eprintln!(c_err("Daemon is not running.")); exit(1) }` arm the same
way: probe, then `eprintln!("{}", c_err(&liveness_line(…)))`, then `exit(1)`.
Keep the `c_err` coloring. The large `Ok(Response::DaemonStatus { … })` match arm
is untouched.

### 6. Make the webhook bind fatal at startup

Split `webhook::start` into bind and serve so the bind error surfaces before the
supervisor exists.

In `src/webhook/server.rs`, change the signature to accept an already-bound
listener and add a `bind` function:

```rust
/// Bind the webhook listener. Fatal at startup: a port already in use is the
/// strongest available signal that another daemon is running
/// (`docs/design/daemon-instance.md` § 4.2).
pub async fn bind(config: &Config) -> anyhow::Result<tokio::net::TcpListener>

/// Serve on an already-bound listener. Runs until the process exits.
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: Config,
    sessions: SessionStore,
    cache: Arc<crate::tmux::cache::SessionCache>,
    schedule_store: Arc<crate::scheduler::ScheduleStore>,
) -> anyhow::Result<()>
```

`bind` does the `bind_addr` parse and the `TcpListener::bind`, and wraps the
error with context naming the address and the likely cause:

```rust
    .with_context(|| {
        format!(
            "failed to bind the webhook listener on {bind_ip}:{port} \
             (is another daemon or another process already using it?)"
        )
    })
```

`serve` keeps the `WebhookState` construction, the `Router`, the
`log::info!("Webhook server listening on {}:{}", …)` line, and `axum::serve`.
Keep the `unwrap_or_else(|_| Ipv4Addr::LOCALHOST.into())` fallback for an
unparsable `bind_addr` — that is pre-existing behavior and not this phase's
business.

In `run_daemon` (`src/daemon/mod.rs:707`), restructure the
`if startup_config.webhook.enabled { … }` block to bind **eagerly** with `?` and
pass the listener into the supervised closure:

```rust
    if startup_config.webhook.enabled {
        let listener = crate::webhook::bind(&startup_config).await?;
        // … existing log::warn!/log::info! about auth, unchanged …
        let listener = Arc::new(tokio::sync::Mutex::new(Some(listener)));
        // … supervise("webhook", …) closure takes the listener out of the Option
        //     on its first run and serves it.
    }
```

The `Option` inside the mutex is the point: `axum::serve` consumes the listener,
so only the supervisor's **first** attempt can serve it. On a restart the
`Option` is `None`, and the closure must then log
`log::error!("webhook listener was consumed; not restarting")` and return
`Ok(())` rather than re-binding. A supervisor that silently re-binds would
re-introduce the retry loop this task exists to delete.

Keep the two `log::warn!` / `log::info!` messages about Bearer-token auth
verbatim, including their current wording.

**`src/webhook/mod.rs:19` is `pub use server::*;` — a glob. Verified: no edit is
needed there.** Do not go looking for a named `start` re-export; there isn't one.

### 7. Update `CLAUDE.md`

In the `## Important Invariants` list, add:

```markdown
- Liveness probes (`daemon_liveness()`) are reports, never authorizations. No
  code may unlink a socket or remove a file based on a `DaemonLiveness` variant
  — instance ownership is decided only by the `InstanceLock` flock.
- The webhook listener binds eagerly in `run_daemon` and a bind failure is fatal.
  It is a duplicate-instance signal, not a transient condition to retry.
```

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test 2>&1 | grep "^test result"` shows the lib count at **937**
      (928 + 9 new) and integration at **27**. Equivalently, and this is the check
      that matters: the lib count is **exactly 9 higher** than the 928 you recorded
      in Pre-flight. **If it is anything else, stop and report a blocker naming the
      number you measured — do not re-run the command hoping for a different
      answer.**
- [ ] `grep -rn "daemon_is_running" src/` returns nothing.
- [ ] `grep -c "pub async fn start" src/webhook/server.rs` returns **0** (it is
      now `bind` + `serve`), and `grep -c "pub async fn bind\|pub async fn serve"
      src/webhook/server.rs` returns **2**.
- [ ] `git diff -U0 -- src/ | grep '^+' | grep unsafe` shows **only**
      `unsafe { std::env::set_var("HOME", …) }` inside a test module — no `unsafe`
      is added to any production path.
      **⚠ Phrased on the diff, not the file, deliberately.** A per-file count
      cannot be 0: `src/daemon/mod.rs:355` already contains the pre-existing
      `unsafe { libc::dup2(…) }` in the log-redirect path, untouched by this phase.
      An earlier version of this criterion demanded 0 per file and was
      unsatisfiable; caught by running it before dispatch.
- [ ] `daemoneye ping` against a wedged daemon reports it as wedged
      (End-to-end verification).

### ⚠ How to check the test count — read this before checking it

The only commands you need, once each:

```bash
cargo test 2>&1 | grep "^test result"     # three lines; lib is the first
cargo test 2>&1 | grep liveness_          # the new tests, each "... ok"
```

**Do not count tests by grepping the per-test `^test ` lines** (`| wc -l`,
`--list | grep -c`, and friends) — those totals do not agree with the summary,
because they include or exclude the bin and integration targets depending on
flags. The summary line is authoritative. **A number that disagrees with this doc
means the doc is wrong; say so and report a blocker.** Re-running a read-only
command that already answered makes no progress and will trip the governor.

## Test plan

`liveness_line` is a pure function of two arguments — test it directly.
`daemon_liveness()` needs a socket; use a `tempfile::TempDir` plus a real
`tokio::net::UnixListener` that you control, so the tests stay hermetic and need
no daemon.

`daemon_liveness()` reads `default_socket_path()`, which depends on `HOME`.

### ⚠ How to redirect `HOME` in a test — corrected 2026-07-29

The earlier draft of this section said to hold `crate::TEST_HOME_LOCK` "exported
from `src/main.rs`". **Both halves were wrong.** The lock lives at
**`src/lib.rs:32`**, and you should not take it directly — take the
poison-recovering accessor **`crate::test_home_guard()`** (`src/lib.rs:45`),
added precisely so that one panicking test does not poison the lock and fail
every other HOME-dependent test in the binary (48 instead of 1, measured).

**Use this RAII shape — it is the codebase idiom, quoted from
`src/daemon/context/recall.rs:246`:**

```rust
    /// RAII test-home guard: holds `TEST_HOME_LOCK`, points `HOME` at a fresh
    /// tempdir, and restores the original `HOME` on drop.
    struct TestHome {
        _tmp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Option<String>,
    }

    impl TestHome {
        fn new() -> Self {
            let lock = crate::test_home_guard();
            let saved = std::env::var("HOME").ok();
            let tmp = tempfile::tempdir().unwrap();
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            Self { _tmp: tmp, _lock: lock, saved }
        }
    }
```

(Its `Drop` restores `saved`, or removes `HOME` when it was absent.)

**This crate is `edition = "2024"`, so `std::env::set_var` is `unsafe` and the
`unsafe { … }` block above is mandatory** — there is no safe alternative. See the
Authorizations: `unsafe` is authorized for exactly this and nothing else.

For the `Confused` test, `Response::Error(String)` (`src/ipc.rs:355`) is a
concrete non-`Ok` variant — use it rather than hunting for one.

In `src/cli/commands/lifecycle.rs`:

- `liveness_line_reports_wedged_with_pid` — `Unresponsive` + `Some(4321)`
  contains `PID 4321` and `wedged`.
- `liveness_line_reports_wedged_without_pid` — `Unresponsive` + `None` contains
  `wedged` and does **not** contain `PID`.
- `liveness_line_distinguishes_stale_pid_file` — `NotRunning` + `Some(4321)`
  mentions `stale PID file`; `NotRunning` + `None` is exactly
  `Daemon is not running.`
- `liveness_line_running_ignores_pid` — `Running` + `Some(1)` and `Running` +
  `None` both return exactly `Daemon is running.`

In `src/daemon/mod.rs`:

- `liveness_is_not_running_when_socket_absent` — `HOME` pointed at an empty temp
  dir → `NotRunning`.
- `liveness_is_unresponsive_when_peer_never_replies` — bind a `UnixListener` at
  the socket path and accept the connection **without writing anything**;
  `daemon_liveness()` returns `Unresponsive`. Keep the probe's 2 s timeout — do
  not shorten it for the test; assert the variant, not the elapsed time (a
  timing assertion would be non-deterministic, which STANDARDS § 3.3 forbids).
- `liveness_is_not_running_when_peer_closes_immediately` — accept, then drop the
  stream → `NotRunning` (the EOF row of the task-1 table).
- `liveness_is_running_when_peer_answers_ok` — accept and write
  `serde_json::to_string(&Response::Ok)` + `\n` → `Running`.
- `liveness_is_confused_on_unexpected_reply` — accept and write a valid but
  wrong response (any `Response` variant that is not `Ok`) + `\n` → `Confused`.

No test is required for `webhook::bind` / `serve`: `bind` is a thin wrapper over
`TcpListener::bind` (STANDARDS § 3.2, pure plumbing) and binding a real port in a
unit test is not hermetic. Its behavior is covered by the E2E below.

## End-to-end verification

Two real-artifact behaviors need checking: the fatal bind, and the wedged
report. Quote actual output in the Update Log.

### ⚠ Phase 08 changed the ordering these scenarios depend on

**Added 2026-07-29.** Phase 08 put the `InstanceLock` at `src/daemon/mod.rs:372`,
which is *before* the webhook bind at `:707`. So if any daemon already holds the
lock, scenario A fails with `another daemon is already running (PID …)` and
**never reaches the webhook bind** — it would look like a pass while testing
nothing.

**Before scenario A, confirm no daemon is running:**

```bash
./target/release/daemoneye stop 2>/dev/null || true
pgrep -af 'daemoneye daemon' | grep -v grep    # expect no output
```

Verified config for these scenarios: `port = 9393`, `bind_addr = "0.0.0.0"`,
`enabled = true`.

**These scenarios stop, `SIGSTOP`, and `SIGKILL` a real daemon and repoint global
tmux hooks.** That is unavoidable for a real-artifact check of this behavior, but
note it in the Update Log, and leave a working daemon running at the end (or say
explicitly that you did not).

```bash
cargo build --release

# A. Webhook bind is fatal. Occupy the configured webhook port, then start.
python3 -c "import socket;s=socket.socket();s.setsockopt(1,2,1);s.bind(('0.0.0.0',9393));s.listen(1);input()" &
OCCUPIER=$!
sleep 1
./target/release/daemoneye daemon --console
# expect non-zero exit and an error naming the address, e.g.:
#   failed to bind the webhook listener on 0.0.0.0:9393 (is another daemon
#   or another process already using it?)
kill $OCCUPIER

# B. Wedged-daemon report. Start a daemon, SIGSTOP it, then probe.
./target/release/daemoneye daemon
sleep 3
PID=$(cat ~/.daemoneye/var/run/daemoneye.pid)
kill -STOP "$PID"
./target/release/daemoneye ping
# expect exit 1 and:
#   Daemon PID <n> is alive but not answering — it may be wedged.
#   Check ~/.daemoneye/var/log/daemon.log.
./target/release/daemoneye status   # same line, in the error color
kill -CONT "$PID"
./target/release/daemoneye ping     # expect "Daemon is running." and exit 0

# C. Dead daemon with a leftover PID file.
kill -9 "$PID"
sleep 1
./target/release/daemoneye ping
# expect exit 1 and: Daemon is not running (stale PID file names PID <n>).
```

Scenario B is the acceptance test for `daemon-instance.md` § 4.1: `SIGSTOP`
reproduces the observable signature of the confirmed stall (socket listening,
nothing answering) without waiting for a real deadlock. Under the old code every
one of B and C printed the same `Daemon is not responding.` / `Daemon is not
running.`

## Authorizations

- [x] May edit `CLAUDE.md` § "Important Invariants" (task 7).
- [x] May change the public signatures of `webhook::start` → `bind` + `serve`
      (task 6) and `daemon_is_running` → `daemon_liveness` (task 1). Both are
      breaking API changes to lib-public items; that is intended.
- [ ] No new dependencies. `tempfile` is present at `Cargo.toml:44` (`tempfile =
      "3"`) — **verified 2026-07-29**, so use it; do not add it.
- [x] **May use `unsafe` for `std::env::set_var` in test modules only** (task:
      the `TestHome` guard). **⚠ Corrected 2026-07-29 — the earlier draft said
      "No `unsafe`", which contradicted this phase's own Test plan and would have
      made the HOME-redirecting tests impossible to write.** This crate is
      `edition = "2024"`, where `set_var` is `unsafe`; every existing HOME test in
      the tree wraps it. **Nothing else may use `unsafe`** — not the liveness
      probe, not the webhook bind, not any production path.

## Out of scope

- **Do not add a supervisor / auto-restart for a wedged daemon.**
  `daemon-instance.md` § 5 lists this as a non-goal. Detect and report only.
- **Do not act on a `DaemonLiveness` variant destructively** — no unlinking, no
  killing, no "clean up the stale socket for the user". Task 1's doc comment says
  why; violating it re-creates the phase-08 bug in a new location.
- **Do not touch `InstanceLock` or the acquisition site.** Phase 08 owns those.
  This phase only *reads* the PID file via `instance::read_pid`.
- **Do not add a PID to `log_event`.** Phase 10.
- **Do not change the fork or parent-side reporting.** Phase 11. In E2E scenario
  A the duplicate is run with `--console` for exactly that reason.
- **Do not touch the `Response::DaemonStatus` match arm in `run_status`**, its
  field list, or the `ipc.rs` protocol. Only the `Err(_)` arm changes.
- **Do not shorten the 2-second Ping timeout** to make tests faster.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 04:35 (started)

**Executor:** Claude executor

Implemented all 7 spec tasks: `DaemonLiveness` enum replacing `daemon_is_running()`, shared `liveness_line()` helper, rewired `run_ping`/`run_stop`/`run_status`, split `webhook::start` into `bind` + `serve` with eager fatal bind, updated `CLAUDE.md`. Added 9 new tests (4 `liveness_line` unit tests + 5 `daemon_liveness` async tests).

End-to-end verification:
- Scenario A (webhook bind fatal): `Error: failed to bind the webhook listener on 0.0.0.0:9393 (is another daemon or another process already using it?)` — confirmed.
- Scenario B (wedged daemon): `Daemon PID 3083667 is alive but not answering — it may be wedged. Check ~/.daemoneye/var/log/daemon.log.` — confirmed for both `ping` and `status`.
- Scenario C (stale PID file): `Daemon is not running (stale PID file names PID 3083667).` — confirmed.
- `cargo test`: 937 lib, 0 bin, 27 integration — matches expected 928 + 9 = 937.
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo fmt --all`: clean.
- `grep -rn "daemon_is_running" src/`: 0 matches.
- `grep -c "pub async fn start" src/webhook/server.rs`: 0; `bind` + `serve`: 1 each.
- `git diff -U0 -- src/ | grep '^+' | grep unsafe`: only in test module `TestHome`.

Note: A real daemon was running during E2E scenarios B and C; it was SIGKILL'd at the end. The daemon needs to be restarted for normal operation.

### Notes for executor — 2026-07-29 (pre-dispatch refresh)

This phase was drafted 2026-07-26. Phases 06h, 07 and 08 have since edited the
files it touches, so the architect re-derived every fact against the tree before
dispatch. Five things changed, and two of them would have blocked you:

1. **Line numbers refreshed.** `daemon_is_running` is at `mod.rs:293`, `supervise`
   at `:82`, the webhook block at `:707`, `run_ping` at `lifecycle.rs:46`,
   `run_status`'s error arm at `status.rs:156-159`. Every code quote is
   byte-identical to the tree; only the numbers moved.
2. **`unsafe` is now authorized for `std::env::set_var` in tests.** It previously
   said "No `unsafe`", which contradicted this phase's own HOME-redirecting
   tests — this crate is `edition = "2024"`, where `set_var` is `unsafe` and has
   no safe alternative. That contradiction would have been unresolvable.
3. **`TEST_HOME_LOCK` guidance corrected.** It is at `src/lib.rs:32`, not
   `src/main.rs`, and you should take `crate::test_home_guard()`
   (`src/lib.rs:45`) rather than the lock directly. A worked `TestHome` RAII
   example is quoted in the Test plan.
4. **`src/webhook/mod.rs:19` is `pub use server::*;`** — a glob, so task 6's
   conditional re-export edit is resolved: there is nothing to change there.
5. **E2E scenario A now needs no daemon running first**, because phase 08's
   instance lock is acquired before the webhook bind. See the note in
   End-to-end verification.

Test baseline is **928**; this phase adds 9, giving **937**. Count with
`cargo test 2>&1 | grep "^test result"` — once. If a number disagrees with this
doc, the doc is wrong: report a blocker naming what you measured rather than
re-running.
