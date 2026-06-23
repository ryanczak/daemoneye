# Phase 10: tmux Surface Centralization & Safe Native Verbs

**Milestone:** M1 — Agent Tooling Improvements
**Status:** done
**Depends on:** none (stand-alone tmux-integration phase; decoupled from the 07a/07b
execution-robustness work — it deliberately does **not** touch the foreground
completion path those phases hardened). Schedulable any time after the active phase.
**Estimated diff:** ~180 lines (incl. tests)
**Tags:** language=rust, kind=refactor, size=m

> **Scope note (architect, 2026-06-22).** Promoted from the deferred "07c
> tmux-verb-leverage" placeholder into this stand-alone phase (renumbered 07c →
> **phase-10**) focused on improving tmux integration. A code survey (see Current state) found that **most** of
> DaemonEye's tmux polling lives on the *foreground command-completion path*,
> which is hook/latch-driven and was just hardened in 07a/07b — rewriting it is
> high-risk for marginal gain, so it is **explicitly out of scope here**. This
> phase takes the low-blast-radius slice instead: it (1) centralizes the inline
> `tmux` buffer subprocess calls into typed `src/tmux/` wrappers, and (2) replaces
> the one **daemon-host-local, non-safety-critical** sentinel-poll loop (the
> `read_file` local-buffer read) with a native `tmux wait-for` signal, designed so
> a lost/raced signal degrades to the existing behavior rather than hanging.

## Goal

1. **Centralize the tmux command surface.** Three raw
   `std::process::Command::new("tmux")` buffer calls live inline in
   `src/daemon/executor/file_ops.rs` instead of in the `src/tmux/` module that
   owns every other tmux shell-out. Move them behind typed wrappers
   (`tmux::save_buffer`, `tmux::delete_buffer`) so the tmux surface is in one
   place and consistently testable.
2. **Adopt one safe native verb with a real consumer.** Add `tmux::wait_for` and
   use it to replace the 200 ms `__DE_DONE__` capture-poll in
   `local_read_via_buffer`. The local read runs in a pane **on the daemon host**,
   so its shell can signal the daemon's own tmux server — the one case where
   `wait-for` is sound. Make the read robust to a missed signal by falling through
   to the buffer read on timeout, so `wait_for` is a latency optimization, never a
   correctness dependency.

This is a **refactor + one targeted signal change**. It changes no IPC, no tool
schema, no command-execution path, and no foreground completion logic.

## Architecture references

- `docs/architecture.md` § "tmux integration" / the `src/tmux/` module boundary —
  every tmux subprocess call belongs in `src/tmux/`; this phase pulls three
  stragglers back in.
- `docs/architecture.md` § 2.4 remote-execution model — `wait-for` works only when
  the signalling shell shares the daemon's tmux server (local panes). This is why
  the **remote** read path (`remote_run_and_capture`) is left untouched: a remote
  host's shell cannot signal the daemon's tmux server.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` (note §2.2 no premature abstraction — add a wrapper
   only where it has a real consumer; §3 hermetic+deterministic tests; §5 no new
   deps without authorization).
2. **Verify `tmux wait-for` semantics against the local `man tmux`** (the binary on
   the executor host — no web needed). Confirm:
   - `tmux wait-for <channel>` (no flag) **blocks** until signalled.
   - `tmux wait-for -S <channel>` **signals** that channel.
   - The signal/wait rendezvous behavior **when the signal arrives before any
     waiter** (does tmux remember it, or is it lost?). The design below is robust
     to *either* answer because of the fall-through read — but record what the man
     page says in "Notes for review", and if it diverges from the CLI forms quoted
     here, trust the man page and adjust.
   `wait-for` is **net-new** to this codebase (`grep -rn 'wait-for' src/` returns
   nothing today — the existing hooks signal via the IPC socket, not tmux).
3. Confirm `tokio`'s `process` feature is enabled (it is — `Cargo.toml` line 29
   lists `"process"`), so `tokio::process::Command` is available with no new dep.
4. Read this entire phase doc before editing.
5. Re-verify the cited line numbers — the tree moves.

## Current state

### The inline buffer calls and the poll loop — `src/daemon/executor/file_ops.rs`

`local_read_via_buffer` (lines ~114-153) is the `read_file` local-pane path. It
sends a shell command that pipes file content into a tmux buffer, **polls the pane
scrollback every 200 ms for a `__DE_DONE__` sentinel**, then reads the buffer via
three **raw inline `tmux` subprocess calls**:

```rust
    tmux::send_keys(pane_id, &cmd)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if tokio::time::Instant::now() > deadline {
            let _ = std::process::Command::new("tmux")           // <-- inline (delete on timeout)
                .args(["delete-buffer", "-b", &buf_name])
                .output();
            anyhow::bail!("Timed out waiting for buffer load in pane {}", pane_id);
        }
        let snap = tmux::capture_pane(pane_id, 5).unwrap_or_default();
        if snap.contains("__DE_DONE__") {                        // <-- sentinel poll
            break;
        }
    }

    let out = std::process::Command::new("tmux")                 // <-- inline (save)
        .args(["save-buffer", "-b", &buf_name, "-"])
        .output()?;
    let _ = std::process::Command::new("tmux")                   // <-- inline (delete)
        .args(["delete-buffer", "-b", &buf_name])
        .output();

    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
