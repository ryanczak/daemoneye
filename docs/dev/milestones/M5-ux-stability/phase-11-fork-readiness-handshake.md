# Phase 11: Fork Readiness Handshake — Make `daemoneye daemon` Tell the Truth

**Milestone:** M5 — UX & Stability
**Status:** todo
**Depends on:** phase-08 (instance lock — the failure this most needs to report),
phase-09 (fatal webhook bind — the second such failure)
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`daemoneye daemon` currently prints `daemoneye daemon started (PID n)` and exits
`0` before the child has proven it can start — so a duplicate launch reports
success while the child dies. Add a pipe-based readiness handshake so the parent
relays the child's real outcome and exits non-zero on failure.

## Architecture references

Read before starting:

- `docs/design/daemon-instance.md` § 4.4 — why this pre-existing dishonesty
  becomes load-bearing once a duplicate launch is an *expected* event rather
  than an accident.
- `CLAUDE.md` § "Important Invariants" — `main()` is synchronous so `libc::fork()`
  runs before the tokio runtime starts. **This phase does not move the fork** and
  must not introduce any async work before it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phases 08 and 09 landed: `InstanceLock::acquire` is called in
   `run_daemon`, and `webhook::bind` is called with `?` there.

## Current state

### The fork — `src/main.rs:254-294`

```rust
    // For `daemon` without `--console`, fork into the background before
    // starting the async runtime so the calling shell is released immediately.
    if let Commands::Daemon { console: false, .. } = &cli.command {
        // SAFETY: This runs before the tokio runtime starts, so only the main
        // thread exists. Forking a live multi-threaded runtime is unsound because
        // only the calling thread survives in the child.
        unsafe {
            let pid = libc::fork();
            if pid < 0 {
                anyhow::bail!("fork() failed: {}", std::io::Error::last_os_error());
            }
            if pid > 0 {
                // Parent: report the child PID and exit cleanly.
                println!("daemoneye daemon started (PID {})", pid);
                return Ok(());
            }
            // Child: create a new session so we are no longer attached to the
            // calling terminal, then redirect stdin from /dev/null.
            if libc::setsid() < 0 { … }
            let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
            …
        }
    }

    // Build the tokio runtime and run async work in the child (or directly
    // for --console / all other subcommands).
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
```

The parent's `println!` + `return Ok(())` is unconditional. Nothing the child
does can influence it.

### The child's failure paths, after phases 08 and 09

`run_daemon` can now `bail!` for reasons a user must see:

| Failure | Origin |
|---|---|
| another daemon holds the instance lock | phase 08, `InstanceLock::acquire` |
| webhook port already bound | phase 09, `webhook::bind` |
| no API key configured | pre-existing, `mod.rs:~407` |
| tmux session could not be created | pre-existing, `mod.rs:~460` |
| socket bind failed | pre-existing, `mod.rs:757` |

Every one of these currently reaches the user as `started (PID n)`, exit `0`,
and a line in a log file they have no reason to open.

### `async_main`'s daemon arm — `src/main.rs:304-318`

```rust
async fn async_main(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Daemon {
            log_file,
            console,
            session,
        } => {
            let log_file = if console {
                None
            …
            daemon::run_daemon(log_file, session).await?;
```

## Spec

### 1. New module `src/daemon/ready.rs`

Declare it in `src/daemon/mod.rs` (`pub mod ready;`).

#### The wire protocol

One line, written by the child to the pipe, read by the parent:

- `READY\n` — the child bound its socket and is entering the accept loop.
- `ERR <message>\n` — the child failed; `<message>` is the error's `to_string()`
  with any `\n` or `\r` replaced by a space (a multi-line error must not become
  two protocol lines).

If the child writes nothing and exits, the parent sees EOF. **That is the design
point that removes the need for a timeout:** the parent holds no copy of the
write end, so once the child dies the last write end closes and the read returns
`Ok(0)` immediately. Do not add a timeout, a `sleep`, or a `waitpid` — they are
all unnecessary and the sleep would violate STANDARDS § 3.3.

#### Public surface

