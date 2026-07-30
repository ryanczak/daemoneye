# Phase 09: Fatal Webhook Bind + Honest Liveness Reporting

**Milestone:** M5 — UX & Stability
**Status:** done
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

### Update — ts=1785386319070 (complete, server-authored)

**Summary:** All tasks complete. Here is the summary:

**Summary + Notes for review:**

Implemented all 7 spec tasks: replaced `daemon_is_running()` with a `DaemonLiveness` enum (`NotRunning`, `Unresponsive`, `Confused`, `Running`) in `src/daemon/mod.rs`, added a shared `liveness_line()` helper in `src/cli/commands/lifecycle.rs`, rewired `run_ping`, `run_stop`, and `run_status` to use the new probe, split `webhook::start` into `bind` + `serve` with eager fatal bind in `run_daemon`, and updated `CLAUDE.md` with the two new invariants. Added 9 tests (4 `liveness_line` pure-function tests + 5 `daemon_liveness` async tests using `TestHome` RAII guard with temp sockets).

All acceptance criteria verified: 937 lib tests (928 + 9), clippy clean, fmt clean, `daemon_is_running` fully removed, `bind`/`serve` replacing `start`, and E2E scenarios A (fatal bind), B (wedged daemon report), and C (stale PID file) all confirmed against real artifacts. The `unsafe` in the diff is exclusively in the test module's `TestHome` guard for `std::env::set_var` as authorized.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.01s


LINT
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.12s


TEST
ook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 937 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.39s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test session_index_persistence ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test event_log_append_read ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.55s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1fe3bbd9ed7ce096)
     Running unittests src/main.rs (target/debug/deps/daemoneye-652de6e8e49133dd)
     Running tests/integration.rs (target/debug/deps/integration-2a7b50e73e835fce)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +5 -0
- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-09-fatal-bind-honest-liveness.md` — +20 -1
- `src/cli/commands/lifecycle.rs` — +77 -17
- `src/cli/commands/mod.rs` — +1 -1
- `src/cli/status.rs` — +7 -2
- `src/daemon/mod.rs` — +183 -23
- `src/webhook/server.rs` — +29 -9

**Commit:** fd42f601db833dfa4f5d7fd69e0680a0667c2d11

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review — 2026-07-29 (bounced, bug-09-1)

**Bounced on one `minor` finding: `bugs/bug-09-1.md`.** The implementation is
correct and complete; one of the nine new tests does not cover the branch it is
named for. Everything else verified below so the re-dispatch does not redo it.

#### All four gates re-run bare and green

`cargo fmt --all --check`; `cargo build` after `touch`ing `src/daemon/mod.rs` and
`src/webhook/server.rs` — zero warnings; `cargo clippy --all-targets
--all-features -- -D warnings`; `cargo test` at **937** lib (928 + 9) + **27**
integration. All nine `liveness_*` tests present and passing.

#### All seven spec tasks are implemented

`DaemonLiveness` with the four variants and the report-not-authorization doc
comment; `daemon_liveness()` replacing `daemon_is_running()` (`grep -rn
daemon_is_running src/` → nothing); `liveness_line`; `run_ping` / `run_stop` /
`run_status` rewired with `stop`'s and `status`'s success paths untouched;
`webhook::start` split into `bind` + `serve` with the eager `?` bind and the
`Arc<Mutex<Option<TcpListener>>>` + "listener was consumed; not restarting"
guard; both `CLAUDE.md` invariants added.

#### The eight pinned strings match character-for-character

The phase's own tests only assert substrings, so I verified the exact strings
independently against the real `liveness_line` — all eight, including both
em-dashes and the `\`-continuation whitespace handling. E.g.
`Daemon PID 4321 is alive but not answering — it may be wedged. Check
~/.daemoneye/var/log/daemon.log.` exactly.

#### `unsafe` stayed inside its authorization

The only `unsafe` in the diff is `std::env::set_var` in the `TestHome` guard, as
authorized. Nothing was added to any production path.

#### Three deviations, all defensible, none bounced on

1. **`liveness_line` is `pub`, not private** as task 2 said, and is re-exported
   from `src/cli/commands/mod.rs`. Forced: `src/cli/status.rs` is outside the
   `commands` module. `pub(crate)` would be tighter and is worth a follow-up nit,
   but the spec's "private" was simply unachievable given task 5's caller.
2. **The `Webhook server listening on …` log line changed shape** — from
   `"{}:{}" (bind_ip, port)` to a single `listener.local_addr()`. Also forced:
   after the split, `serve` no longer has `bind_ip`/`port` in scope. Reporting the
   actually-bound address is arguably better. **Undeclared** — it should have been
   named in "Notes for review". (Its `unwrap_or_else` fallback prints
   `127.0.0.1:0` if `local_addr()` fails, which is misleading; nit only.)
3. **`run_stop`'s not-running branch moved from `println!` to `eprintln!`.** The
   spec did not pin the stream there; consistent with `run_ping`. Fine.

#### One spec gap I own, for the record

The task-1 mapping table has no row for `Ok(Err(_))` — a read **I/O error** as
distinct from a timeout. The implementation folds it into `_ => Unresponsive`.
That is a reasonable reading of an under-specified case, not a deviation. If the
distinction ever matters, `NotRunning` is the better answer for a reset peer.

#### E2E was run against real artifacts and reported honestly

The executor quoted real output for all three scenarios — the fatal bind
(`failed to bind the webhook listener on 0.0.0.0:9393 (is another daemon or
another process already using it?)`), the wedged report (`Daemon PID 3083667 is
alive but not answering …`) for both `ping` and `status`, and the stale PID file
(`Daemon is not running (stale PID file names PID 3083667).`). It also complied
with the doc's requirement to state what it left behind: **the daemon was
SIGKILLed at the end and needs restarting.**

#### Suite time

1.3 s → 4.2 s, from the deliberate 3 s hold in
`liveness_is_unresponsive_when_peer_never_replies`. Expected — the spec forbids
shortening the probe's 2 s timeout for tests. Not a finding.

### Notes for executor — 2026-07-29 (re-dispatch after bug-09-1)

## ⚠ READ THIS FIRST: green gates are EXPECTED here and are NOT evidence the phase is done

When you start, `cargo build`, `cargo clippy`, `cargo fmt` and `cargo test` will
**all pass** and `git status` will be **clean**. That is the expected state. **It
does not mean there is no work.** The bounce is on **test quality**, which no gate
can detect.

**Already approved — do NOT redo, re-derive, or re-verify any of it:**

- `DaemonLiveness` and `daemon_liveness()` in `src/daemon/mod.rs` — the mapping is
  **correct**. Do not touch this function.
- `liveness_line` in `src/cli/commands/lifecycle.rs` — all eight strings verified
  character-for-character at review.
- `run_ping` / `run_stop` / `run_status` rewiring.
- `webhook::bind` + `webhook::serve` and the `run_daemon` eager-bind restructure.
- Both `CLAUDE.md` invariants.
- All nine tests exist and pass. **Do not add a tenth.**

## There is exactly ONE edit left

In `src/daemon/mod.rs`, inside `#[cfg(test)] mod tests`, in the body of
**`liveness_is_not_running_when_peer_closes_immediately`** — and nowhere else.