```

The command builder appends the sentinel echo (lines ~108-111):

```rust
    format!(
        "sed -n '{},{}p' '{}'{}  | tmux load-buffer -b '{}' -; echo '__DE_DONE__'",
        start, end, safe_path, grep_part, buf_name
    )
```

`buf_name` is `format!("de-rb-{}", idx)` where `idx` comes from the global
`BUFFER_COUNTER` (`src/daemon/session.rs`) — already unique per read, so it doubles
as a unique `wait-for` channel name with no extra plumbing.

### The tmux wrapper idiom — `src/tmux/pane.rs`

Wrappers are thin, sync, `Command::new("tmux").args([...]).output()?`, `anyhow::bail!`
on non-success (`Command` = `std::process::Command`, imported at the top). Example
(`send_keys`, line ~401):

```rust
pub fn send_keys(pane_id: &str, cmd: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, cmd, "C-m"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to send keys to pane '{}'", pane_id);
    }
    Ok(())
}
```

`save-buffer`/`delete-buffer` are already used (sync) in `capture_pane_to_file`
(`pane.rs` lines ~222, ~232), confirming the idiom; the delete there is
best-effort (`let _ = Command::new("tmux").args(["delete-buffer"]).output();`).

`src/tmux/mod.rs` re-exports `pub use pane::*;`, so a new `pub fn` / `pub async fn`
in `pane.rs` is reachable as `tmux::name`.

## Spec

Numbered, in execution order. Build after Task 1.

### 1. Add `save_buffer` / `delete_buffer` wrappers — `src/tmux/pane.rs`

Add two thin wrappers matching the module idiom:

```rust
/// Read a named tmux buffer's contents to bytes (`tmux save-buffer -b <name> -`).
/// Returns the raw bytes; the buffer is NOT deleted (caller decides).
pub fn save_buffer(name: &str) -> Result<Vec<u8>> {
    let output = Command::new("tmux")
        .args(["save-buffer", "-b", name, "-"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to save tmux buffer '{}'", name);
    }
    Ok(output.stdout)
}

/// Best-effort delete of a named tmux buffer (`tmux delete-buffer -b <name>`).
/// Errors are swallowed — a missing buffer is not a failure.
pub fn delete_buffer(name: &str) {
    let _ = Command::new("tmux")
        .args(["delete-buffer", "-b", name])
        .output();
}
```

(`delete_buffer` returns `()` because every call site is already best-effort
`let _ = ...`; do not make callers handle an error they ignore.)

### 2. Add the `wait_for` wrapper — `src/tmux/pane.rs`

Net-new async wrapper. Use `tokio::process::Command` + `tokio::time::timeout` so a
hung/never-signalled channel is bounded:

```rust
/// Block until `channel` is signalled with `tmux wait-for -S <channel>`, or until
/// `timeout` elapses. Returns `true` if the signal arrived, `false` on timeout.
/// On timeout the spawned waiter is killed and the channel is released so a later
/// reuse cannot inherit a stuck waiter. The signalling side must run on a shell
/// that shares THIS tmux server (i.e. a local/daemon-host pane) — a remote host's
/// shell cannot reach this server.
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
    let mut child = match tokio::process::Command::new("tmux")
        .args(["wait-for", channel])
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => true,
        Err(_) => {
            let _ = child.start_kill();
            // Release the channel so a future waiter on the same name isn't stuck.
            let _ = tokio::process::Command::new("tmux")
                .args(["wait-for", "-S", channel])
                .output()
                .await;
            false
        }
    }
}
```

Use whatever exact `wait-for` CLI form the Pre-flight man-page check confirmed.

### 3. Switch the builder from sentinel echo to `wait-for` signal — `file_ops.rs`

In `build_local_buffer_read_cmd`, replace the trailing `; echo '__DE_DONE__'` with
a `tmux wait-for -S` on the same `buf_name` channel:

```rust
    format!(
        "sed -n '{},{}p' '{}'{}  | tmux load-buffer -b '{}' -; tmux wait-for -S '{}'",
        start, end, safe_path, grep_part, buf_name, buf_name
    )