```rust
/// What the parent learned from the forked child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildReport {
    /// Child bound its socket and is serving.
    Ready,
    /// Child reported a startup failure.
    Failed(String),
    /// Child exited without reporting. Its own log is the only record.
    Died,
}

/// Create the readiness pipe. Returns `(read_end, write_end)`.
pub fn create_pipe() -> std::io::Result<(OwnedFd, OwnedFd)>;

/// Install the write end as this process's reporter. Called in the child.
pub fn set_reporter(fd: OwnedFd);

/// Report success, then release the reporter. No-op when none is installed
/// (`--console`, or any non-forking entry point).
pub fn report_ready();

/// Report a startup failure, then release the reporter. No-op when none is
/// installed.
pub fn report_failure(msg: &str);

/// Block until the child reports or the pipe reaches EOF. Called in the parent.
pub fn await_child_report(read_end: OwnedFd) -> ChildReport;

/// Parse one protocol line. Pure — this is what the unit tests target.
pub fn parse_report_line(line: &str) -> ChildReport;
```

#### Implementation notes

`create_pipe` holds the **only** `unsafe` block in this phase:

```rust
pub fn create_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as std::os::fd::RawFd; 2];
    // SAFETY: `fds` is a two-element array of RawFd, exactly what pipe(2)
    // requires. On success the kernel has written two fresh descriptors we take
    // sole ownership of via OwnedFd.
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created by pipe(2) and are not owned
    // elsewhere.
    unsafe {
        Ok((
            OwnedFd::from_raw_fd(fds[0]),
            OwnedFd::from_raw_fd(fds[1]),
        ))
    }
}
```

Use `std::os::fd::{OwnedFd, FromRawFd, AsRawFd}`. `OwnedFd` closes on drop, so
there is no manual `libc::close` anywhere in this phase — do not add any.

The reporter is a module-level `static REPORTER: Mutex<Option<OwnedFd>>`. Lock it
with `.unwrap_or_log()` (the `UnpoisonExt` trait from `src/util.rs`) — that is a
project invariant, see `CLAUDE.md` § "Important Invariants". `report_ready` /
`report_failure` `take()` the fd out of the `Option` and let it drop after
writing, so the write end closes as soon as the outcome is reported and a second
call is a silent no-op.

Write with `std::io::Write::write_all` on a `std::fs::File` created via
`File::from(fd)`, and ignore the result — if the parent has already gone away
there is nobody to tell, and a failed status report must never take the daemon
down.

`await_child_report` wraps the read end in a `BufReader` over
`std::fs::File::from(read_end)` and calls `read_line` **once**:

- `Ok(0)` → `ChildReport::Died`
- `Ok(_)` → `parse_report_line(&line)`
- `Err(_)` → `ChildReport::Died`

`read_line`, not `read_to_string`: on success the child keeps running forever and
never closes its write end, so reading to EOF would block for the daemon's whole
lifetime.

`parse_report_line` — trim trailing `\r\n`, then:

- exactly `READY` → `Ready`
- starts with `ERR ` → `Failed(rest.to_string())` (the remainder after the single
  space, not trimmed further — the message may legitimately have inner spacing)
- exactly `ERR` with no message, or anything else including an empty line →
  `Died`

### 2. Create the pipe and split it across the fork

In `src/main.rs`, restructure the `if let Commands::Daemon { console: false, .. }`
block. The pipe is created **before** `libc::fork()` so both processes inherit
it:

```rust
    if let Commands::Daemon { console: false, .. } = &cli.command {
        let (read_end, write_end) = daemon::ready::create_pipe()
            .context("failed to create the daemon readiness pipe")?;

        // SAFETY: (existing comment, unchanged)
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            anyhow::bail!("fork() failed: {}", std::io::Error::last_os_error());
        }
        if pid > 0 {
            // Parent: drop our copy of the write end so the child's is the only
            // one left — otherwise the read below never sees EOF.
            drop(write_end);
            return match daemon::ready::await_child_report(read_end) {
                daemon::ready::ChildReport::Ready => {
                    println!("daemoneye daemon started (PID {})", pid);
                    Ok(())
                }
                daemon::ready::ChildReport::Failed(msg) => {
                    eprintln!("daemoneye: daemon failed to start: {msg}");
                    std::process::exit(1);
                }
                daemon::ready::ChildReport::Died => {
                    eprintln!(
                        "daemoneye: daemon exited during startup without reporting — \
                         see ~/.daemoneye/var/log/daemon.log"
                    );
                    std::process::exit(1);
                }
            };
        }
        // Child: drop the read end, keep the write end as our reporter.
        drop(read_end);
        daemon::ready::set_reporter(write_end);

        // SAFETY: (existing comment about setsid/dup2, unchanged)
        unsafe {
            if libc::setsid() < 0 { … }   // existing body, unchanged
            …
        }
    }
```

