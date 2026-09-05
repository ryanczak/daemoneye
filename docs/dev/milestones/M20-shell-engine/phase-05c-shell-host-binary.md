# Phase 05c: the `daemoneye shell-host` binary, detached spawn and readiness

**Milestone:** M20 — Shell Engine
**Status:** in-progress
**Depends on:** phase-05a (`bind`, `serve`, `ShellBackend`) and phase-05b
(`PtyBackend`), both done.
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Turn 05b's in-process backend into a **real detached process**: a
`daemoneye shell-host --id sN` subcommand that owns one PTY and serves it on
`var/run/shells/sN.sock`, plus the spawn-and-wait-for-ready helper the daemon
will call in phase-06.

Three things must be true when this phase lands:

1. `daemoneye shell-host --id s1` binds the socket and serves the live shell.
2. A caller can spawn that host **detached** — it survives the caller's exit —
   and know when it is actually ready to answer.
3. A second host for the same id is **refused**, not silently allowed to
   unlink the first one's socket.

Point 3 is the carry recorded at the 05a close ("who may unlink a live
socket"). It is settled here.

## Architecture references

- `docs/design/daemoneye-2.0.md` § the shell-host process model.
- `docs/dev/milestones/M20-shell-engine/README.md` § Notes — the 05b/05c split
  and the `process_group(0)` finding.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib shell::` must report the phase counts below before you
  start. If any differs, stop and say so — the tree is not what this doc
  was written against.

## Current state

- `src/shell/` holds `mod.rs`, `pty.rs`, `log.rs`, `screen.rs`, `proto.rs`,
  `host.rs`, `backend.rs`. Test counts today: `pty` **13**, `log` **12**,
  `screen` **11**, `host` **8**, `proto` **5**, `backend` **7**.
- `src/shell/launch.rs` **does not exist**. `cargo test --lib shell::launch::`
  matches **0** tests today.
- `src/main.rs` has no `ShellHost` variant and no `Commands::ShellHost` arm.

The three APIs this phase composes already exist and are **not** to be changed:

```rust
// src/shell/backend.rs:44
pub fn spawn(
    shell: &str,
    rows: u16,
    cols: u16,
    scrollback: usize,
    cast: &Path,
    started_unix: u64,
) -> anyhow::Result<Arc<Self>>

// src/shell/host.rs:34 — removes a stale file, binds, chmods 0700
pub async fn bind(path: &Path) -> anyhow::Result<UnixListener>

// src/shell/host.rs:50 — accepts forever, one task per connection
pub async fn serve<B: ShellBackend>(listener: UnixListener, backend: Arc<B>) -> anyhow::Result<()>
```

`src/shell/mod.rs` re-exports selectively; add `mod launch;` and a `pub use`
line beside the existing ones.

## Measured facts — executed 2026-09-04, not reasoned about

Every claim below was run on scrappy while drafting. Probe sources are in
`M20-shell-engine/probes/`.

**F1 — `process_group(0)` gives the child its own group, and no `unsafe`.**
`std::os::unix::process::CommandExt::process_group` is safe and stable.
Measured, spawning the same child twice:

```
with .process_group(0):     child_pid=336681  child_pgid=336681   <- its own group
without (control):          child_pid=336754  child_pgid=336753   <- parent's group
```

**F2 — the child outlives the parent.** With the parent exiting immediately
after `spawn()` and never waiting, the child kept writing its heartbeat file
and was reparented to init:

```
child_pid=336646 child_pgid=336646
beat 0  ppid_now=1372
...
beat 13 ppid_now=1372        <- parent long gone
```

**F3 — THE LOAD-BEARING ONE: `connect()` succeeding does NOT mean the host is
serving.** A Unix socket that is bound and `listen()`ing but never `accept()`ed
still completes a client `connect()` **and accepts a write**, because the
connection sits in the listen backlog:

```
connect+send succeeded without accept(), 0.01ms
reply: TIMEOUT — connect alone is NOT proof the host is serving
```

So readiness **must** be a full `Status` round-trip. A readiness check that
polls for the socket file, or that stops at a successful `connect()`, will
report ready against a half-started host and the first real request will hang.
This is pinned as a negative test below.

**F4 — the directories already exist.** Any subcommand's `Config::ensure_dirs()`
creates both, mode 0700. Measured under a throwaway `HOME`:

```
drwx------ .../.daemoneye/var/run/shells
drwx------ .../.daemoneye/var/log/shells
```

So `bind()` will find its parent directory. Do **not** add `create_dir_all`
calls for these.

**F5 — `std::env::current_exe()` returns the real binary path**, so the spawn
re-invokes this same binary rather than searching `$PATH`.

**F6 — the `Status` wire frames** (from `src/shell/proto.rs` tests, unchanged):

```
request:  {"type":"Status"}
response: {"type":"Status","state":"Running","rows":40,"cols":120,"pid":1234}
```

`encode()` appends the trailing newline; frames are newline-delimited.

## Spec

### Task 1 — `src/shell/launch.rs`, the paths and the id guard

```rust
pub fn validate_shell_id(id: &str) -> anyhow::Result<()>
pub fn socket_path(id: &str) -> PathBuf   // shells_dir().join(format!("{id}.sock"))
pub fn lock_path(id: &str) -> PathBuf     // shells_dir().join(format!("{id}.lock"))
pub fn cast_path(id: &str, started_unix: u64) -> PathBuf
                                          // shell_logs_dir().join(format!("{id}-{started_unix}.cast"))
```

`validate_shell_id` accepts **exactly** `s` followed by 1–9 ASCII digits —
`s0`, `s1`, `s42`. Everything else is an error. The id lands in a filename, so
this is a confinement boundary, not cosmetics. Follow the shape already in
`src/scripts.rs:129`:

```rust
fn validate_script_name(name: &str) -> Result<()> {
    if name.is_empty() { bail!("Script name cannot be empty"); }
    if name == "." || name == ".." { bail!("Invalid script name: '{}'", name); }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
        bail!("Invalid script name: '{}'", name);
    }
    Ok(())
}
```

**Must-NOT-accept, all pinned as test cases:** `""`, `"."`, `".."`,
`"../etc/passwd"`, `"s1/../x"`, `"s1/x"`, `"s"`, `"s-1"`, `"S1"`, `"shell"`,
`"s1.sock"`, `"s1 "`, `"s1234567890"` (ten digits).
**Must accept:** `"s0"`, `"s1"`, `"s42"`, `"s123456789"` (nine digits).

Use `config::shells_dir()` and `config::shell_logs_dir()` — do not rebuild
those paths from `config_dir()`.

### Task 2 — `run_host`, the in-process host

```rust
pub async fn run_host(id: &str, shell: &str, rows: u16, cols: u16) -> anyhow::Result<()>
```

In this order, and the order matters:

1. `validate_shell_id(id)?`
2. **Acquire the instance lock** on `lock_path(id)` and hold it for the whole
   function. A second host for the same id must fail here, before it can
   touch the socket.
3. `PtyBackend::spawn(shell, rows, cols, scrollback, &cast_path(id, now), now)`
   where `scrollback` is `Config::load()`'s `shells.scrollback_lines` as
   `usize` and `now` is the current unix time in seconds.
4. `let listener = host::bind(&socket_path(id)).await?;`
5. `host::serve(listener, backend).await`

**Reuse `InstanceLock` — do not write a second locking scheme.** It already
exists, is reviewed, and is generic over the path
(`src/daemon/instance.rs:65`):

```rust
impl InstanceLock {
    pub fn acquire(path: &Path) -> Result<Self, AcquireError> {
        let file = OpenOptions::new()
            .create(true).truncate(false).read(true).write(true)
            .open(path).map_err(AcquireError::Io)?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(guard) => { /* writes its pid as diagnostic payload */ }
            Err((back, Errno::EWOULDBLOCK)) => Err(AcquireError::Held { pid: read_pid_from_file(&back) }),
            Err((_, errno)) => Err(AcquireError::Io(std::io::Error::from(errno))),
        }
    }
}
```

Reach it as `crate::daemon::instance::InstanceLock` (`pub mod daemon`,
`pub mod instance` — it is already reachable, no visibility change needed).

**Why the lock is step 2 and not step 4.** `host::bind` deliberately unlinks
whatever sits at the path before binding — its doc comment says "the caller
owns the id, so a leftover file at `path` is by definition stale." That is only
true if ownership of the id is established first. The lock is what makes the
caller the owner. Without it, a second `shell-host --id s1` would unlink a live
host's socket and leave that host running, invisible, still holding a PTY.
Mirror the daemon's invariant, stated in `CLAUDE.md`: *"Holding the lock is
what authorizes unlinking a socket."*

The error on a contended lock must name the id, e.g.
`shell s1 is already hosted (PID 1234)`.

### Task 3 — `await_ready`, a full round-trip

```rust
pub async fn await_ready(sock: &Path, deadline: std::time::Instant) -> anyhow::Result<()>
```

Loop until `deadline`:

- connect to `sock`; on any error sleep ~20 ms and retry.
- write `encode(ShellRequest::Status)?` bytes.
- read one newline-delimited line **with a short per-attempt timeout**
  (~200 ms, via `tokio::time::timeout`) so a backlogged connection cannot
  block the loop past its deadline.
- parse it; succeed **only** on `ShellResponse::Status { .. }`. Anything else —
  timeout, EOF, `Error`, a parse failure — is a retry, not a success.

On deadline, return an error naming the socket path.

Per F3 this is not a stylistic choice: connect-and-stop reports ready against a
bound-but-unserved socket.

### Task 4 — `spawn_detached`

```rust
pub async fn spawn_detached(id: &str, shell: &str, rows: u16, cols: u16) -> anyhow::Result<()>
```

1. `validate_shell_id(id)?`
2. Build `std::process::Command::new(std::env::current_exe()?)` with args
   `["shell-host", "--id", id, "--shell", shell, "--rows", &rows.to_string(),
   "--cols", &cols.to_string()]`.
3. `.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())`
4. `.process_group(0)` — import `std::os::unix::process::CommandExt`.
5. `spawn()`, and **do not wait on the child**. Drop the handle.
6. `await_ready(&socket_path(id), Instant::now() + Duration::from_secs(10)).await`

### Task 5 — the CLI subcommand

Add to the `Commands` enum in `src/main.rs` (4-space indent, matching
`Ping` at line 52):

```rust
    /// Run a shell host for one PTY-backed shell (internal; normally spawned by the daemon)
    ShellHost {
        /// Shell id, e.g. s1
        #[arg(long, value_name = "ID")]
        id: String,
        /// Shell to run (default: $SHELL, else bash)
        #[arg(long, value_name = "SHELL")]
        shell: Option<String>,
        #[arg(long, default_value_t = 24)]
        rows: u16,
        #[arg(long, default_value_t = 80)]
        cols: u16,
    },
