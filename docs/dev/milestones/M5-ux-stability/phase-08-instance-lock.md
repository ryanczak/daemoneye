# Phase 08: Instance Lock — One Daemon, Enforced by the Kernel

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** none (independent of the 04x lock-conversion sequence)
**Estimated diff:** ~260 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Make "only one daemon per `$HOME`" an OS-enforced invariant instead of an
inference from a 2-second Ping timeout, and stop a second daemon from unlinking
a live daemon's socket. Introduces `InstanceLock` — an exclusive `flock` on
`~/.daemoneye/var/run/daemoneye.pid` — acquired before any startup side effect.

## Architecture references

Read before starting:

- `docs/design/daemon-instance.md` § 1 — the 2026-07-25 incident and why the
  existing guard failed. § 1.1 has the four-cases-collapse-to-`false` table.
- `docs/design/daemon-instance.md` § 2 — the three ownership rules this phase
  implements. § 2.3's table lists every side effect that currently runs *before*
  the guard; moving the lock ahead of them is task 4.
- `CLAUDE.md` § "Important Invariants" — `main()` is synchronous so `fork()`
  precedes the tokio runtime. This phase does not move the fork.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "daemon_is_running" src/daemon/mod.rs        # expect 2 (definition + the call task 5 deletes)
grep -n "^nix" Cargo.toml                           # expect line 23: nix = "0.31.1"
grep -rc '\bnix::' src/ | grep -v ":0" | wc -l      # expect 0 — nothing uses nix today
                                                    # (the \b matters: a bare `nix::`
                                                    # also matches std::os::unix:: — 10 hits)
ls src/daemon/instance.rs 2>&1                      # expect "No such file" — task 2 creates it
cargo test 2>&1 | grep "^test result" | head -3     # expect 921 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
on 2026-07-29, immediately before dispatch.** If one differs, **stop and report a
blocker**.

## Current state

> **⚠ Line numbers in this section were refreshed 2026-07-29 before dispatch.**
> The phase was drafted 2026-07-26; phase 06h then converted 9 tmux call sites in
> this same file, shifting everything below them by up to **+52** (the guard moved
> `739` → `791`). **Every code quote below is byte-identical to the tree as of the
> refresh** — only the numbers changed. If a number is still off, re-derive with
> the grep beside it and trust the tree, not the doc.

### The guard and the unlink, `src/daemon/mod.rs:789-812`

Re-derive with `grep -n 'daemon_is_running()\|symlink_metadata' src/daemon/mod.rs`.

```rust
    let socket_path: PathBuf = default_socket_path();

    if daemon_is_running().await {
        anyhow::bail!(
            "A daemon is already running on {}.\n\
             Stop it with:  daemoneye stop",
            socket_path.display(),
        );
    }

    // Use symlink_metadata() (does not follow symlinks) so a symlink at the
    // socket path removes the symlink itself rather than its target (S3).
    match socket_path.symlink_metadata() {
        Ok(_) => {
            std::fs::remove_file(&socket_path).context("Failed to remove stale socket file")?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Failed to stat socket path"),
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind to socket at {}", socket_path.display()))?;

    log::info!("Daemon listening on {}", socket_path.display());
```

The `symlink_metadata` → `remove_file` pair is the hijack: reached whenever
`daemon_is_running()` returns `false`, which includes "alive but did not answer
within 2 s". **Keep the S3 symlink behavior** — it is a real fix for a real
issue; only its *authorization* changes.

### Unconditional teardown, `src/daemon/mod.rs:855-866`

Re-derive with `grep -n 'Graceful shutdown' src/daemon/mod.rs`.

```rust
    // ── Graceful shutdown ────────────────────────────────────────────────────
    // 1. Remove the socket so new clients get a clean "not running" error.
    let _ = std::fs::remove_file(&socket_path);

    // 2. Uninstall global tmux hooks so they don't fire against a dead daemon.
    for hook in &[
        "pane-died",
        "after-new-session",
        "client-attached",
        "client-detached",
    ] {
```

### The startup ordering problem

`run_daemon` (`src/daemon/mod.rs:327`) performs these in this order. The guard is
at **791** — everything numbered above it happens *first*:

| Line | What happens |
|---|---|
| 335 | `env_logger` init |
| 355 | `log_file` → `dup2` onto stdout/stderr |
| 371 | `Config::load()` |
| 386 | `crate::memory::migrate_namespace()` |
| 396 | deletes `de-pipe-*.log` files |
| 492 | `log_event("daemon_start", …)` |
| 514, 532, 553, 571 | four `tmux set-hook -g` calls |
| 585 | `install_session_hooks(&sn, &hp)` |
| 626 | cache poller spawned (`supervise`) |
| 664 | scheduler spawned |
| 701 | webhook spawned |
| 727, 771 | two further `supervise` tasks |
| **791** | **`daemon_is_running()` guard** |