Two constraints on this restructuring:

- **`drop(write_end)` in the parent is load-bearing.** If the parent keeps a copy
  of the write end, the pipe never reaches EOF and a child that dies without
  reporting hangs the parent forever. Same for `drop(read_end)` in the child,
  which otherwise leaks a descriptor into the daemon.
- **The `/dev/null` stdin redirect must stay as it is.** `pipe(2)` returns the
  lowest free descriptors, which are ≥ 3 because 0/1/2 are open, and
  `run_daemon`'s `dup2` only touches 1 and 2 — so the reporter fd survives both
  redirects untouched. Do not renumber it, and do not add `FD_CLOEXEC`; nothing
  `exec`s here.

`libc::fork()` may be pulled out of the big `unsafe` block as shown so the
control flow after it is safe code, or left inside it — either is acceptable.
Keep both existing `// SAFETY:` comments; they are still accurate.

### 3. Report success from `run_daemon`

In `src/daemon/mod.rs`, immediately after the
`log::info!("Daemon listening on {}", socket_path.display());` line (currently
`mod.rs:760`) and before the accept loop:

```rust
    ready::report_ready();
```

After the bind, not before: the socket existing is the condition a client cares
about, and it is the last thing that can fail during startup.

### 4. Report failure from the daemon entry point

In `src/main.rs`'s `async_main`, change the `Commands::Daemon` arm's
`daemon::run_daemon(log_file, session).await?;` to report before propagating:

```rust
            if let Err(e) = daemon::run_daemon(log_file, session).await {
                daemon::ready::report_failure(&e.to_string());
                return Err(e);
            }
```

This single site covers every failure in the Current-state table, including the
two the preceding phases added, because they all surface as an `Err` out of
`run_daemon`.

`report_failure` is also a no-op under `--console` (no reporter installed), so
the `--console` path keeps behaving exactly as it does today.

### 5. Update `CLAUDE.md`

Add to the `## Key files` table:

```markdown
| `src/daemon/ready.rs` | Fork readiness handshake — child reports `READY` / `ERR <msg>` to the parent over a pipe |
```

And to `## Important Invariants`:

```markdown
- `daemoneye daemon` (without `--console`) does not report success until the
  forked child has bound its socket. The parent relays the child's outcome over
  the readiness pipe (`src/daemon/ready.rs`) and exits non-zero if the child
  failed or died. The parent must drop its copy of the write end before reading,
  or a child that dies silently hangs it.
```

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes: existing tests plus the 7 new ones below.
- [ ] `grep -c "unsafe" src/daemon/ready.rs` returns `2` — both inside
      `create_pipe`, and no other `unsafe` anywhere in the phase's diff.
- [ ] `grep -n "libc::close" src/daemon/ready.rs src/main.rs` shows no new
      occurrence (`OwnedFd` handles closing; the pre-existing
      `libc::close(devnull)` in `main.rs` stays).
- [ ] A duplicate `daemoneye daemon` exits non-zero with the instance-lock
      message on **stderr** (End-to-end verification).

## Test plan

`parse_report_line` is pure and carries the protocol — it gets the coverage.
`create_pipe` + `await_child_report` are testable in-process without forking:
create the pipe, write into the write end, drop it, read the report.

In `src/daemon/ready.rs`:

- `parses_ready` — `"READY\n"` → `ChildReport::Ready`.
- `parses_failure_with_message` — `"ERR another daemon is already running (PID 42)\n"`
  → `Failed("another daemon is already running (PID 42)")`. Pins that inner
  spaces and parentheses survive intact.
- `parses_unknown_line_as_died` — each of `""`, `"\n"`, `"READYISH\n"`,
  `"ERR\n"`, `"ready\n"` → `Died`. The lowercase case pins that the match is
  exact and not case-insensitive; `"READYISH"` pins that it is not a prefix
  match.
- `await_report_reads_ready_then_returns` — write `READY\n` into the write end,
  **keep the write end alive**, and assert `await_child_report` returns `Ready`
  without blocking. This is the test that pins `read_line` over
  `read_to_string`: the latter would hang here, so a mutation to
  `read_to_string` fails by timing out rather than by asserting — note that in a
  comment on the test so a future reader knows the hang is the signal.
- `await_report_returns_died_on_eof` — drop the write end without writing →
  `Died`.