```

and the arm in `async_main`, beside the other simple arms:

```rust
        Commands::ShellHost { id, shell, rows, cols } => {
            let shell = shell
                .or_else(|| std::env::var("SHELL").ok())
                .unwrap_or_else(|| "bash".to_string());
            shell_mod::launch::run_host(&id, &shell, rows, cols).await?;
        }
```

`async_main` already runs under a multi-thread tokio runtime built in `main()`,
so no runtime construction is needed here.

## Acceptance criteria

Each was run against the current tree while drafting and returns the "before"
value shown.

- [ ] `test -f src/shell/launch.rs && echo yes` → **yes** (now: absent).
- [ ] `grep -c '^mod launch;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -cE '^    ShellHost \{' src/main.rs` → **1** (now `0`).
- [ ] `grep -c 'Commands::ShellHost' src/main.rs` → **1** (now `0`).
- [ ] `grep -c 'process_group(0)' src/shell/launch.rs` → **1**.
- [ ] **The lock is reused, not reinvented:**
      `grep -c 'InstanceLock' src/shell/launch.rs` → **1 or more**, and
      `grep -c 'FlockArg\|flock' src/shell/launch.rs` → **0**. A second
      locking scheme is the defect this pair rules out.
- [ ] **No `unsafe` in non-test code:**
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/launch.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'`
      → **0**. Scoped to pre-test code on purpose — the tests mutate `HOME`
      and `std::env::set_var` is `unsafe` in edition 2024. The `^` anchor is
      required or a doc comment mentioning `#[cfg(test)]` ends the scan early.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/launch.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**.
- [ ] `cargo test --lib shell::launch::` reports **8 or more** passing,
      `0 failed` (now: `0 passed; … 1596 filtered out`).
- [ ] Earlier phases unchanged: `shell::pty::` **13**, `shell::log::` **12**,
      `shell::screen::` **11**, `shell::host::` **8**, `shell::proto::` **5**,
      `shell::backend::` **7**.
- [ ] All four gates pass.

### Round 2 — the HOME/PATH race (added at the 2026-09-05 escalation)

**Read this first: your round-1 work is on disk, correct, and approved as it
stands.** `src/shell/launch.rs` (319 lines), the `mod launch;` line, the
`ShellHost` variant and its `async_main` arm are all **done** — all eight
pinned `launch_*` tests exist and pass, and every structural criterion above
already returns its target value. Do **not** rewrite, re-derive or reformat
any of it. Your own progress note was right: what remained was the gates and
the E2E block.

Running those gates found **one defect**, and it is a real one:
`cargo test --lib` fails intermittently — measured **2 failures in 4 runs** —
in a *pre-existing phase-02 test*:

```
thread 'shell::pty::tests::pty_shell_is_usable_after_a_timeout' panicked at src/shell/pty.rs:601:59:
bash available in the test env: spawn_command failed
Caused by:
    No such file or directory (os error 2)
```

**`bash` is not missing — the environment is being torn out from under the
lookup.** The new `launch_*` tests call `std::env::set_var("HOME", …)` while
other threads are inside `PtyShell::spawn`, which resolves `bash` through
`PATH`. glibc's `setenv` may reallocate `environ`, so a concurrent `getenv`
can read a torn pointer and return nothing — the spawn then reports
"No such file or directory". This is exactly why `std::env::set_var` is
`unsafe` in edition 2024, and it is **not** a fault in your code: the spec told
you to take `test_home_guard()` and you did. The guard only serializes tests
that *take* it, and the `shell::pty::` tests read the environment without
taking it. That gap was the architect's.

Confirmed by measurement, not inference:

```
single-threaded (--test-threads=1):  2 runs, 2 clean
parallel (default):                  4 runs, 1 FAILED (2 tests)
```

**The fix, verified by the architect before prescribing it.** Add the guard to
the four `shell::pty::` tests that spawn, so they participate in the same
mutex. Each of the four sites currently reads:

```rust
        let mut shell = PtyShell::spawn("bash", (24, 80)).expect("bash available in the test env");
```

and becomes:

```rust
        let _home = crate::test_home_guard();
        let mut shell = PtyShell::spawn("bash", (24, 80)).expect("bash available in the test env");
```

Measured with that change applied: `cargo test --lib shell::` **5 runs, 5
clean**, and the full `cargo test --lib` **3 runs, 3 clean** — with no
slowdown (`shell::` stayed at 3.3–3.5 s).

- [ ] `grep -c 'let _home = crate::test_home_guard();' src/shell/pty.rs` → **4**
      (now `0`).
- [ ] `cargo test --lib` run **three times in a row**, each reporting
      `0 failed`. Paste all three `test result:` lines. One clean run does not
      settle a race — the bug reproduced at 1-in-4.
- [ ] `cargo test --lib shell::launch::` still reports **8 passed — 8, not 9.**
      This round fixes a race and adds no tests; a rising count is scope creep.
- [ ] `shell::pty::` still **13**, and the other modules unchanged
      (`log` 12, `screen` 11, `host` 8, `proto` 5, `backend` 7).
- [ ] The § End-to-end verification block is run and pasted, with the
      `PASTE MATCH` verdict line — this is the step you had not reached.

## Test plan

Names pinned; placement is not — `#[cfg(test)] mod tests` at the bottom of
`src/shell/launch.rs`. Every name begins `launch_`.

Tests that touch `shells_dir()` mutate `HOME` and **must** take the RAII guard
first. Copy this shape from `src/config/lifecycle.rs:370`:

```rust
#[test]
fn every_existing_directory_has_a_policy_entry() {
    let _guard = test_home_guard();
    let tmp_home = std::env::temp_dir().join(format!("de_lifecycle_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_home).ok();
    unsafe { std::env::set_var("HOME", &tmp_home); }
    Config::ensure_dirs().ok();
    // ...
}
```

Use `crate::test_home_guard()` (`src/lib.rs`), **not** the raw
`TEST_HOME_LOCK` — the accessor recovers from poisoning. Give each test a
distinct temp dir name so they do not collide.

- `launch_validate_shell_id_accepts_canonical_ids` — `s0`, `s1`, `s42`,
  `s123456789` all `is_ok()`.
- `launch_validate_shell_id_rejects_traversal_and_junk` — **the negative
  case.** Every string in the must-NOT-accept list above is `is_err()`.
  Assert each individually so a failure names the input.
- `launch_paths_live_under_the_configured_dirs` — under a temp `HOME`,
  `socket_path("s1")` ends with `var/run/shells/s1.sock`, `lock_path("s1")`
  with `var/run/shells/s1.lock`, and `cast_path("s1", 17)` with
  `var/log/shells/s1-17.cast`.
- `launch_await_ready_rejects_a_bound_but_unserved_socket` — **the F3
  guard, and the most important test here.** Bind a `UnixListener` in a temp
  dir and never accept on it. `await_ready` with a ~1 s deadline must return
  `Err`. A version that stops at a successful `connect()` passes this
  test only by failing it — if this test ever goes green with a
  connect-only implementation, the implementation is wrong.
- `launch_await_ready_succeeds_against_a_status_responder` — spawn a task
  that accepts one connection, reads a line, and writes
  `encode(ShellResponse::Status { state: Running, rows: 24, cols: 80, pid: 1 })`.
  `await_ready` returns `Ok`. No PTY needed.
- `launch_second_host_for_the_same_id_is_refused` — acquire an
  `InstanceLock` on `lock_path("s1")`, hold it, and assert a second
  `InstanceLock::acquire` on the same path returns `Err`. Hermetic, no PTY,
  no process spawn.
- `launch_run_host_serves_status_over_its_socket` — **the integration
  test.** Under a temp `HOME`, run `run_host("s1", "bash", 24, 80)` in a
  spawned task; `await_ready` succeeds; then open a second connection, send
  `{"type":"Status"}`, and assert the reply parses as
  `ShellResponse::Status { state: ShellState::Running, .. }`. Bound every
  wait. This is the one test that needs a real PTY.
- `launch_run_host_rejects_an_invalid_id_before_binding` — `run_host` with
  `"../x"` returns `Err` **and** no file appears in `shells_dir()`. Assert
  the directory is still empty — the point is that validation happens before
  any filesystem effect.

**Two things happen at once in this phase and one test must show it:**
`launch_run_host_serves_status_over_its_socket` has the host serving while a
client asks — that is the pairing. Do not split it into "the host starts" and
"a client can ask" as separate tests; the milestone's standing lesson is that
two tests asserting one thing each do not cover the case where they overlap.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-05c.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::launch::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::launch:: totals =="
cargo test --lib shell::launch:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. earlier phases untouched (13, 12, 11, 8, 5, 7) =="
for m in pty log screen host proto backend; do
  cargo test --lib shell::$m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== F. structural greps (each must print the stated number) =="
echo -n "launch.rs exists         (1): "; test -f src/shell/launch.rs && echo 1 || echo 0
echo -n "mod launch declaration   (1): "; grep -c '^mod launch;' src/shell/mod.rs
echo -n "ShellHost enum variant   (1): "; grep -cE '^    ShellHost \{' src/main.rs
echo -n "Commands::ShellHost arm  (1): "; grep -c 'Commands::ShellHost' src/main.rs
echo -n "process_group(0)         (1): "; grep -c 'process_group(0)' src/shell/launch.rs
echo -n "InstanceLock reused     (>=1): "; grep -c 'InstanceLock' src/shell/launch.rs
echo -n "no second lock scheme    (0): "; grep -c 'FlockArg\|flock' src/shell/launch.rs
echo -n "no unsafe pre-test       (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/launch.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/launch.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
echo "== G. the real binary answers on a real socket =="
H=$(mktemp -d); HOME="$H" ./target/debug/daemoneye ping >/dev/null 2>&1
HOME="$H" ./target/debug/daemoneye shell-host --id s1 >/dev/null 2>&1 &
HP=$!
for i in $(seq 1 50); do
  [ -S "$H/.daemoneye/var/run/shells/s1.sock" ] && break
  sleep 0.2
done
echo -n "socket mode (srwx------): "; stat -c '%A' "$H/.daemoneye/var/run/shells/s1.sock" 2>&1
echo -n "status round-trip: "
S="$H/.daemoneye/var/run/shells/s1.sock" timeout 10 python3 -c '
import socket, os
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(5)
s.connect(os.environ["S"]); s.sendall(b"{\"type\":\"Status\"}\n")
print(s.makefile().readline().strip())' 2>&1 | head -1
echo -n "second host refused (nonzero exit): "
HOME="$H" timeout 10 ./target/debug/daemoneye shell-host --id s1 >/dev/null 2>&1; echo "exit=$?"
kill "$HP" 2>/dev/null; wait "$HP" 2>/dev/null
find "$H" -depth -delete 2>/dev/null
} > /tmp/e2e-05c.txt 2>&1
cat /tmp/e2e-05c.txt
```

Paste the contents of `/tmp/e2e-05c.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-05c-shell-host-binary.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05c.txt
diff /tmp/pasted-05c.txt /tmp/e2e-05c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B through E can each report success with nothing having run.**
Measured on the current tree: `cargo test --lib shell::launch::` prints
`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1596 filtered out`
and exits `0`. A zero exit proves nothing — the pass conditions are the named
lines in B, eight or more in C, and exactly `13`, `12`, `11`, `8`, `5`, `7`
in D.

