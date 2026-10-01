# Phase 03c: host-exits-with-its-shell — a shell host stops when its shell ends, and `close_shell` really closes

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** 03b (done)
**Estimated diff:** ~110 lines of production code across 7 files, plus 4 doc lines; 4 new tests and 1 tightened
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Two pre-existing defects, both measured live on a real detached
`daemoneye shell-host` at commit `c9c10e56`:

1. **A host outlives its shell.** After `exit` is typed, the host process
   stays alive and keeps its instance lock. The registry's reaper decides
   liveness from that lock (`reap_exited` → `host_is_alive`), so the shell is
   **never** marked exited: `ShellList` would show it live forever. A
   subscriber gets one last `Chunk`, then nothing: no EOF and no `Exited`
   frame.
2. **`close_shell` cannot close an idle local shell.** It sends `TERM`
   through the host's `Signal` path, which goes to the foreground group. An
   interactive bash at its prompt ignores `TERM`. Three seconds later the
   host still reported `state: "Idle"` and was alive.

After this phase:
- a host serves only until its shell has exited, then returns, so the process
  exits and releases its lock;
- a new host request, `Close`, sends `SIGHUP` to the shell's **pid**, which
  ends bash **and** its foreground job (measured);
- `close_shell` sends `Close`, falling back to `TERM` for an older host that
  refuses the frame.

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md` — row 03c (inserted
  at drafting, PE decision 2026-10-01). The `ShellTail` relay moves to 03d and
  can rely on EOF for exit.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.
4. **Never run `kill`, `pkill` or `killall` yourself, and never send a signal
   to a process this phase's tests did not start.** The tests do all the
   signalling. Sending a signal by hand to the wrong pid has crashed this
   machine before.

## Current state

Measured on **2026-10-01** at commit `c9c10e56`. Baseline:
`cargo test --lib` → **2110 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the five tests (four new, one tightened) passed on 6 of 6 runs;
- the full suite was lib 2114/0/14 on three runs, with every other target
  unmoved;
- clippy `-D warnings` was clean;
- Mutations A, B and C each went red on 6 of 6 runs;
- through the real binary, a host exited (and its subscriber saw EOF) both
  after a typed `exit` and after `Close` with `sleep 300` in the foreground.

### Measured facts

- **`PtyShell` keeps its own clone of the stream sender**
  (`src/shell/pty.rs`, the `stream_tx` field). So the output broadcast never
  closes while the shell object lives, and the backend's pump never sees
  `RecvError::Closed`. "The pump ended" is **not** an exit signal. What
  works is `backend.status()`, which already reports
  `ShellState::Exited` once the child is gone.
- **`SIGHUP` to the shell's pid** ended an idle bash. With `sleep 300` in
  the foreground, it ended bash and the `sleep` child too (state `Exited`
  within 2 s; the child's `/proc` entry gone).
- **`TERM` via `Signal`** goes to the foreground group
  (`backend.signal` → `signal_foreground`), and an idle interactive bash
  ignores it.
- **An unknown host frame** gets `ShellResponse::Error { message:
  "malformed frame: …" }` and the connection stays open. So an adopted host
  from an older binary refuses `Close` cleanly, and the client can fall back.

### The codebase's signal idiom (`src/shell/pty.rs:440-451`)

Signals are sent by running `kill`, with no `unsafe`:

```rust
    fn signal_group(&self, pgid: i64, sig: &str) -> anyhow::Result<()> {
        if pgid > 0 {
            std::process::Command::new("kill")
                .arg("-s")
                .arg(sig)
                .arg("--")
                .arg(format!("-{pgid}"))
                .status()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context(format!("kill -s {sig} -- -{pgid} failed"))?;
        }
        Ok(())
    }
```

### Where the host serves forever (`src/shell/launch.rs:102-103`)

```rust
    let listener = crate::shell::host::bind(&socket_path(id)).await?;
    crate::shell::host::serve(listener, backend).await
