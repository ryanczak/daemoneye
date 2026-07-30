# Phase 11: Fork Readiness Handshake — Make `daemoneye daemon` Tell the Truth

**Milestone:** M5 — UX & Stability
**Status:** done
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
6. Verify the starting state:

```bash
ls src/daemon/ready.rs 2>&1                              # expect "No such file" — task 1 creates it
grep -c 'libc::fork()' src/main.rs                       # expect 1
grep -c 'libc::close' src/main.rs                        # expect 1 (the pre-existing devnull close)
grep -c 'Daemon listening on' src/daemon/mod.rs          # expect 1  (task 3's anchor)
grep -c 'InstanceLock::acquire' src/daemon/mod.rs        # expect 1  (phase 08 landed)
grep -c 'webhook::bind' src/daemon/mod.rs                # expect 1  (phase 09 landed)
grep -n '^libc' Cargo.toml                               # expect line 21: libc = "0.2"
cargo test 2>&1 | grep "^test result" | head -3   # expect 940 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
on 2026-07-30, immediately before dispatch.** If one differs, **stop and report a
blocker**.

> **Use `cargo test`, not `cargo test --lib`.** The full command prints **three**
> `test result` lines; `--lib` prints only the first.

## Current state

### The fork — `src/main.rs:256-296`

> **⚠ Line numbers refreshed 2026-07-30 before dispatch.** Drafted 2026-07-26;
> phases 07–10 have edited `src/daemon/mod.rs` heavily since, shifting its
> references by **+118**. `src/main.rs` moved by only +2. Every code quote is
> byte-identical to the tree as of the refresh.

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
| no API key configured | pre-existing, `mod.rs:456` |
| tmux session could not be created | pre-existing, `mod.rs:~515` (the `new-session` arm) |
| socket bind failed | pre-existing, `mod.rs:868` |

Every one of these currently reaches the user as `started (PID n)`, exit `0`,
and a line in a log file they have no reason to open.

### `async_main`'s daemon arm — `src/main.rs:304-316`

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

Import **exactly** `use std::os::fd::{FromRawFd, OwnedFd};`.

**⚠ Do NOT import `AsRawFd`.** An earlier draft of this doc told you to, and it is
wrong: nothing in this module calls `as_raw_fd()`, and under
`cargo clippy --all-targets --all-features -- -D warnings` an **unused import is an
error, not a warning** — so importing it fails the lint gate. Verified at the
refresh:

```
error: unused import: `AsRawFd`
 --> src/daemon/ready.rs:2:19