**Section F on an absent file errors rather than printing `0`** — `grep -c`
against a missing path warns on stderr and exits `2`. The block redirects
`2>&1`, so a warning there is itself proof the file is missing.

**Section G uses `python3`, which is present at `/usr/bin/python3`.**
`socat` is **not installed on this host** — measured while drafting — so do not
reach for it. The pass conditions in G are: a mode line of `srwx------`, a
round-trip line starting `{"type":"Status"`, and a **nonzero** exit from the
second `shell-host` (that is the instance lock refusing it, and a zero there is
a phase failure, not a pass). `find "$H" -depth -delete` is the cleanup form to
use; `rm -rf` is not available to you.

## Authorizations

You may edit, and nothing else:

- **create** `src/shell/launch.rs`
- `src/shell/mod.rs` — add `mod launch;` and one `pub use` line
- `src/main.rs` — the `ShellHost` enum variant and its `async_main` arm
- this phase doc's Update Log

**Forced chains to be aware of:**

- Adding the `ShellHost` variant to `Commands` makes the `match` in
  `async_main` non-exhaustive until you add the arm. Both edits are required
  together; the build will not pass with only one.
- `src/main.rs` refers to the library crate — check how the existing arms
  name it and follow that, rather than assuming a `crate::` path.

**Do not** change `src/shell/host.rs`, `backend.rs`, `log.rs`, `screen.rs` or
`proto.rs`. If you believe one needs a change, stop and say so in the Update
Log instead — that is a spec bug, not your call to make.