- `await_report_returns_failure` — write `ERR boom\n` → `Failed("boom")`.
- `report_ready_without_reporter_is_a_noop` — call `report_ready()` with no
  reporter installed and assert it returns normally. Guards the `--console` path.

The reporter is process-global state, so any test that calls `set_reporter` must
not run concurrently with another that does. There is precedent for this in the
crate: serialize them behind a module-local `static REPORTER_TEST_LOCK:
Mutex<()>` (do **not** reuse `crate::TEST_HOME_LOCK` — this has nothing to do
with `HOME`, and borrowing it would couple unrelated suites).

## End-to-end verification

The whole point of this phase is the exit code and message of a real
`daemoneye daemon` invocation — unit tests cannot observe either. Quote actual
output in the Update Log.

```bash
cargo build --release

# 1. Honest success: parent waits for the bind, then reports.
./target/release/daemoneye daemon; echo "exit=$?"
# expect: daemoneye daemon started (PID <n>)   /   exit=0
./target/release/daemoneye ping     # expect "Daemon is running." — already bound
#   when the parent returned, so this must not need a sleep

# 2. Honest duplicate failure — the phase-08 message now reaches the user.
./target/release/daemoneye daemon; echo "exit=$?"
# expect exit=1 and on STDERR:
#   daemoneye: daemon failed to start: another daemon is already running
#   (PID <n>) — stop it with: daemoneye stop
./target/release/daemoneye daemon 2>/dev/null; echo "exit=$?"
# expect exit=1 and NO stdout output at all (the message must be on stderr)

# 3. The first daemon is still healthy after the failed duplicate.
./target/release/daemoneye ping     # expect "Daemon is running."
./target/release/daemoneye stop

# 4. Honest config failure. Break the API key and confirm the real reason
#    reaches the terminal instead of "started (PID n)".
DAEMONEYE_TEST_HOME=$(mktemp -d)
HOME="$DAEMONEYE_TEST_HOME" ./target/release/daemoneye daemon; echo "exit=$?"
# expect exit=1 and a stderr line naming the missing API key, e.g.:
#   daemoneye: daemon failed to start: No API key found for provider '…'
rm -rf "$DAEMONEYE_TEST_HOME"

# 5. --console is unchanged: runs in the foreground, no handshake.
timeout 5 ./target/release/daemoneye daemon --console; echo "exit=$?"
# expect it to run in the foreground until the timeout kills it (exit=124)
```

Step 1's `ping` with no `sleep` is the real assertion that the handshake works:
before this phase the parent returned before the socket existed, so an immediate
`ping` could race and report "not running".

## Authorizations

- [x] May write `unsafe` — **only** the two blocks inside `ready::create_pipe`
      wrapping `libc::pipe` and `OwnedFd::from_raw_fd`, each with a `// SAFETY:`
      comment as shown in task 1. This is the exception STANDARDS § 1 requires
      principal-engineer sign-off for; it is granted here and nowhere else in
      this phase. Any other `unsafe` is a blocker.
- [x] May add the new file `src/daemon/ready.rs`.
- [x] May edit `CLAUDE.md` § "Key files" and § "Important Invariants" (task 5).
- [x] May restructure the existing `unsafe` block in `src/main.rs:256-294` as
      described in task 2, keeping both `// SAFETY:` comments.
- [ ] No new dependencies. `libc` is already a dependency (`Cargo.toml:21`).

## Out of scope

- **Do not move the fork, or make it conditional, or add async work before it.**
  `main()` being synchronous until after the fork is a hard invariant
  (`CLAUDE.md`); forking a live multi-threaded runtime is unsound.
- **Do not add a timeout, `sleep`, or `waitpid` to the parent's wait.** EOF on
  the pipe already covers the child-died case — task 1 explains why.
- **Do not use the handshake to pass anything other than the startup outcome.**
  No PID, no socket path, no config echo. One line, three shapes.
- **Do not report readiness from anywhere other than the post-bind site in
  `run_daemon`** (task 3), and do not report failure from anywhere other than the
  `async_main` arm (task 4). Scattering reports through the startup path makes
  the protocol unanalyzable.
- **Do not change `--console` behavior.** No reporter is installed on that path
  and both report functions must stay no-ops.
- **Do not touch `InstanceLock`, `webhook::bind`, `daemon_liveness`, or
  `log_event`.** Phases 08–10 own those; this phase only relays the errors they
  produce.
- **Do not add `FD_CLOEXEC`** to either pipe end, and do not renumber the
  descriptors.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