**These numbers are current as of 2026-07-29.** Re-derive the whole table with:

```bash
grep -n 'env_logger::Builder\|dup2(fd, 1)\|Config::load\|migrate_namespace\|de-pipe-\|log_event(\|"set-hook", "-g"\|install_session_hooks(&sn\|tokio::spawn(supervise(\|daemon_is_running()' src/daemon/mod.rs
```

The point of the table is the **ordering**, not the exact integers: eleven-plus
side effects, several of them destructive and none of them reversible by
`anyhow::bail!`, all run *before* the only duplicate check. That is what task 4
fixes, and it stays true regardless of how the numbers drift.

### `var_run_dir()` already anticipates this

```rust
// src/config/load.rs:19-22
/// `~/.daemoneye/var/run/` — sockets, lock files, mutable runtime state.
pub fn var_run_dir() -> PathBuf {
    config_dir().join("var/run")
}
```

`Config::ensure_dirs()` (`src/config/seeds.rs:13`) creates it, and `main()` calls
`ensure_dirs()` first (`src/main.rs:249`), so the PID file's parent directory
always exists by the time `run_daemon` runs. **Do not add a `create_dir_all`.**

### `nix` is a declared dependency with no features enabled

`Cargo.toml:23` is `nix = "0.31.1"`. Nothing in `src/` uses `nix::` today. The
`Flock` wrapper lives behind nix's `fs` feature, which is off — task 1 turns it
on. This is authorized (see Authorizations).

## Spec

### 1. Enable the `nix` `fs` feature

In `Cargo.toml`, change line 23 from `nix = "0.31.1"` to:

```toml
nix = { version = "0.31.1", features = ["fs"] }
```

Nothing else in `Cargo.toml` changes. Do not bump the version.

**Why a wrapper and not `libc::flock` directly:** `STANDARDS.md` § 1 forbids
`unsafe` without authorization, and this phase does not authorize it.
`nix::fcntl::Flock` is a safe RAII wrapper that releases on `Drop`. Use it.

#### Reference excerpt — the exact `nix` 0.31.1 API

You cannot fetch docs. This is the real signature, read from the vendored source
at `nix-0.31.1/src/fcntl.rs:1038-1100`:

```rust
pub struct Flock<T: Flockable>(T);

impl<T: Flockable> Flock<T> {
    /// On failure returns the value back, paired with the errno.
    pub fn lock(t: T, args: FlockArg) -> std::result::Result<Self, (T, Errno)>;
}

impl<T: Flockable> Drop for Flock<T> { /* flock(fd, LOCK_UN) */ }
impl<T: Flockable> Deref for Flock<T> { type Target = T; }
impl<T: Flockable> DerefMut for Flock<T> { … }

unsafe impl Flockable for std::fs::File {}
```

`FlockArg::LockExclusiveNonblock` maps to `LOCK_EX | LOCK_NB`
(`fcntl.rs:1091`). Import path: `nix::fcntl::{Flock, FlockArg}`; the errno type
is `nix::errno::Errno`.

**This API shape was compile-verified against the real crate on 2026-07-29**, in
this tree with `features = ["fs"]` added: `Flock::lock(file,
FlockArg::LockExclusiveNonblock)`, the `Err((back, Errno::EWOULDBLOCK))`
destructuring with `back` usable as a `File` afterwards, and `set_len(0)` /
`rewind()` / `writeln!(&mut *guard, …)` / `flush()` through `DerefMut` all compile
with **zero warnings**. Without the `fs` feature the import alone is
`error[E0432]: unresolved imports nix::fcntl::Flock, nix::fcntl::FlockArg` — which
is what you will see if you skip task 1.

Three properties that matter for the spec below:

- `lock()` returns `Err((file, errno))` — the `File` comes **back** on failure,
  so you can still read the existing PID out of it for the error message.
- `Flock<File>` derefs to `File`, so `writeln!(&mut *guard, …)` writes into the
  locked file without reopening it.
- Contention errno is `Errno::EWOULDBLOCK`. On Linux `EAGAIN` and
  `EWOULDBLOCK` are the same value, so matching `Errno::EWOULDBLOCK` alone is
  sufficient — do **not** write a two-arm match, it will not compile (unreachable
  duplicate pattern).

### 2. New module `src/daemon/instance.rs`