```

### `close_shell` today (`src/daemon/executor/knowledge/shell.rs:1981-2025`)

```rust
    let socket = socket_path(shell);
    if let Err(e) =
        crate::shell::send_signal(&socket, crate::shell::ShellSignal::Term, signal_timeout).await
    {
        return format!( … could not be signalled … );
    }
    // The signal is one-way; what closes the shell is the host's PTY ending.
    // Give it a bounded moment and observe, rather than asserting either way.
    let lock = crate::shell::lock_path(shell);
    closed_by_observed_end(shell, &lock)
}
```

`closed_by_observed_end` (`:2006`) polls `host_is_alive(lock)` for up to
1.5 s, using **`std::thread::sleep`**. That is a blocking sleep inside an
async caller. **Gotcha, measured:** on a `#[tokio::test]`, which uses a
current-thread runtime, that blocking sleep starves the in-process host task,
so it never notices the exit and never drops its lock. The tightened test
then fails on every run at ~1.5 s. It must become an `async fn` using
`tokio::time::sleep`.

### The in-process test host (`src/daemon/executor/knowledge/shell.rs:2048-2064`)

`lifecycle_test_support::spawn_with_cast` serves forever and leaks the lock
(`std::mem::forget(lock)`), so a closed shell never looks closed in tests:

```rust
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
```

The existing test `close_shell_ends_the_callers_shell` (`:3741`) accepts
**either** outcome. Its comment says "an idle bash ignores it, so the honest
outcome for a live host is the 'still finishing' branch". That comment
documents defect 2.

## Spec

### Task 1 — protocol (`src/shell/proto.rs`)

Add `Close` to `ShellRequest`, after `Status`:

```rust
    /// Hang up the shell itself (SIGHUP to its pid), ending it and its jobs.
    Close,
```

### Task 2 — hang up the shell (`src/shell/pty.rs`)

Add to `impl PtyShell`, just above `signal_group`:

```rust
    /// Hang up the shell itself: SIGHUP to its pid, not to the foreground
    /// group. An interactive bash exits on it and hangs up its own jobs.
    pub fn hangup(&self) -> anyhow::Result<()> {
        let pid = self
            .pid()
            .ok_or_else(|| anyhow::anyhow!("the shell has no pid to hang up"))?;
        let status = std::process::Command::new("kill")
            .arg("-s")
            .arg("HUP")
            .arg("--")
            .arg(pid.to_string())
            .status()
            .context("running kill -s HUP")?;
        if !status.success() {
            anyhow::bail!("kill -s HUP -- {pid} failed: {status}");
        }
        Ok(())
    }
```

### Task 3 — the host (`src/shell/host.rs`, `src/shell/backend.rs`, `src/shell/launch.rs`, `src/shell/mod.rs`)

1. In `host.rs`, add `ShellState` to the `use crate::shell::proto::{…}` line
   (`:15`).
2. In the `ShellBackend` trait, after `subscribe_with_screen`, add a method
   with a default body, so the four test fakes stay unchanged:

   ```rust
       /// End the shell itself. A backend that cannot answers an error, which
       /// the host relays as an `Error` frame.
       async fn close(&self) -> anyhow::Result<()> {
           anyhow::bail!("this shell host cannot close its shell")
       }
   ```

3. In `handle_line`, after the `ShellRequest::Status` arm:

   ```rust
        ShellRequest::Close => match backend.close().await {
            Ok(()) => ShellResponse::Ok,
            Err(e) => ShellResponse::Error {
                message: e.to_string(),
            },
        },
   ```

4. In `host.rs`, add above `pub async fn serve`:

   ```rust
   /// `serve`, but return once the shell has exited: every `poll`, ask the
   /// backend's status and stop when it reads `Exited`. A host process that
   /// returns from this exits, which releases its instance lock.
   pub async fn serve_until_exit<B: ShellBackend>(
       listener: UnixListener,
       backend: Arc<B>,
       poll: std::time::Duration,
   ) -> anyhow::Result<()> {
       let watched = Arc::clone(&backend);
       tokio::select! {
           res = serve(listener, backend) => res,
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
       }
   }
   ```