**`src/shell/pty.rs` is authorized for ONE test-only edit**, added at the
2026-09-05 escalation and scoped exactly to what § "Round 2" describes: adding
`let _home = crate::test_home_guard();` as the first line of the four `#[test]`
functions that call `PtyShell::spawn("bash", (24, 80))`. Nothing else in that
file may change — no production code, no new tests, no renames.

**Do not run `tmux kill-server`, `kill -- -1`, or any signal to a process group
or to PID -1.** Section G's cleanup kills exactly the one pid it started.

## Out of scope

- The shell **registry** — `ShellId` allocation, per-owner caps, startup
  adoption, dead-socket sweep. That is phase-06. This phase takes the id as an
  argument and never invents one.
- Routing `run_terminal_command` through any of this (phase-07).
- Signals and interactive-command handling beyond what `PtyBackend` already
  does (phase-08).
- Restart survival end to end (phase-09).
- Any change to the `[execution] backend` switch. This subcommand works
  regardless of its value; nothing reads it yet.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-05 05:52 (progress)

Started 05c. Built `src/shell/launch.rs` (validate_shell_id, path helpers,
run_host, await_ready, spawn_detached), wired it into `src/shell/mod.rs` and
added the `ShellHost` CLI subcommand + async_main arm in `src/main.rs`.
`cargo test --lib shell::launch::` reports 8 passed, 0 failed. Structural
greps all match the spec (mod launch = 1, ShellHost variant = 1, arm = 1,
process_group(0) = 1, InstanceLock reused, no flock re-implementation, no
unsafe/no unwrap/expect/panic in pre-test code). Next: full gates and the
end-to-end verification block.