Declare it in `src/daemon/mod.rs` alongside the existing submodules
(`pub mod instance;`).

Public surface — exactly these four items, nothing more:

```rust
pub struct InstanceLock { … }

pub enum AcquireError {
    /// Another daemon holds the lock. `pid` is its PID if the file was readable
    /// and parsable; `None` when the payload was absent or malformed.
    Held { pid: Option<u32> },
    /// The lock file could not be opened or written.
    Io(std::io::Error),
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> Result<Self, AcquireError>;
    pub fn pid_path(&self) -> &Path;
}

/// Reads the PID payload without taking the lock. Returns `None` if the file is
/// absent, unreadable, empty, or not a bare integer.
pub fn read_pid(path: &Path) -> Option<u32>;
```

`acquire` behavior, in order:

1. Open `path` with `create(true).read(true).write(true)` — **not**
   `truncate(true)`. Truncating before the lock is taken would erase the
   incumbent's PID payload on a failed acquisition, destroying the diagnostic
   this type exists to provide.
2. `Flock::lock(file, FlockArg::LockExclusiveNonblock)`.
3. On `Err((file, Errno::EWOULDBLOCK))` → read the PID out of the returned
   `file` and return `AcquireError::Held { pid }`. On any other errno → map to
   `AcquireError::Io`.
4. On success: truncate to 0, rewind, write `format!("{}\n", std::process::id())`,
   `flush()`. Truncation is safe here because the lock is held.
5. Store the `Flock<File>` and the path in the returned `InstanceLock`.

`InstanceLock` must own the `Flock<File>` for its whole lifetime — the lock
releases when the `Flock` drops. Do **not** implement `Drop` yourself; the field
drop is what releases, and adding a `Drop` that removes the PID file would
introduce a race (a successor may already have created and locked its own).
**Leave the PID file on disk at exit.** A stale file is harmless: the payload is
diagnostic only, and § 2.1 of the design doc is explicit that nothing branches on
it.

`AcquireError` implements `std::fmt::Display`:

- `Held { pid: Some(p) }` → `another daemon is already running (PID {p}) — stop it with: daemoneye stop`
- `Held { pid: None }` → `another daemon is already running — stop it with: daemoneye stop`
- `Io(e)` → `could not acquire the instance lock: {e}`

Implement `std::error::Error` for it so `anyhow` can wrap it via `?`.

### 3. `pid_path()` helper in `src/config/load.rs`

Add next to `default_socket_path()` (line 58), matching its doc-comment style:

```rust
/// Default path for the instance lock / PID file:
/// `~/.daemoneye/var/run/daemoneye.pid`.
pub fn default_pid_path() -> PathBuf {
    var_run_dir().join("daemoneye.pid")
}
```

Re-export it wherever `default_socket_path` is re-exported so
`crate::config::default_pid_path()` resolves.

### 4. Acquire the lock before every side effect

In `run_daemon` (`src/daemon/mod.rs`), insert the acquisition **immediately
after** the `log_file` `dup2` block ends (currently line 369, just before
`Config::load()` at 372) and before everything else:

```rust
    let instance = match instance::InstanceLock::acquire(&crate::config::default_pid_path()) {
        Ok(lock) => lock,
        Err(e) => {
            log::error!("{e}");
            anyhow::bail!("{e}");
        }
    };
```

Placement rationale, and it is load-bearing: it must come *after* the log
redirect so the failure lands in `daemon.log` rather than a terminal that may not
exist, and *before* `Config::load()` so that every item in the § 2.3 table —
memory migration, pipe-log deletion, `daemon_start`, the four global
`set-hook -g` calls, per-session hooks, and all three supervisor spawns — is
downstream of it.

Bind `instance` to a named variable, not `_`. It must stay alive until the end of
`run_daemon`; a `_` binding drops it immediately and releases the lock.

Add `let _ = &instance;` immediately before the `Ok(())` at the end of
`run_daemon` **only if** the compiler warns the binding is unused. It will not —
the value is live across the accept loop by virtue of not being dropped — so do
not add it speculatively.

### 5. Delete the `daemon_is_running()` guard from the startup path

Remove the whole `if daemon_is_running().await { … }` block at
`src/daemon/mod.rs:791-797` (`grep -n 'if daemon_is_running()' src/daemon/mod.rs`).
The lock from task 4 replaces it.

**Do not delete the `daemon_is_running()` function itself.** Phase 09 reshapes
it.

This leaves it with zero call sites, which is fine and will **not** trip
`-D warnings`: `src/lib.rs:10` has `pub mod daemon;`, so `pub async fn
daemon_is_running` is reachable from the library root and `dead_code` does not
fire on it. Do not add an `#[allow(dead_code)]`, and do not "fix" the absence of
callers by inventing one.