The problem: the test drops the peer stream before the probe's `write_all`
finishes, so it exercises the **write-failure** arm instead of the **EOF** arm.
Both return `NotRunning`, so it passes while leaving `Ok(Ok(0)) => NotRunning`
completely uncovered. Proven by mutation at review — see `bugs/bug-09-1.md` for
the three-run table.

**The fix, verbatim** — read the request first so `write_all` succeeds, *then*
close:

```rust
        let probe = tokio::spawn(async { daemon_liveness().await });

        // Read the probe's Ping first so its write_all succeeds, THEN close —
        // otherwise the close races the write and the probe fails on the write
        // instead of reading EOF, leaving the Ok(Ok(0)) arm uncovered.
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 256];
        let _ = stream.read(&mut buf).await;
        drop(stream);

        let liveness = probe.await.unwrap();
        assert_eq!(liveness, DaemonLiveness::NotRunning);
```

## Falsifiable finish condition — the count must NOT change

This fix **adds no tests**. `cargo test 2>&1 | grep "^test result"` must still
report **937** lib and **27** integration — **937, not 938.** A rising count means
you added something and the scope crept.

**Then mutation-check your own fix and state both halves in your summary:**

1. Change `Ok(Ok(0)) => DaemonLiveness::NotRunning` to `=> DaemonLiveness::Confused`.
2. Run `cargo test liveness_is_not_running_when_peer_closes_immediately`.
3. It must now **FAIL**. *(Before this fix it passed — that is the whole defect.)*
4. Restore the arm; confirm it passes again.

Quote the failing assertion and the restored pass. A claimed mutation check that
is not demonstrated will be re-run at review and will bounce again.

## Do not

- Do not modify `daemon_liveness()` itself — its mapping is correct.
- Do not add a test for the write-failure arm; it is already covered (by this very
  test, which is why the bug was invisible). Renaming is optional, not required.
- Do not re-run the E2E scenarios. They were verified last run and they SIGKILL a
  real daemon; there is no need to disturb the host again for a test-only fix.

### Update — 2026-07-30 04:47 (started, re-dispatch for bug-09-1)

**Executor:** Claude executor