### Update — 2026-09-05 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the phase is all but finished — 319 lines of `launch.rs`, all
eight pinned tests passing, build/clippy/fmt clean — but the gates the executor
never reached expose a real defect it was not authorized to fix, so a plain
resume would carry the same wall forward.

**What the stall actually was.** `NoProgressStall` at 200 consecutive read-only
calls. The executor's own last progress note had it exactly right — *"Next:
full gates and the end-to-end verification block"* — and then it read instead
of running them. Nothing was lost: every file it wrote is on disk and is kept.

**Gates run by the architect against the partial tree**, per the escalate
skill's `NoProgressStall` rule:

```
cargo build                          exit 0
cargo clippy --all-targets --all-features -- -D warnings   exit 0
cargo fmt --all --check              exit 0
cargo test --lib shell::launch::     8 passed, 0 failed   (all 8 pinned names present)
cargo test --lib                     1599 passed, 1 FAILED   <- intermittent
```

**The one defect, and it is the architect's spec gap, not the executor's
error.** The new tests mutate `HOME` while other threads resolve `bash` through
`PATH` inside `PtyShell::spawn`; a concurrent `setenv`/`getenv` race makes the
spawn report "No such file or directory". Measured 2 failures in 4 parallel
runs, 0 in 2 single-threaded runs. The spec told the executor to take
`test_home_guard()` and it did — the guard simply does not cover the
`shell::pty::` tests that read the environment without taking it. § Acceptance
criteria § Round 2 carries the mechanism, the measurements, and the fix, which
the architect ran (5/5 and 3/3 clean, no slowdown) before prescribing it.

