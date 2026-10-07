# Phase 05c: pause-deadline — time spent paused does not count against a command's deadline

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-05b
**Estimated diff:** ~120 lines (about 25 code, 95 tests)
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Q6, decided at scope sign-off: **a pause suspends the command's deadline
clock**, as prompt time already does. Today, if an operator pauses a
command with `Ctrl-p` in attached mode, the paused time still counts. A ghost
command with a 300 s limit that is paused for 5 minutes is reported as timed
out the moment it is resumed. The fix lives in the one place every command's
deadline lives, `read_to_marker` in `src/shell/client.rs`. It covers host
shells and sandbox shells alike: since 05b, a sandbox shell's `Status` reports
`Paused` from inside the VM.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Decisions on the open
  questions", Q6.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`read_to_marker` (`src/shell/client.rs`) waits for the command's end marker in
slices of at most `EXIT_POLL_INTERVAL` (500 ms). When a slice times out with no
output it checks the deadline **first**, then asks the host whether the shell
has exited:

```rust
let mut deadline = tokio::time::Instant::now() + timeout;
…
loop {
    let slice_end = match drain_until {
        Some(end) => end,
        None => deadline.min(tokio::time::Instant::now() + EXIT_POLL_INTERVAL),
    };
    let read = tokio::time::timeout_at(slice_end, reader.read_until(b'\n', &mut line)).await;
    let n = match read {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => { … }
        Err(_) => {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out after {timeout:?} waiting for command output: {cmd:?}"
                );
            }
            if let Some(end) = drain_until {
                if tokio::time::Instant::now() >= end {
                    return Err(ShellExited { cmd: cmd.to_string() }.into());
                }
            } else if let Ok(ShellResponse::Status {
                state: ShellState::Exited,
                ..
            }) = status(sock, EXIT_STATUS_TIMEOUT).await
            {
                drain_until = Some(tokio::time::Instant::now() + EXIT_DRAIN_WINDOW);
            }
            continue;
        }
    };
```

**The idiom to copy is in the same function.** Time spent waiting on a prompt
answer is already added back to the deadline:

```rust
let asked = tokio::time::Instant::now();
let reply = on_chunk(&seen).await;
deadline += asked.elapsed();
```

A paused shell prints nothing, so a paused command sits in the `Err(_)`
(slice timed out) arm, and every slice ends with the same `status` call. That
call is where the pause is visible: `ShellResponse::Status { state:
ShellState::Paused, .. }`.

### Test fixture to copy (`src/shell/client.rs` tests)

```rust
async fn spawn_host() -> Host {
    let _home = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    let cast = dir.path().join("e2e.cast");
    let backend =
        PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0).expect("bash available");
    let path = dir.path().join("shell.sock");
    let listener = host::bind(&path).await.expect("bind the shell socket");
    tokio::spawn(host::serve(listener, Arc::clone(&backend)));
    Host { path, _dir: dir }
}
```

`send_signal(&host.path, ShellSignal::Stop, Duration::from_secs(5))` and the
`Cont` form already work against this host (the tests near line 1049 use
them), and its `Status` reports `Paused` while a job is stopped.

## Spec

### Task 1 — Count a paused slice out of the deadline

In `read_to_marker`, restructure only the `Err(_) =>` arm (the slice
timed out), so that:

1. Record when the slice began. Add `let slice_started = tokio::time::Instant::now();`
   immediately **before** the `timeout_at(slice_end, …)` call.
2. In the `Err(_)` arm, do the **status check first**, then the deadline
   check:
   - **draining** (`drain_until` is `Some`): unchanged; past `end` →
     `ShellExited`.
   - **not draining**: call `status(sock, EXIT_STATUS_TIMEOUT).await` once and
     match it:
     - `Ok(Status { state: Exited, .. })` → set `drain_until` as today;
     - `Ok(Status { state: Paused, .. })` →
       `deadline += slice_started.elapsed();`, the time this slice spent
       paused, the same way prompt time is added back;
     - anything else → nothing.
   - **then** `if tokio::time::Instant::now() >= deadline { bail!(… timed out …) }`,
     with the message unchanged;
   - then `continue`.

Moving the deadline check after the status check matters. A command paused
across its deadline must have the paused slice added back **before** it is
judged.

Add a one-sentence comment on the `Paused` branch, in the voice of the
prompt-time comment: paused time is not the command's run time, so it is
added back to the deadline.

Change nothing else in the function: not the `Chunk` arm, not the prompt
handling, not `EXIT_POLL_INTERVAL`.

### Task 2 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `cargo test --lib client_run_command_pause_does_not_count_against_the_deadline`
      reports `1 passed; 0 failed`.
- [ ] `cargo test --lib client_run_command_still_times_out_when_not_paused`
      reports `1 passed; 0 failed`.
- [ ] `cargo test --lib shell::client` reports `0 failed`, and no existing
      test there is edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

In `src/shell/client.rs` tests, next to the shell-exit tests:

- `client_run_command_pause_does_not_count_against_the_deadline`:
  1. `let host = spawn_host().await;`
  2. Start `run_command(&host.path, "sleep 1; echo PAUSED_OK", "bash", Duration::from_secs(2))`
     in a `tokio::spawn`, so it runs concurrently. Clone `host.path` into the
     task.
  3. `tokio::time::sleep(Duration::from_millis(300)).await;` then
     `send_signal(&host.path, ShellSignal::Stop, Duration::from_secs(5)).await.expect(…)`.
  4. Sleep **3 s**: longer than the 2 s limit, all of it paused.
  5. `send_signal(&host.path, ShellSignal::Cont, …)`.
  6. Await the task. The command **succeeds** and its output contains
     `PAUSED_OK`. Without the fix, it times out.
- `client_run_command_still_times_out_when_not_paused` (the negative case):
  `run_command(&host.path, "sleep 5", "bash", Duration::from_secs(1))` with no
  pause → `Err` whose message contains `timed out`, returned in **under 3 s**.
  The extension must apply only while paused.

Use real time (`#[tokio::test]`, not `start_paused`): the host is a real PTY.

**Reviewer mutation:** delete the `deadline += slice_started.elapsed();` line,
and the pause test must fail with `timed out`.

## End-to-end verification

```sh
cargo test --lib client_run_command_pause_does_not_count_against_the_deadline 2>&1 | grep -E '^test result'
cargo test --lib client_run_command_still_times_out_when_not_paused 2>&1 | grep -E '^test result'
cargo test --lib shell::client 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.

## Authorizations

None. The change is inside `read_to_marker` and its tests.

## Out of scope

- The run-level pause bookkeeping (`src/daemon/run/pause.rs`): unchanged.
- How often a quiet command polls `Status`. For a sandbox shell each poll is
  an in-VM `FIND`; that cost predates this phase and stays as it is.
- Any other timeout (prompt timeouts, ghost turn budgets).

## Update Log

<!-- entries appended below this line -->