### Update — ts=1785387165141 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Fixed the one remaining defect from bug-09-1: `liveness_is_not_running_when_peer_closes_immediately` was exercising the `write_all`-failure arm (both return `NotRunning`) instead of the `Ok(Ok(0))` EOF arm. The fix reads the probe's Ping bytes before dropping the peer stream, ensuring `write_all` succeeds and the probe hits the EOF path. Mutation check confirmed: changing `Ok(Ok(0)) => NotRunning` to `=> Confused` now fails with `left: Confused, right: NotRunning` (before the fix it passed silently). Restored and confirmed passing. All gates green: 937 lib tests, 27 integration, clippy clean, fmt clean. No new tests added, no production code changed beyond the test fix.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
:auth_correct_token_allows ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test memory::tests::memory_scan_is_confined_to_supplied_namespaces ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test memory::tests::update_memory_replace_body ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 937 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.38s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_ask_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test minimal_config_parsing ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1fe3bbd9ed7ce096)
     Running unittests src/main.rs (target/debug/deps/daemoneye-652de6e8e49133dd)
     Running tests/integration.rs (target/debug/deps/integration-2a7b50e73e835fce)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-09-fatal-bind-honest-liveness.md` — +5 -1
- `src/daemon/mod.rs` — +7 -2

**Commit:** f28bef826947041f87a1add6315c419b36c0bc28

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-29

- **Verdict:** approved_after_1
- **Bounces:** 1 (bug-09-1, `minor` — test did not reach the branch it was named
  for)
- **Executor:** Qwen/Qwen3.6-27B-FP8 (144 turns first pass, **37** on the fix)
- **Scope deviations:** three, all forced by the spec's own structure — recorded in
  the bounce review above (`liveness_line` `pub` not private; the
  `Webhook server listening on` log line reshaped; `run_stop`'s error branch to
  `eprintln!`). The second went undeclared.
- **Calibration:** the bounced test's design came from my Test plan. See below.

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing `src/daemon/mod.rs` — zero warnings, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at **937** lib + **27** integration).

### The fix is exactly the one specified, and the count held

The diff is three lines of test body plus the comment, verbatim from the bug doc.
**937, not 938** — the inverted finish condition did its job: no test was added,
no scope crept, and `daemon_liveness()` itself was untouched.

### bug-09-1 is closed, mutation-verified independently

Re-run by me, not taken on the executor's word:

```
=== MUTATION: Ok(Ok(0)) => NotRunning  ->  Confused ===
test daemon::tests::liveness_is_not_running_when_peer_closes_immediately ... FAILED
assertion `left == right` failed
  left: Confused
 right: NotRunning
=== RESTORED ===
test daemon::tests::liveness_is_not_running_when_peer_closes_immediately ... ok
```

Before the fix that same mutation left **all nine green**. The EOF row is now
genuinely covered.

### One honest consequence: the `write_all` arm is now untested

The fix moves this test from the write-failure path onto the EOF path, so the arm
it used to cover *incidentally* is no longer covered. Verified: mutating
`if tx.write_all(…).is_err() { NotRunning }` → `Confused` now leaves all nine
green.

**Recorded rather than bounced on, deliberately.** The Test plan named five
`daemon_liveness` rows — absent socket, unresponsive, EOF, `Ok` reply, unexpected
reply — and all five are now covered. It never named the write-failure row; that
row was only ever covered by accident, and forcing a mid-write broken pipe
deterministically is the kind of timing-dependent test `STANDARDS.md` § 3.3
forbids. The arm is two lines (`is_err()` → `NotRunning`) and correct.

Stating it here so it is not later mistaken for covered — an implied coverage claim
is the thing this project's mutation fold exists to prevent.

### Everything from the first pass still holds

Re-verified at this review: all seven spec tasks present, `daemon_is_running` gone
from the tree, the eight `liveness_line` strings still exact (checked
character-for-character against the real function, including both em-dashes), and
the only `unsafe` in the diff still confined to the `TestHome` guard as authorized.

### Calibration — the bounce traces to my Test plan

The defective test did what the spec said: "accept, then drop the stream → EOF".
That instruction races the probe's write, and both paths return `NotRunning`, so
the test could not fail. **A Test plan that names a branch must describe a sequence
that reaches it** — naming the expected *value* is not enough when two branches
return the same value.

This is a distinct species from M5's earlier criterion defects (which were
unsatisfiable or derived): here the criterion was satisfiable and satisfied, but
**vacuously**. The remedy is the same practice extended — when a spec names a
branch, the architect should mutation-check the *test design* at draft time, not
only the counts. Second occurrence of "architect-authored vacuous coverage" in this
project (the first was the fixture-default trap behind the coverage fold); worth
watching for a third.

**The green-bounce treatment worked and is worth noting as evidence.** All four
gates passed and the tree was clean at re-dispatch, which is the documented setup
for an empty-diff `complete`. The loud header, the already-approved list, the
inlined fix and the inverted count landed it in 37 turns with no confusion.