### 6. License the socket unlink on ownership

Replace the comment above the `symlink_metadata` match (`mod.rs:799-800`) so the
invariant is stated, and leave the matching logic itself unchanged:

```rust
    // The instance lock is held, so no other daemon is alive: any socket file at
    // this path is definitionally stale and safe to remove. symlink_metadata()
    // (does not follow symlinks) so a symlink at the socket path removes the
    // symlink itself rather than its target (S3).
```

### 7. Identity-checked teardown

Immediately after the successful `UnixListener::bind` (`mod.rs:809`), record the
bound socket's identity:

```rust
    let socket_id = socket_path
        .symlink_metadata()
        .ok()
        .map(|m| (std::os::unix::fs::MetadataExt::dev(&m), std::os::unix::fs::MetadataExt::ino(&m)));
```

Then at the shutdown unlink (`mod.rs:857`, under the `── Graceful shutdown ──`
banner), replace
`let _ = std::fs::remove_file(&socket_path);` with a version that only removes
the socket it actually bound:

```rust
    // Only unlink the socket this daemon bound. If the identity differs, another
    // process replaced the path and removing it would strip a successor's address.
    let current_id = socket_path
        .symlink_metadata()
        .ok()
        .map(|m| (std::os::unix::fs::MetadataExt::dev(&m), std::os::unix::fs::MetadataExt::ino(&m)));
    if socket_id.is_some() && current_id == socket_id {
        let _ = std::fs::remove_file(&socket_path);
    } else {
        log::warn!(
            "socket at {} is not the one this daemon bound — leaving it in place",
            socket_path.display()
        );
    }
```

The `tmux set-hook -gu` teardown loop that follows (`mod.rs:859`, four hook names)
stays exactly as it is. Reaching shutdown now implies this process owned the
instance, because task 4 bails before any hook is installed.

**Note the loop's current shape**, which changed after this phase was drafted:
phase 06h wrapped it in `crate::tmux::off_runtime("set-hook-unset", …)` with an
`.unwrap_or_else(|| Err(std::io::Error::other("timed out uninstalling hook")))`
collapse. **That wrapper is correct and pre-existing — leave it alone.** Do not
remove it, and do not treat it as something this phase introduced.

### 8. Document the invariant in `CLAUDE.md`

Add one bullet to the `## Important Invariants` list:

```markdown
- Exactly one daemon may run per `$HOME`, enforced by an exclusive `flock` on
  `~/.daemoneye/var/run/daemoneye.pid` acquired in `run_daemon` before any
  startup side effect (`src/daemon/instance.rs`). The kernel releases it on
  process death, so there is no stale-lock recovery path. The PID written into
  the file is diagnostic payload only — never branch on it. Holding the lock is
  what authorizes unlinking a socket at `default_socket_path()`.
```

Also add the `src/daemon/instance.rs` row to the "Key files" table:

```markdown
| `src/daemon/instance.rs` | `InstanceLock` — flock-based single-instance enforcement + PID payload |
```

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes: the **921** existing lib-unit tests plus the 6 new
      ones from the Test plan = **927**, and the **27** integration tests.
      **⚠ Baseline refreshed 2026-07-29 before dispatch** — the phase was drafted
      against 914; phase 06s then added 5 `bounded_output` tests and 05g/05h added
      others. Verify the starting point with
      `cargo test 2>&1 | grep "^test result" | head -3` (expect 921 / 0 / 27) and
      **report a blocker if it is not 921** rather than adjusting the target.
- [ ] `grep -c "daemon_is_running" src/daemon/mod.rs` returns **1** — the
      function definition survives (line 292) and its only call, inside
      `run_daemon`, is gone. It reads **2** before this phase.
- [ ] `grep -rn "unsafe" src/daemon/instance.rs` returns nothing.
- [ ] Starting a second daemon against a running first fails, and the running
      daemon keeps working (End-to-end verification below).

## Test plan

All tests in `src/daemon/instance.rs` under `#[cfg(test)] mod tests`. Use
`tempfile::TempDir` for the PID path — these tests must **not** touch
`$HOME`, and because they never call `config_dir()` they do not need
`crate::TEST_HOME_LOCK`.

- `acquire_writes_own_pid` — after `acquire`, the file's contents parse to
  `std::process::id()`.
- `second_acquire_is_held_with_pid` — with a first lock alive, a second
  `acquire` on the same path returns `AcquireError::Held { pid: Some(p) }` where
  `p == std::process::id()`. (Same-process `flock` on a *different* open file
  description does contend, so this works in one test binary — that is a
  property of `flock`, not an accident. Do not spawn a subprocess.)