5. In `impl ShellBackend for PtyBackend` (`backend.rs`), after
   `subscribe_with_screen`:

   ```rust
       async fn close(&self) -> anyhow::Result<()> {
           self.shell.lock().unwrap_or_log().hangup()
       }
   ```

6. In `launch.rs`, replace `crate::shell::host::serve(listener, backend).await`
   at the end of `run_host` with:

   ```rust
       crate::shell::host::serve_until_exit(listener, backend, SHELL_EXIT_POLL).await?;
       log::info!("shell host {id}: the shell exited; stopping");
       Ok(())
   }

   /// How often a host checks whether its shell has exited.
   const SHELL_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(250);
   ```

7. In `src/shell/mod.rs`, add `serve_until_exit` to
   `pub use host::{ShellBackend, bind, serve};` and `close_or_term` (Task 4)
   to the `pub use client::{…}` list.

### Task 4 — close, with a fallback (`src/shell/client.rs`)

Add above `/// The shell's current state, rows, cols and pid.`. It has the
`send_signal` shape (`client.rs:63-78`):

```rust
/// Ask the host to end its shell (`ShellRequest::Close`). A host from before
/// `Close` refuses the frame, so fall back to `Signal { TERM }`, the only
/// closure it understands.
pub async fn close_or_term(sock: &Path, timeout: Duration) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let wire = encode(ShellRequest::Close).context("encoding Close frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Close frame")?;
    writer.flush().await.context("flushing Close frame")?;
    match read_ack(sock, &mut reader, timeout).await {
        Ok(()) => Ok(()),
        Err(_) => send_signal(sock, ShellSignal::Term, timeout).await,
    }
}
```

### Task 5 — `close_shell` (`src/daemon/executor/knowledge/shell.rs`)

1. In `close_shell_with` (`:1981`), replace the `send_signal(…Term…)` call
   (keep its error message unchanged):

   ```rust
       if let Err(e) = crate::shell::close_or_term(&socket, signal_timeout).await {
   ```

2. Make `closed_by_observed_end` an **`async fn`**. Replace its
   `std::thread::sleep(std::time::Duration::from_millis(50));` with
   `tokio::time::sleep(std::time::Duration::from_millis(50)).await;`, and
   call it as `closed_by_observed_end(shell, &lock).await`.
3. Its still-finishing message no longer names TERM or the missing kill
   frame:

   ```rust
                   "Asked shell {shell} to close, but its host is still holding its \
                    lock — the shell may still be finishing. list_shells still shows \
                    it; check again before assuming it is closed."
   ```

4. In `lifecycle_test_support::spawn_with_cast` (`:2048`), serve like a real
   host and release the lock when the shell exits. Replace the
   `tokio::spawn(crate::shell::serve(…))` and `std::mem::forget(lock);` lines
   with:

   ```rust
        // Like a real host: serve until the shell exits, then release the lock.
        tokio::spawn(async move {
            let _ = crate::shell::serve_until_exit(
                listener,
                backend,
                std::time::Duration::from_millis(100),
            )
            .await;
            drop(lock);
        });
   ```

   Keep `std::mem::forget(dir);`. The module's `use std::sync::Arc;` then
   becomes unused; remove it, or clippy `-D warnings` fails.

### Task 6 — docs: say what `close_shell` now does

Four places say "terminate signal". Reword each to say that `close_shell`
**hangs the shell up**: SIGHUP to the shell, which also ends a command
running in it.

- `CLAUDE.md:155`: the `close_shell` row → "End one of this session's PTY
  shells by id: hangs the shell up (SIGHUP, which also ends a command running
  in it) and reports whether the host actually went away, refusing another
  session's or a ghost job's shell".
- `README.md:518`: the `close_shell` row → "End this session's PTY shell by
  id — the id from `list_shells`. Hangs the shell up (ending any command
  running in it) and reports whether the shell's host actually went away;
  another session's or a ghost job's shell is refused".
- `assets/prompts/sre.toml:190`: "The tool sends a terminate signal and then
  reports" → "The tool hangs the shell up (ending any command running in it)
  and then reports".