**One authorization widened:** `src/shell/pty.rs` was on the do-not-touch list;
it is now open for exactly the four-line test-only guard insertion, and nothing
else. That omission is what would have made a plain resume stall again.

### Update — 2026-09-05 15:38 (progress)

Resume after the 2026-09-05 escalation. Applied the round-2 fix: added
`let _home = crate::test_home_guard();` as the first line of the four
`PtyShell::spawn("bash", (24, 80))` tests in `src/shell/pty.rs` (count = 4).
No other changes — `src/shell/launch.rs`, `mod launch;`, the `ShellHost`
variant and its `async_main` arm are carried forward as-is. Gates and E2E run
below.

### Update — 2026-09-05 15:38 (end-to-end verification)

```
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.85s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::launch::tests::launch_await_ready_rejects_a_bound_but_unserved_socket ... ok
shell::launch::tests::launch_await_ready_succeeds_against_a_status_responder ... ok
shell::launch::tests::launch_paths_live_under_the_configured_dirs ... ok
shell::launch::tests::launch_run_host_rejects_an_invalid_id_before_binding ... ok
shell::launch::tests::launch_run_host_serves_status_over_its_socket ... ok
shell::launch::tests::launch_second_host_for_the_same_id_is_refused ... ok
shell::launch::tests::launch_validate_shell_id_accepts_canonical_ids ... ok
shell::launch::tests::launch_validate_shell_id_rejects_traversal_and_junk ... ok
cargo_exit=0
== C. shell::launch:: totals ==
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1596 filtered out; finished in 1.12s
cargo_exit=0
== D. earlier phases untouched (13, 12, 11, 8, 5, 7) ==
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1591 filtered out; finished in 2.51s
cargo_exit=0
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1592 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1593 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1596 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1599 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1597 filtered out; finished in 3.46s
cargo_exit=0
== E. lib suite totals ==
test result: ok. 1600 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 17.75s
cargo_exit=0
== F. structural greps (each must print the stated number) ==
launch.rs exists         (1): 1
mod launch declaration   (1): 1
ShellHost enum variant   (1): 1
Commands::ShellHost arm  (1): 1
process_group(0)         (1): 1
InstanceLock reused     (>=1): 3
no second lock scheme    (0): 0
no unsafe pre-test       (0): 0
unwrap/expect/panic pre-test (0): 0
== G. the real binary answers on a real socket ==
socket mode (srwx------): srwx------
status round-trip: {"type":"Status","state":"Running","rows":24,"cols":80,"pid":831978}
second host refused (nonzero exit): exit=1
```