error: could not compile `daemoneye` (lib) due to 1 previous error
```

`OwnedFd` closes on drop, so there is no manual `libc::close` anywhere in this
phase — do not add any.

**The whole module as specified here was compile-verified at the refresh** (build
clean, clippy `-D warnings` clean) with that one import correction. `std::fs::File`
implements `From<OwnedFd>`, so `File::from(fd)` works; `UnpoisonExt` comes from
`use crate::util::UnpoisonExt;`.

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
**`mod.rs:878`** — re-derive with `grep -n 'Daemon listening on' src/daemon/mod.rs`)
and before the accept loop:

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
- [ ] `cargo test 2>&1 | grep "^test result"` shows the lib count at **947**
      (940 + 7 new) and integration at **27**. Equivalently, and this is the check
      that matters: the lib count is **exactly 7 higher** than the 940 you recorded
      in Pre-flight. **If it is anything else, stop and report a blocker naming the
      number you measured — do not re-run the command hoping for a different
      answer.**
- [ ] `grep -c "unsafe" src/daemon/ready.rs` returns **2** — both inside
      `create_pipe`. *(Verified satisfiable at the refresh against a
      spec-following implementation.)*
- [ ] `git diff -U0 HEAD -- src/ | grep '^+' | grep -c unsafe` returns **3**, and
      `git diff -U0 HEAD -- src/ | grep '^+' | grep unsafe` shows exactly these
      three lines:

      ```
      +    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
      +    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
      +        let pid = unsafe { libc::fork() };
      ```

      **⚠ Both the command and the number were wrong in the previous draft, and
      the run stalled on them. Corrected 2026-07-30.**
      - **`HEAD`, not a bare `git diff`.** A bare `git diff` shows only *unstaged*
        changes, so it returns **0** the moment you `git add` — and you will.
        `HEAD` pins the baseline, per `WORKFLOW.md` § "Every acceptance criterion
        must be satisfiable, and its mechanics pinned": *if a criterion asks the
        executor to prove a property of its own diff, pin the baseline commit.*
      - **Three, not two.** Task 2 tells you to pull `libc::fork()` out of the big
        `unsafe` block, which necessarily *adds* a third `unsafe` line in
        `main.rs`. The old "2" contradicted this doc's own task 2.

      **If your count is not 3, compare against the three lines above rather than
      re-running the command — and if a line differs, report a blocker naming what
      you see.** Re-running a read-only command that already answered makes no
      progress and will trip the governor. That is exactly how the previous run
      ended.
- [ ] `grep -c 'libc::close' src/main.rs` returns **1** — the pre-existing
      `libc::close(devnull)` at `main.rs:291`, unchanged — and
      `grep -c 'libc::close' src/daemon/ready.rs` returns **0**.
      **⚠ Phrased as counts deliberately.** The earlier wording, "shows no new
      occurrence", is unverifiable: the grep *will* match the pre-existing line, so
      there is no output that distinguishes pass from fail.
- [ ] `grep -c 'AsRawFd' src/daemon/ready.rs` returns **0** — see the import note
      in task 1; importing it is a lint-gate error.
- [ ] `grep -c 'pub mod ready' src/daemon/mod.rs` returns **1**.
- [ ] `grep -c 'report_ready()' src/daemon/mod.rs` returns **1** and
      `grep -c 'report_failure(' src/main.rs` returns **1** — one report site each,
      as Out-of-scope requires.
- [ ] A duplicate `daemoneye daemon` exits non-zero with the instance-lock
      message on **stderr** (End-to-end verification).

### ⚠ How to check the test count — read this before checking it

Two commands, once each:

```bash
cargo test 2>&1 | grep "^test result"     # three lines; lib is the first
cargo test 2>&1 | grep -E 'parses_|await_report|report_ready_without'   # the 7 new tests
```

**Do not count tests by grepping the per-test `^test ` lines** — those totals do
not agree with the summary. The summary line is authoritative. **A number that
disagrees with this doc means the doc is wrong; say so and report a blocker.**
Re-running a read-only command that already answered makes no progress and will
trip the governor.

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
- [x] May restructure the existing `unsafe` block in `src/main.rs:256-296` as
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

### Update — 2026-07-30 11:58 (started)

**Executor:** model (phase-11 executor)

Implemented the fork readiness handshake: new `src/daemon/ready.rs` module with pipe-based `READY`/`ERR` protocol, parent waits for child report before exiting, child reports success after socket bind and failure from `async_main`. 7 new unit tests.

### Notes for executor — 2026-07-30 (pre-dispatch refresh)

Drafted 2026-07-26; phases 07–10 have edited `src/daemon/mod.rs` heavily since, so
the architect re-derived every fact and **compile-verified the new module**. Four
corrections, one of which would have failed a gate outright:

1. **⚠ Do NOT import `AsRawFd`.** The original task-1 text said
   `use std::os::fd::{OwnedFd, FromRawFd, AsRawFd};`, but nothing in this module
   calls `as_raw_fd()` — and under `clippy -D warnings` an **unused import is an
   error**. Following the doc literally would have failed the lint gate. Import
   exactly `use std::os::fd::{FromRawFd, OwnedFd};`.
2. **The whole module was compile-verified at the refresh** — written as this doc
   specifies (with the import fixed), `cargo build` and
   `cargo clippy --all-targets --all-features -- -D warnings` both came back clean.
   `std::fs::File` implements `From<OwnedFd>`, so `File::from(fd)` works, and
   `UnpoisonExt` is `use crate::util::UnpoisonExt;`. The `grep -c "unsafe"
   src/daemon/ready.rs` → **2** criterion was checked against that implementation
   and is satisfiable.
3. **`src/daemon/mod.rs` line numbers moved by +118.** Task 3's anchor —
   `log::info!("Daemon listening on …")` — is at **`:878`**, not `:760`. The socket
   bind is `:868`, the API-key bail `:456`. `src/main.rs` moved by only +2: the
   fork block is `256-296`, `async_main` still `:304`, its `run_daemon` call `:316`.
4. **⚠ An acceptance criterion was unverifiable and is now a count.** It said
   `grep -n "libc::close" …` "shows no new occurrence" — but that grep *will* match
   the pre-existing `libc::close(devnull)` at `main.rs:291`, so no output
   distinguishes pass from fail. It now pins `main.rs` at **1 unchanged** and
   `ready.rs` at **0**.

Test baseline is **940**; this phase adds 7, giving **947**. Count with
`cargo test 2>&1 | grep "^test result"` — once. If a number disagrees with this
doc, the doc is wrong: report a blocker naming what you measured rather than
re-running.

**Two hazards this phase's design turns on — do not "simplify" either:**

- **`drop(write_end)` in the parent** before reading. Keep a copy and the pipe
  never reaches EOF, so a child that dies silently hangs the parent **forever**.
  There is no timeout to save you, deliberately.
- **`read_line`, not `read_to_string`.** On success the child never closes its
  write end, so reading to EOF blocks for the daemon's whole lifetime. The test
  `await_report_reads_ready_then_returns` keeps the write end alive precisely so a
  `read_to_string` mutation **hangs** rather than fails — the hang is the signal.

**On the E2E:** it starts, duplicates, and stops a real daemon and repoints global
tmux hooks. No daemon is running as of this refresh. Step 4 uses a throwaway
`HOME`, so it will not touch the real config. State what you leave behind.

### Update — 2026-07-30 (escalation)

**Chosen lever:** resume (`continue_phase`) after correcting the criterion
**Rationale:** The implementation is complete and all four gates pass — the run
stalled for 60 read-only turns on an acceptance criterion of mine that was
unsatisfiable two ways over, so fixing the criterion removes the loop's cause;
resume rather than re-dispatch because the **E2E has not run yet** (real work the
executor can still reach) and re-dispatch would throw away ~220 lines of correct
fd/fork work to re-derive it.

### Notes for executor — 2026-07-30 (resumed)

## Your implementation is complete and correct. Do not redo it.

Verified at the escalation: `cargo fmt --all --check`, `cargo build`,
`cargo clippy --all-targets --all-features -- -D warnings` and `cargo test` all
green, with **947** lib tests (940 + 7) and all seven `daemon::ready::tests`
present and passing. `src/daemon/ready.rs` (+168), `src/main.rs` (+39 −12),
`src/daemon/mod.rs` (+2) and `CLAUDE.md` (+6) are on disk and **staged**.

**The previous run failed on my criterion, not on your code.** It said

```
git diff -U0 -- src/ | grep '^+' | grep -c unsafe    → 2
```

and both halves were wrong: a bare `git diff` shows only *unstaged* changes, so it
returned **0** once you staged; and the true count is **3**, because this doc's own
task 2 tells you to pull `libc::fork()` out of the big `unsafe` block. You could
not have made that command print 2. It is now `git diff -U0 HEAD -- src/ …` → **3**,
with the three exact lines listed for comparison, and it passes against your work
as it stands.

## What is actually left

**Only the End-to-end verification.** It never ran — the stall happened first.
Work through the five scenarios in the E2E section against
`./target/release/daemoneye` and quote the real output for each:

1. Honest success — `daemon` then an immediate `ping` with **no sleep**. That
   `ping` is the real assertion: before this phase the parent returned before the
   socket existed.
2. Honest duplicate failure — exit **1**, the instance-lock message on **stderr**,
   and **nothing on stdout** when stderr is redirected away.
3. The first daemon still healthy after the failed duplicate, then `stop`.
4. Honest config failure under a throwaway `HOME` (`mktemp -d`) — exit **1** naming
   the missing API key. This will not touch the real config.
5. `--console` unchanged — runs in the foreground until `timeout` kills it
   (exit **124**).

No daemon is running right now, so scenario 1 should start cleanly. **Say what
state you leave the host in** when you are done.

## Do not

- Do not modify `src/daemon/ready.rs`, `src/main.rs`, `src/daemon/mod.rs` or
  `CLAUDE.md` further unless the E2E reveals an actual defect.
- Do not add tests. The count must stay at **947** — 947, not 948.
- Do not re-run the unit gates more than once to confirm; they were verified green
  at the escalation.
- **Do not re-run a read-only command that already gave you its answer.** If a
  number disagrees with this doc, the doc is wrong: report a blocker naming what
  you measured. That is what ended the previous run.

### Update — ts=1785414253584 (complete, server-authored)

**Summary:** Good — the working tree is clean (the previous commit already included the code changes and the escalation doc update). The phase doc's status is `in-progress` as required, and the server will handle the completion bookkeeping.

**Summary + Notes for review:**

The fork readiness handshake is fully implemented and verified. All 5 end-to-end scenarios passed against `./target/release/daemoneye`: (1) honest success with immediate `ping` succeeding with no sleep — proving the parent waits for the child's socket bind; (2) duplicate daemon exits 1 with the instance-lock message on stderr and nothing on stdout; (3) the original daemon remained healthy after the failed duplicate; (4) config failure under a throwaway HOME exits 1 with the real "No API key found" message; (5) `--console` runs in the foreground unchanged, killed by timeout at exit 124. No daemon is running on the host. The implementation (ready.rs, main.rs, mod.rs, CLAUDE.md) was already committed by the prior executor and all gates (fmt, build, clippy, 947 tests) were verified green at escalation — I did not modify any source files, only ran the E2E verification.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.98s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


TEST
::tests::auth_missing_header_denies ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test memory::tests::memory_without_frontmatter_has_no_tags ... ok
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

test result: ok. 947 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.26s


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
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

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

(none)

**Commit:** be0a69e1aff5dfc2c94743c85c28c613d0bb3666

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-30

- **Verdict:** approved_after_1
- **Bounces:** 1 (`hard_fail`, `NoProgressStall` at 60 read-only turns — **caused by
  an architect criterion that was unsatisfiable two ways over**; no bug doc, handled
  via escalation → resume)
- **Executor:** Qwen/Qwen3.6-27B-FP8 (109 turns to a complete implementation, then
  **33** on resume to finish the E2E)
- **Scope deviations:** none by the executor. **One by the architect** — see
  "Commit hygiene" below.
- **Calibration:** eighth architect criterion defect in M5; third to cost a run.

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build` after
`touch`ing `src/daemon/ready.rs` and `src/main.rs` — zero warnings, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo test` at **947** lib (940 + 7)
+ **27** integration).

### Every acceptance criterion verified

`unsafe` in `ready.rs` **2**; added `unsafe` vs the pre-phase baseline **3** and
exactly the three expected lines; `libc::close` **1** in `main.rs` (pre-existing) and
**0** in `ready.rs`; `AsRawFd` **0**; `pub mod ready` **1**; `report_ready()` **1** in
`mod.rs` and `report_failure(` **1** in `main.rs` — one report site each, as
Out-of-scope required; zero `TODO`/`dbg!`/`#[allow]`/`#[ignore]`; and **zero**
`unwrap`/`expect`/`panic!` in `ready.rs`'s production half.

### Both load-bearing hazards are implemented correctly

`drop(write_end)` sits in the parent before the read, with the comment explaining
why, and `drop(read_end)` in the child. `read_line` at `ready.rs:73`, never
`read_to_string`.

### The unusual mutation behaves as predicted — and there is a second layer

The spec warned that a `read_to_string` mutation manifests as a **hang**, not an
assertion failure, because the child never closes its write end on success. Both
halves checked:

| Mutation | Result |
|---|---|
| `read_line` → `read_to_string` as-is | **compile error** — `Read` is not imported, so the method is not in scope |
| same, with `use std::io::Read` added | **hangs**; killed by `timeout 40` (`Terminated`) |

Restored, the test passes in 0.00 s. So the import list gives an accidental but real
first line of defence, and the prediction holds underneath it.

### End-to-end: one scenario re-run by me, four accepted from the executor's run

I re-ran **scenario 4** (config failure under a throwaway `HOME`) because it proves
the whole mechanism — parent waits, child reports `ERR`, parent relays and exits
non-zero — with **zero blast radius**:

```
exit=1
--- STDOUT (must NOT say 'started') ---
                                          ← empty
--- STDERR ---
daemoneye: daemon failed to start: No API key found for provider 'anthropic'. Set 'api_key' in [models.default] in ~/.daemoneye/etc/config.toml  or  export ANTHROPIC_API_KEY=<your-key>
```

Under the old code this printed `daemoneye daemon started (PID n)` and exited **0**.
That is the defect this phase existed to fix, and it is fixed.

**Scenarios 1, 2, 3 and 5 I accept from the executor's run**, which quoted real
output for each: the immediate `ping` succeeding with no sleep (the proof the parent
waits for the bind), the duplicate exiting 1 with the instance-lock message on stderr
and nothing on stdout, the original daemon healthy afterwards, and `--console`
unchanged at exit 124. Re-running them myself would start, duplicate and kill a real
daemon and repoint global tmux hooks on the live server — disproportionate for a
success path already covered in-process by
`await_report_reads_ready_then_returns`. The executor reported no daemon left
running; confirmed.

### ⚠ Commit hygiene — an architect error, unresolved by design

**`be0a69e`, labelled `docs: escalate M5 phase-11 …`, actually contains the entire
implementation:**

```
 phase-11-fork-readiness-handshake.md  | 100 +++++-
 src/daemon/mod.rs                     |   2 +
 src/daemon/ready.rs                   | 168 +++++++++++++
 src/main.rs                           |  51 ++++--
```

Cause: the stalled run had `git add -A`'d its work, so it sat in the index; I then
ran `git add -A docs/` followed by a plain `git commit`, which commits **everything
staged**. A ~220-line feature landed under a `docs:` message.

This violated the DoD's "one conventional commit per logical change." **Not
bounced** — the executor cannot fix an architect's commit.

**RESOLVED 2026-07-30 on PE instruction: history rewritten.** `be0a69e` was split,
and the two docs commits that followed it were replayed unchanged, giving:

| Commit | Message | Contents |
|---|---|---|
| `ae4e833` | `feat: fork readiness handshake — parent relays the child's real startup outcome` | `src/` only (3 files, +209 −12) |
| `a477f22` | `docs: escalate M5 phase-11 — correct doubly-unsatisfiable unsafe-diff criterion …` | docs only |
| `53ff4c1` | `docs: server-authored completion bookkeeping` | docs only |
| `c30f5b8` | `docs: approve M5 phase-11 (done, approved_after_1); eighth exit criterion met` | docs only |

No commit now mixes `src/` and `docs/`. **Verified content-identical to the
pre-rewrite state**: the final tree hash is `b1eda0d…` before and after, and
`git diff <old-HEAD> HEAD` is empty. `cargo build` and `cargo test` (947 + 27) still
green afterwards. The rewrite was safe to do because `src/` was touched by *only*
`be0a69e` in the replayed range, so the split needed no content reconstruction.

**Lesson, and it is a small one worth keeping:** after an escalation that touches a
tree the executor has already staged, `git commit -- <paths>` or a check of
`git diff --cached --name-only` before committing. `git add <subset>` does not narrow
what a following `git commit` picks up.

### The bounce: my criterion, unsatisfiable two ways over

```
git diff -U0 -- src/ | grep '^+' | grep -c unsafe    → expected 2
```

- **Wrong mechanism.** A bare `git diff` shows only *unstaged* changes, so it returned
  **0** once the executor staged. `WORKFLOW.md` § "Every acceptance criterion must be
  satisfiable, and its mechanics pinned" already folded this exact lesson — *pin the
  baseline commit* — and I did not.
- **Wrong number.** The truth is **3**; the third `unsafe` line is one *my own task 2*
  instructs the executor to create by pulling `libc::fork()` out of the big block.

The executor could not have made that command print 2 by any route. It thrashed
between `git diff` and `git diff --cached` for 60 turns until the governor stopped it
— having already finished the implementation and passed every gate.

**Eighth criterion defect of mine in M5, third to cost a run, and the second of the
three that was a diff-based criterion with no pinned baseline** — a species the
workflow had already folded. That it recurred anyway is the argument for a mechanical
check rather than more prose.

### The resume lever was the right call

Re-dispatching would have discarded 220 lines of correct fd/fork work — the most
delicate code in the milestone — to re-derive it. Resume kept it, the corrected
criterion removed the loop's cause, and the outstanding E2E finished in **33 turns**.
It also preserved the model's telemetry data point, which a takeover would have
forfeited.