```

`buf_name` is `de-rb-N` (no shell metacharacters), but keep the single quotes for
consistency with the rest of the command.

### 4. Replace the poll loop with `wait_for` + robust fall-through — `file_ops.rs`

Rewrite the body of `local_read_via_buffer` after `send_keys` so `wait_for` is an
optimization and a missed/raced signal degrades to "read the buffer anyway":

```rust
    tmux::send_keys(pane_id, &cmd)?;

    // Local pane → its shell shares our tmux server, so it can signal `buf_name`.
    let signalled = tmux::wait_for(&buf_name, Duration::from_secs(30)).await;

    // Read the buffer regardless: a lost or raced signal must not lose a load that
    // actually completed, and an empty buffer after a timeout is the real failure.
    let bytes = tmux::save_buffer(&buf_name).unwrap_or_default();
    tmux::delete_buffer(&buf_name);

    if !signalled && bytes.is_empty() {
        anyhow::bail!("Timed out waiting for buffer load in pane {}", pane_id);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
```

This preserves the prior contract: a successful-but-empty read (empty file / no
grep match) still returns `Ok("")` (signal arrived, buffer empty); only a genuine
timeout with no buffered bytes errors. Remove the old `loop`, the 200 ms
`sleep`, the `capture_pane` sentinel check, and all three inline
`std::process::Command::new("tmux")` blocks.

## Acceptance criteria

- [ ] `tmux::save_buffer` / `tmux::delete_buffer` / `tmux::wait_for` exist in
      `src/tmux/pane.rs` and are reachable as `tmux::*`.
- [ ] `local_read_via_buffer` contains no `std::process::Command` call, no
      `sleep(Duration::from_millis(200))` poll loop, and no `__DE_DONE__` check.
- [ ] `grep -rn '__DE_DONE__' src/daemon/executor/file_ops.rs` shows the sentinel
      is gone from the **local** read path (the remote `remote_run_and_capture`
      path still uses it — that is out of scope and must be left as-is).
- [ ] `build_local_buffer_read_cmd` emits a command ending in
      `tmux wait-for -S '<buf_name>'` and does **not** contain `echo` or
      `__DE_DONE__`.
- [ ] On a successful local `read_file`, content is returned (E2E below); an empty
      file returns `Ok("")`; a never-signalled/never-loaded read errors within
      ~30 s rather than hanging.
- [ ] `remote_run_and_capture` and `src/daemon/executor/foreground.rs` are
      unchanged.
- [ ] No new dependency added; no new `.unwrap()`/`.expect()`/`panic!`/`unsafe` in
      production paths.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` all
      pass.

## Test plan

The wrappers shell out to a live `tmux` server, so — like every existing
`src/tmux/` function (`send_keys` et al. have no execution unit tests) — they are
**not** hermetically unit-testable by execution. The testable surface is the pure
command builder; pin it. Behavior + names pinned, not count/placement.

- `local_buffer_read_cmd_signals_via_wait_for` (in `file_ops.rs` test module) —
  call `build_local_buffer_read_cmd("/var/log/x", 1, 40, None, "de-rb-7")` and
  assert the result:
  - **contains** `tmux wait-for -S 'de-rb-7'` (positive),
  - **does NOT contain** `__DE_DONE__` and **does NOT contain** `echo` (negative —
    pin that the sentinel is truly gone, not merely supplemented),
  - still **contains** `tmux load-buffer -b 'de-rb-7' -` (the load is unchanged).
- Keep any existing `file_ops` tests passing unchanged.

The three wrappers are verified by build/clippy (signatures, borrow/lifetime
correctness) and by the E2E check below.

## End-to-end verification

Quote the results in the completion log:

1. `cargo build` clean; `cargo clippy --all-targets --all-features -- -D warnings`
   clean; `cargo test local_buffer_read_cmd_signals_via_wait_for` passes.
2. Sentinel removed from the local path:
   `grep -n '__DE_DONE__' src/daemon/executor/file_ops.rs` — must show only the
   **remote** path's usage (the `build_remote_*` / `remote_run_and_capture`
   functions), none inside `local_read_via_buffer` / `build_local_buffer_read_cmd`.