- `src/ai/tools/defs.rs:886-893`: the `close_shell` description → "End one
  of your PTY shells by id: hang it up (SIGHUP, which also ends a command
  running in it) and report whether its host actually went away. Uses the
  shell id from list_shells (e.g. \"s3\"). Refuses a shell this session does
  not own — another session's or a ghost job's shell is not yours to end. A
  host that has not gone away within a moment is reported as still finishing.
  Use close_shell to free a per-owner shell slot or end a shell you no longer
  need."

### Task 7 — tests

Four new tests and one tightened, names pinned.

1. `src/shell/client.rs`, in `mod oneshot` beside
   `client_send_signal_round_trips`, **`close_falls_back_to_term_on_a_host_without_close`**:
   - `start_server(ShellResponse::Ok)`; its `SignalBackend` uses the trait's
     default `close`, so it refuses;
   - `close_or_term(&server.path, 5 s)` is `Ok`;
   - `server.backend.calls() == vec![Call::Signal(ShellSignal::Term)]`.
2. `src/shell/host.rs`, beside `host_answers_subscribe_screen_with_the_screen_first`,
   **`host_answers_close_with_an_error_when_the_backend_cannot`**: write
   `ShellRequest::Close` to a `start_server()` host. The response is a
   `ShellResponse::Error` whose message contains `"cannot close"`.
3. `src/shell/backend.rs`, **`close_hangs_up_a_shell_and_its_foreground_job`**,
   a `#[test]` in the style of the other backend tests:
   - `spawn_backend(&dir)`;
   - `backend.input(b"sleep 300\n")`;
   - `status_until(&backend, ShellState::Running)` is `Running`;
   - `backend.close()` is `Ok`;
   - `status_until(&backend, ShellState::Exited)` is `Exited`.
4. `src/shell/backend.rs`, **`serve_until_exit_returns_when_the_shell_exits`**:
   - `spawn_backend(&dir)`;
   - bind `dir.path().join("s.sock")` with `crate::shell::bind`;
   - `tokio::spawn(crate::shell::serve_until_exit(listener, Arc::clone(&backend), Duration::from_millis(50)))`;
   - `backend.input(b"exit\n")`;
   - `tokio::time::timeout(Duration::from_secs(10), handle)` gives `Ok(Ok(Ok(())))`.
5. **Tighten** `close_shell_ends_the_callers_shell`
   (`knowledge/shell.rs:3741`). Replace everything from the comment `// The
   signal is one-way and an idle bash ignores it, …` down to (not including)
   `// Either way the registry record stays: …` with **one**
   `close_shell_with(&reg, &id, &Owner::Chat("closer".into()), Duration::from_millis(3000)).await`
   call, then two assertions:
   - the output contains `format!("Closed shell {id}")`;
   - `!host_is_alive(&crate::shell::lock_path(&id))`.

   Keep the trailing record-stays assertion.

**Test count:** four new, so **2114 passed / 14 ignored**.

### Task 8 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-03c.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the three mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today, and each was run at drafting.

- [ ] `cargo test --lib` → **2114 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2110/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The five pinned tests pass:
      ``cargo test --lib -- close_falls_back_to_term_on_a_host_without_close host_answers_close_with_an_error_when_the_backend_cannot close_hangs_up_a_shell_and_its_foreground_job serve_until_exit_returns_when_the_shell_exits close_shell_ends_the_callers_shell``
      → **5 passed** (current: 1 passed, the old lenient
      `close_shell_ends_the_callers_shell`).
- [ ] **A real host exits with its shell.** The end-to-end block's
      `live_host_exit=` line is **`0`** (current: **`124`**: the host
      lingered until `timeout 10` killed it).
- [ ] No doc still promises a terminate signal:
      ``cat CLAUDE.md README.md assets/prompts/sre.toml src/ai/tools/defs.rs | grep -c 'terminate signal'``
      → **0** (current: 4).
- [ ] Mutations A, B and C were applied, run, and their observed results
      recorded **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's contents,
      and the paste-check line is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run once, restore, and **record the
observed result**. Each was measured red on 6 of 6 runs at drafting.

- **Mutation A:** in `serve_until_exit`, delete the `break;` line. The loop
  then never ends. Run test 4. Expected **red** (the 10 s timeout fires).
- **Mutation B:** make `PtyBackend::close` return `Ok(())` without calling
  `hangup()`. Run test 3. Expected **red**.
- **Mutation C:** in `close_or_term`, replace the fallback arm with
  `Err(e) => Err(e),`. Run test 1. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Five tests:
- the fallback to TERM on a host without `Close`;
- the host relaying a backend's refusal;
- `close` ending a shell with a foreground job;
- `serve_until_exit` returning when the shell exits;
- `close_shell` really closing an idle shell, through the tool's own function.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo build -q
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- close_falls_back_to_term_on_a_host_without_close host_answers_close_with_an_error_when_the_backend_cannot close_hangs_up_a_shell_and_its_foreground_job serve_until_exit_returns_when_the_shell_exits close_shell_ends_the_callers_shell 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'terminate_signal_docs=%s\n' "$(cat CLAUDE.md README.md assets/prompts/sre.toml src/ai/tools/defs.rs | grep -c 'terminate signal')"
  H=$(mktemp -d /tmp/de03c.XXXX)
  ( HOME=$H timeout 10 target/debug/daemoneye shell-host --id s8 --shell bash >/dev/null 2>&1; echo "$?" > $H/rc ) &
  for i in $(seq 1 50); do [ -S $H/.daemoneye/var/run/shells/s8.sock ] && break; sleep 0.1; done
  python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.sendall(b"{\"type\":\"Input\",\"bytes\":[101,120,105,116,10]}\n"); s.recv(64)' $H/.daemoneye/var/run/shells/s8.sock
  wait
  printf 'live_host_exit=%s\n' "$(cat $H/rc)"
  find "$H" -depth -delete
} > /tmp/e2e-m23-03c.txt
cat /tmp/e2e-m23-03c.txt
```

The live part starts a real detached shell host under `timeout 10` in a
throwaway HOME, types `exit` into it (bytes `e x i t \n`), and records the
host's exit code: `0` means it stopped by itself; `124` means `timeout` had
to kill it. The `/tmp/de03c.XXXX` path is short on purpose: a Unix socket
path over 108 bytes fails to bind.

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2114p-0f-14i`;
- `new_tests=5p-0f`;
- `isolation=11 passed; 0 failed`;
- `terminate_signal_docs=0`;
- `live_host_exit=0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** A concurrent run
of the PTY tests makes them fail spuriously. Then make the entry's first
fenced block the **contents of** `/tmp/e2e-m23-03c.txt` (copied, not retyped;
**never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-03c-host-exits-with-its-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-03c.txt
diff /tmp/pasted-m23-03c.txt /tmp/e2e-m23-03c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-03c.txt /tmp/e2e-m23-03c.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/proto.rs`: Task 1.
- `src/shell/pty.rs`: Task 2.
- `src/shell/host.rs`, `src/shell/backend.rs`, `src/shell/launch.rs` and
  `src/shell/mod.rs`: Task 3, and tests 2–4.
- `src/shell/client.rs`: Task 4 and test 1.
- `src/daemon/executor/knowledge/shell.rs`: Task 5 and test 5.
- `CLAUDE.md`, `README.md`, `assets/prompts/sre.toml` and
  `src/ai/tools/defs.rs`: only the four `close_shell` texts in Task 6.
- Any file the compiler names for a non-exhaustive match on
  `ShellRequest::Close`: add the arm only.
- This phase doc's Update Log.

## Out of scope

- **The `ShellTail` relay:** row 03d.
- **Lagged subscribers** (the host drops `Lagged` chunks silently): 03d or
  later.
- **Hosts already running from an older binary.** They keep today's
  behaviour until they end. `close_or_term` covers closing them.
- **`run_terminal_command`'s own shells**: unchanged. They exit through the
  same host path and simply stop lingering.

## Update Log

<!-- entries appended below this line -->