- `acquire_succeeds_after_drop` — drop the first lock, then `acquire` again
  succeeds.
- `held_error_reports_none_for_unparsable_payload` — pre-write `"garbage"` to
  the path, take a lock in-process by hand, and assert the contended
  `AcquireError::Held` carries `pid: None`.
- `failed_acquire_preserves_incumbent_payload` — the failing path must not
  truncate: with a first lock alive (PID written), a second failed `acquire`
  leaves the file's contents still parsing to the first PID. This is what pins
  the "no `truncate(true)` on open" requirement from task 2 — a mutation that
  adds `.truncate(true)` must fail this test.
- `read_pid_returns_none_for_missing_file` — `read_pid` on a nonexistent path is
  `None`.

## End-to-end verification

Unit tests cannot show that a duplicate daemon fails to damage a live one. Verify
against the real binary and quote the actual output in the Update Log.

```bash
cargo build --release

# 1. Start a daemon and confirm it owns the lock.
./target/release/daemoneye daemon
sleep 3
cat ~/.daemoneye/var/run/daemoneye.pid          # expect the running daemon's PID
./target/release/daemoneye ping                  # expect "Daemon is running."

# 2. Attempt a duplicate in the foreground so the error is visible.
./target/release/daemoneye daemon --console
# expect a non-zero exit and:
#   another daemon is already running (PID <n>) — stop it with: daemoneye stop

# 3. The first daemon must be UNHARMED — this is the whole point of the phase.
./target/release/daemoneye ping                  # expect "Daemon is running."
ls -la ~/.daemoneye/var/run/daemoneye.sock       # socket still present
tmux show-hooks -g | grep -c pane-died           # expect 1, hooks intact

# 4. Kernel-release check: SIGKILL leaves no stale lock.
kill -9 "$(cat ~/.daemoneye/var/run/daemoneye.pid)"
sleep 1
./target/release/daemoneye daemon                # must start cleanly, no manual cleanup
sleep 3
./target/release/daemoneye ping                  # expect "Daemon is running."
./target/release/daemoneye stop
```

Step 3 is the acceptance test for the incident in `daemon-instance.md` § 1: under
the old code the duplicate's startup deleted the live daemon's pipe logs and
repointed its tmux hooks before failing. Step 4 is the property that motivated
`flock` over a bare PID file (§ 2.1).

Quote the real output of steps 2, 3, and 4. If step 3's `ping` reports anything
other than "Daemon is running.", the phase is **not** done.

## Authorizations

- [x] May modify `Cargo.toml` — solely to add `features = ["fs"]` to the
      existing `nix` dependency (task 1). No version bumps, no new
      dependencies, no other keys.
- [x] May add the new file `src/daemon/instance.rs` (STANDARDS § 2.2 prefers
      editing; a new module is required here because instance ownership is not a
      concern of any existing module).
- [x] May edit `CLAUDE.md` § "Important Invariants" and § "Key files" (task 8).
- [ ] `unsafe` is **not** authorized. If you believe you need it, you have not
      enabled the `nix` `fs` feature — re-read task 1.

## Out of scope

- **Do not touch `daemon_is_running()`'s body or signature.** Removing its
  *call site* is task 5; reshaping the function is phase 09.
- **Do not make the webhook bind fatal.** It is the other duplicate-instance
  signal (`daemon-instance.md` § 4.2) and it is phase 09's task. A duplicate
  daemon now exits before reaching the webhook spawn anyway.
- **Do not add a PID to `log_event`.** Phase 10.
- **Do not change the fork or how the parent reports success.** Phase 11. A
  duplicate `daemoneye daemon` (without `--console`) will still print
  `daemoneye daemon started (PID n)` and exit 0 while the child dies — this is
  known, it is why phase 11 exists, and it is why the E2E steps above use
  `--console` for the duplicate attempt. Do not fix it here.
- **Do not add file locking to `schedules.json`, the memory index, or session
  JSONL** (`daemon-instance.md` § 3). Single-instance enforcement is the fix.
- **Do not remove the PID file on shutdown.** Task 2 explains the race.
- **Do not touch the 04x lock-conversion work.** This phase is independent of
  it; `sessions.lock()` sites are not yours.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-29 21:30 (started)

**Executor:** Claude (sonnet)

Started phase 08: Instance Lock. Implementing `InstanceLock` module, acquiring
the lock before startup side effects, removing `daemon_is_running()` guard,
identity-checked socket teardown, and CLAUDE.md documentation.