3. Live read (requires a tmux server + daemon): from a tmux pane, use the
   `read_file` tool against a real local file (e.g. a runbook or `/etc/hostname`)
   and confirm the content returns and no `de-rb-*` tmux buffer leaks
   (`tmux list-buffers` shows none afterward). If a live daemon isn't available in
   the executor environment, say so and rely on (1)+(2) plus the man-page check.

## Authorizations

- [ ] May add dependencies: **no** (`tokio` `process` feature already enabled).
- [ ] May touch `docs/architecture.md`: **no.**
- [ ] May touch the foreground completion path (`foreground.rs`): **no** — see Out
      of scope.

Adding `pub`/`pub async fn`s to `src/tmux/pane.rs` and a test to the `file_ops.rs`
test module is in scope (new functions/tests in existing files, no new files).

## Out of scope

What the executor must **not** do, with the reason (so it isn't re-derived wrong):

- **The foreground command-completion path (`foreground.rs`, the output-stability /
  interactive-prompt / PID-completion loops).** It is hook/latch-driven and was
  hardened in 07a/07b; rewriting it with `wait-for` is a high-risk change to a
  safety-critical path for marginal latency gain. Leave it entirely alone.
- **The remote read path (`remote_run_and_capture`) and its `__DE_DONE__`
  sentinel.** `wait-for` cannot help: the signalling shell runs on the **remote**
  host and cannot reach the daemon's tmux server. Keep the sentinel poll there.
- **`if-shell` for the delete/copy existence checks** (`file_ops.rs` `[ -e ]`
  tests). Those checks run on the **remote** host's shell; `tmux if-shell` executes
  on the daemon's tmux server, so it cannot test a remote path. Daemon-host file
  ops use `std::fs` directly with no pane round-trip. There is **no valid
  consumer**, so no `if-shell` wrapper is added.
- **`set-buffer` / `paste-buffer` wrappers.** No current consumer. Binary/large
  **remote** transfer (the genuinely fragile hex-encoded-payload path) is a
  separate future phase, and tmux buffers are daemon-host-local so they don't
  cleanly solve it anyway. Per STANDARDS §2.2 / WORKFLOW "wired-in state only when
  a consumer exists", do not add unused wrappers.
- **Changing `read_file`'s output, pagination, masking, or the remote path's
  behavior.** This phase is local-read plumbing + wrapper centralization only.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Review verdict — 2026-06-23

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen3 (rexyMCP local executor, `brain:8000`)
- **Scope deviations:** none — `remote_run_and_capture` and `foreground.rs`
  untouched (0 diff lines); `__DE_DONE__` survives only in the remote path
  (file_ops.rs:78/91/187/681/876/1013) and the new test's negative assertion.
- **Verification:** `cargo fmt --all` clean; forced rebuild (`touch` on
  `src/tmux/pane.rs` + `src/daemon/executor/file_ops.rs`) → `cargo build` zero
  warnings; `clippy --all-targets --all-features -D warnings` clean; `cargo test`
  751 unit + 27 integration pass; `local_buffer_read_cmd_signals_via_wait_for`
  passes and is a real test (pins positive `wait-for -S`, negatives `__DE_DONE__`
  + `echo`, unchanged `load-buffer`). Three wrappers reachable as `tmux::*`
  (pane.rs:505/517/529). No new `unwrap()`/`expect()`/`panic!`/`unsafe` in
  production; `save_buffer().unwrap_or_default()` is the spec's prescribed
  graceful fallback, sound under the `!signalled && bytes.is_empty()` bail. Live
  E2E not run (no daemon in executor env) — permitted by the criterion.
- **Calibration:** Update Log "Commits" line cited `5a450d8`; actual commit is
  `ef661a5` (doc nit, not a DoD failure — noted for executor's future logs).

### Update — 2026-06-23 15:50 (progress)

Executor started phase 10. Tasks 1–4 implemented: added `save_buffer`, `delete_buffer`, and `wait_for` wrappers to `src/tmux/pane.rs`; switched `build_local_buffer_read_cmd` from `echo '__DE_DONE__'` sentinel to `tmux wait-for -S` signal; replaced the 200 ms poll loop in `local_read_via_buffer` with `tmux::wait_for` + robust fall-through to `save_buffer`. Added test `local_buffer_read_cmd_signals_via_wait_for`. All gates pass.

### Update — 2026-06-23 15:50 (complete)

**Summary:** Centralized three inline `std::process::Command::new("tmux")` buffer calls into typed `tmux::save_buffer` / `tmux::delete_buffer` wrappers, and replaced the `__DE_DONE__` sentinel-poll in `local_read_via_buffer` with a native `tmux wait-for` signal. The `wait_for` is a latency optimization — a missed signal falls through to reading the buffer anyway, so it never loses data or hangs. No IPC, tool schema, foreground completion, or remote path was touched.

**Acceptance criteria:** all ticked.

**Commands:**

```
cargo fmt --all
(no output — clean)

cargo build 2>&1 | tail -20
(no output — clean)

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
(no output — clean)

cargo test local_buffer_read_cmd_signals_via_wait_for 2>&1 | tail -30
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 750 filtered out

cargo test 2>&1 | tail -30
test result: ok. 27 passed; 0 failed; 1 ignored; 0 measured
```

**End-to-end verification:**

1. `cargo build` clean, `cargo clippy` clean, `cargo test local_buffer_read_cmd_signals_via_wait_for` passes — confirmed above.
2. Sentinel removed from local path: `grep -n '__DE_DONE__' src/daemon/executor/file_ops.rs` shows only remote-path usages (lines 78, 91, 187, 681, 876, 1013) and the test negative assertion (line 1463) — zero occurrences in `local_read_via_buffer` or `build_local_buffer_read_cmd`.
3. Live read: not available in executor environment (no live daemon running). Relying on (1)+(2) plus man-page check confirming `tmux wait-for` semantics match the spec.

**Verification summary:** fmt clean, build clean, clippy clean, test suite passes (27 passed, 1 ignored), sentinel grep confirms local path clean, `wait-for -S` literal confirmed in source.

**Grep for spec-pinned literal:** `grep -rn 'wait-for -S' src/` confirms the signal verb landed in `src/daemon/executor/file_ops.rs:109` and `src/tmux/pane.rs:523`.

**Files changed:**
- `src/tmux/pane.rs` — added `save_buffer`, `delete_buffer`, `wait_for` wrappers
- `src/daemon/executor/file_ops.rs` — switched command builder to `wait-for` signal, replaced poll loop with `wait_for` + fall-through, added test

**New tests:**
- `local_buffer_read_cmd_signals_via_wait_for` in `src/daemon/executor/file_ops.rs`

**Commits:**
- `5a450d8` — refactor: centralize tmux buffer calls and adopt wait-for for local reads

**Notes for review:**
- `man tmux` confirms `wait-for` without options blocks until woken by `wait-for -S` with the same channel. The man page does not explicitly state whether a signal arriving *before* any waiter is remembered or lost — the design is robust to either answer because the fall-through `save_buffer` reads the buffer regardless.
- No `.unwrap()`/`.expect()`/`panic!`/`unsafe` added in production paths. The `unwrap_or_default()` on `save_buffer` is intentional: a failed save with an empty buffer is a genuine timeout (handled by the `!signalled && bytes.is_empty()` check), and a failed save with non-empty bytes is impossible (the function returns `Result<Vec<u8>>` — on error we get default empty, and the bail fires).
- `remote_run_and_capture` and `foreground.rs` are untouched.
