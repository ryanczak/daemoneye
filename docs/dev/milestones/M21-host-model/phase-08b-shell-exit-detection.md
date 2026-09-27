# Phase 08b: shell-exit-detection — a command sent to a dead shell fails in seconds, not at its timeout

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** phase-04a (SSH shells), the M20 shell substrate
**Estimated diff:** ~200 lines across 4 files, plus 7 tests
**Tags:** language=rust, kind=bugfix, size=s

## Goal

When a shell's program has exited — an `ssh` that could not authenticate, a
shell that ran `exit`, a remote connection that dropped — a command sent to
it **fails within a few seconds, saying the shell exited**. Today it waits
out its whole timeout (45 s by default, up to 3600 s) and then reports a
*timeout*, and the model is told the command "may still be running".

This is the substrate the next row needs. Row 08c runs ghost commands on SSH
shells opened with `-o BatchMode=yes`: ssh then refuses to prompt and exits
at once — measured, 17 ms for an unknown host key — and this row is what
turns that exit into a fast, honest failure instead of a silent timeout.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 — remote shells are PTY shells whose
  program is `ssh`; a program that exits leaves the shell host alive and its
  socket answering.
- `docs/dev/milestones/M21-host-model/README.md` § Notes — the probes and PE
  decisions recorded for rows 08b–08e on 2026-09-27.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-27** at commit `3bc2a185`, on a clean tree.

Baseline: `cargo test --lib` → **2037 passed / 0 failed / 8 ignored**.

### 1. The defect, measured through the real client

At drafting a throwaway test hosted a **real `ssh`** in an in-process shell
host (`PtyBackend::spawn("ssh", &args, …)` + `bind` + `serve`) and drove it
with the daemon's real `crate::shell::run_command`, timeout 20 s:

| ssh argv (after `-tt -o ConnectTimeout=8`) | ssh alone | `run_command` result |
|---|---|---|
| `-o BatchMode=yes pinky` (key auth) | exit 0, 378 ms | `Ok(exit=0, "PROBE_42")` in **464 ms** |
| `-o BatchMode=yes -o UserKnownHostsFile=<empty> -o StrictHostKeyChecking=ask pinky` | exit 255 in **17 ms**, `Host key verification failed.` | `Err(timed out after 20s …)` after **20.0 s** |

ssh was gone in 17 ms; the client waited the full 20 s and called it a
timeout. The shell host process stays alive and its socket keeps answering
after its program dies — it just never sends another byte.

### 2. Why: nothing in the run loop ever asks whether the shell is alive

Every command path — `run_command`, `run_command_with_prompts` and
`spawn_command` — reads its output through **one** loop,
`read_to_marker` (`src/shell/client.rs:274`). Its only exits are the end
marker, a closed socket, a malformed frame, or the single deadline:

```rust
    loop {
        line.clear();
        let read = tokio::time::timeout_at(deadline, reader.read_until(b'\n', &mut line)).await;
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                return Err(e).with_context(|| format!("reading frame from {}", sock.display()));
            }
            Err(_) => {
                anyhow::bail!("timed out after {timeout:?} waiting for command output: {cmd:?}");
            }
        };
```

The protocol **already carries the answer**: `ShellResponse::Status { state,
.. }` reports `ShellState::Exited` once the child is dead
(`src/shell/backend.rs:216`, `Ok(false) => ShellState::Exited`, pinned by the
existing backend test at `backend.rs:885-924`). The client function
`crate::shell::status(sock, timeout)` (`client.rs:66`) fetches it over a
**separate** connection. The run loop just never calls it.
`ShellResponse::Exited { code }` exists in `src/shell/proto.rs:55` but **no
code produces or consumes it** — do not build on it; `Status` is the signal.

### 3. The worked example: `watch_shell` already polls `Status`

`src/daemon/executor/knowledge/shell.rs:1292-1308`:

```rust
                match crate::shell::status(&socket_path(shell), SIGNAL_TIMEOUT).await {
                    Ok(crate::shell::ShellResponse::Status { state, .. }) => match state {
                        crate::shell::ShellState::Running => seen_running = true,
                        crate::shell::ShellState::Idle => { /* … */ }
                        crate::shell::ShellState::Paused => return WatchOutcome::Paused,
                        crate::shell::ShellState::Exited => return WatchOutcome::Exited,
                    },
                    Ok(_) => {}
                    Err(_) => return WatchOutcome::Unreachable,
                }
```

Same call, same match. The difference for the run loop: a `status` **error**
must **not** fail a running command (see Task 1) — `watch_shell` can afford
to give up on an unreachable socket; a command mid-run cannot.

### 4. The cancel-safety hazard this fix walks into

To poll status while waiting, the read must wake up periodically — i.e. the
`timeout_at` around `read_until` fires **before** the real deadline, and the
`read_until` future is dropped. tokio 1.53.1 (the locked version) documents
exactly what happens then (`tokio-1.53.1/src/io/util/async_buf_read_ext.rs`,
`read_until`, § Cancel safety — quoted verbatim):

> If used as a branch in `tokio::select!` and another branch completes
> first, then some data may have been partially read. Any partially read
> bytes are appended to `buf`, and the method can be called again to continue
> reading until `byte`.
>
> This method returns the total number of bytes read. If you cancel the call
> to `read_until` and then call it again to continue reading, the counter is
> reset.

So **a slice that times out may leave half a JSON frame in `line`.** The
loop above calls `line.clear()` at the top of every iteration — keep that
and the half-frame is discarded, the next read starts mid-frame, and
`serde_json::from_slice` reports a malformed frame. **`line` must be cleared
only after a complete frame has been parsed**, never after a slice timeout.
And because the counter resets, parse the frame from `line` (the
accumulated bytes), never from the count `n`.

### 5. What the model is told today

Both PTY arms in `src/daemon/executor/mod.rs` (`:506` and `:603`) render a
run error identically:

```rust
                                        let msg = format!(
                                            "Error running command in shell {shell_id}: {e:#}. \
                                             The command may still be running in that shell — \
                                             do not assume it failed outright; retrying could \
                                             double-apply its effect."
                                        );
```

And `run_in_shell_core` (`src/daemon/executor/shell_exec.rs:363` and
`:386`) wraps **every** run error in a timeout-shaped context —
`"command exceeded the {applied_timeout:#?} limit in shell {}"` and
`"sudo in shell {} did not complete within the {applied_timeout:#?} limit"`.
For a dead shell, both are false: nothing exceeded a limit, and nothing is
still running. **No test pins either wording** (checked at drafting:
`grep -rn 'exceeded the' src` finds only these two sites and two unrelated
clamp messages).

### 6. The test fixture to copy

`src/shell/client.rs:468` — a real PTY host on a temp socket:

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

For a shell whose program exits, spawn `"sh"` with `&["-c".to_string(),
"exit 0".to_string()]` instead of `"bash", &[]` — factor a
`spawn_host_running(program, args)` beside it if you like. `PtyBackend::spawn`
takes `args: &[String]`.

### 7. A stale fragment in the file you are editing

`src/daemon/executor/shell_exec.rs:76` is half a sentence left behind by a
WIP commit — *"The non-chat entry point: no transport, so a sudo command that
needs a"* — sitting on top of `run_in_shell_with_prompts`'s real doc comment.
Delete that one line (Task 2).

## Spec

### Task 1 — the run loop notices a dead shell (`src/shell/client.rs`)

1. Add a typed error and export it from `src/shell/mod.rs` beside the other
   `client::` names:

   ```rust
   /// The shell's program exited before this command's end marker arrived.
   #[derive(Debug)]
   pub struct ShellExited {
       pub cmd: String,
   }
   // Display: "shell exited before the command finished: {cmd:?}"
   // plus `impl std::error::Error for ShellExited {}`
   ```

2. In `read_to_marker`, wake up every **`EXIT_POLL_INTERVAL` = 500 ms**
   while waiting: wrap `read_until` in
   `tokio::time::timeout_at(min(deadline, now + EXIT_POLL_INTERVAL), …)`.
   When a slice elapses:
   - If the **real** deadline has passed → the existing
     `"timed out after {timeout:?} waiting for command output: {cmd:?}"`
     error, **byte-identical** (`client_run_command_times_out_on_a_silent_command`
     pins it).
   - Otherwise call `status(sock, EXIT_STATUS_TIMEOUT)` (1 s). On
     `Ok(ShellResponse::Status { state: ShellState::Exited, .. })`: **drain**
     — keep reading and processing frames exactly as the loop does for up to
     **250 ms**, because an end marker may already be in flight — and if no
     outcome was found, return `Err(ShellExited { cmd }.into())`.
     **Any other `status` result, including an `Err`, means keep waiting** —
     a transient status failure must never fail a live command.
3. **Clear `line` only after a complete frame is parsed** (§ Current state
   4). A slice timeout must leave `line` as it is.

`spawn_command` and `run_command_with_prompts` get the fix for free — they
share `read_to_marker`. Change nothing else about framing, prompts or the
write-half lifetime.

### Task 2 — say "exited", not "timed out" (`src/daemon/executor/shell_exec.rs`)

- At both context sites (`:363`, `:386`): when the error's chain contains a
  `ShellExited` — `e.chain().any(|c| c.is::<crate::shell::ShellExited>())` —
  use the context `"shell {id} exited before the command finished"` instead
  of the limit wording. A real timeout keeps today's wording unchanged.
- Delete the orphaned line 76 (§ Current state 7).

### Task 3 — tell the model the truth (`src/daemon/executor/mod.rs`)

Replace the two identical `format!`s at `:506` and `:603` with one helper
beside them:

```rust
fn shell_run_error_text(shell_id: &str, e: &anyhow::Error) -> String
```

- `e`'s chain contains `ShellExited` → `"Error running command in shell
  {shell_id}: {e:#}. That shell has exited, so the command is not running
  there. Omit `shell` (or pass \"new\") to run it in a live shell."`
- Otherwise → today's text, **byte-identical**, including the "may still be
  running … could double-apply" caveat.

### Task 4 — tests

Seven tests. Names pinned; placement as listed.

In `src/shell/client.rs` `mod tests` (real PTY hosts, § Current state 6):

1. `client_run_command_fails_fast_when_the_shell_program_has_exited` — host
   running `sh -c 'exit 0'`; `run_command(…, "echo hi", "sh",
   Duration::from_secs(20))` returns `Err` in **under 5 s**, the error
   downcasts to (or its chain contains) `ShellExited`, and its text contains
   `exited`.
2. `client_run_command_fails_fast_when_the_command_exits_the_shell` — a live
   `bash` host; the command `exit 7`; `Err` in under 5 s containing
   `exited`.
3. `client_run_command_on_a_live_shell_is_not_cut_short_by_the_exit_check`
   — *preservation*: `bash`, `sleep 2; echo LIVE_OK`, timeout 10 s → `Ok`,
   output contains `LIVE_OK`. At 500 ms slices this crosses four status
   polls while the shell is alive.
4. `client_spawn_command_fails_fast_when_the_shell_program_has_exited` — the
   `spawn_command` path over an `sh -c 'exit 0'` host: its outcome resolves
   to an `Err` containing `exited` in under 5 s.

In `src/daemon/executor/shell_exec.rs` tests:

5. `an_exited_shell_is_reported_as_exited_not_as_a_timeout` — through
   `run_in_shell_with_prompts` with a spawn closure whose in-process host runs
   `sh -c 'exit 0'` (copy `spawn_with_cast`, `shell_exec.rs:734`, changing
   only the program and args). `format!("{e:#}")` contains `exited` and does
   **not** contain `exceeded`.

In `src/daemon/executor/mod.rs` tests:

6. `a_dead_shell_error_does_not_say_the_command_may_still_be_running` — pure,
   on `shell_run_error_text`: a `ShellExited` error → text names the shell id,
   contains `exited`, and does **not** contain `may still be running`; a plain
   `anyhow!("timed out after …")` error → text **does** contain `may still be
   running` (the caveat survives for real timeouts).

Live, ignored (the M21 pattern — run by hand in Task 6):

7. `a_batchmode_ssh_shell_that_cannot_connect_fails_fast_on_pinky` in
   `src/shell/client.rs` tests, `#[ignore = "requires the configured host
   \`pinky\` reachable over ssh"]` — § Current state 1's second row as a
   test: host `ssh` with `-tt -o BatchMode=yes -o ConnectTimeout=8 -o
   UserKnownHostsFile=<empty temp file> -o StrictHostKeyChecking=ask pinky`;
   `run_command` → `Err` containing `exited` in under 5 s. Print the elapsed
   time with `println!` so the by-hand run shows it.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 6 — the live check

Run test 7 by hand and paste its output into the same entry, after the
mutations:

```sh
cargo test --lib a_batchmode_ssh_shell_that_cannot_connect_fails_fast_on_pinky -- --ignored --nocapture 2>&1 | grep -E 'elapsed|^test |test result'
```

**If `pinky` is unreachable, report the criterion unmet** — do not describe
what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today — checked at drafting.

- [ ] `cargo test --lib` → **at least 2043 passed / 0 failed / 9 ignored**,
      on three consecutive runs (current: 2037/0/8). 2037 + tests 1–6 =
      2043; test 7 is ignored (8 → 9). A floor, not an exact count: extra
      tests are fine, fewer are not. **The ignored count is exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The typed error exists and is exported:
      ``grep -c 'pub struct ShellExited' src/shell/client.rs`` → **1**
      (current: 0), and ``grep -c 'ShellExited' src/shell/mod.rs`` → **≥ 1**
      (current: 0).
- [ ] The run loop polls status:
      ``awk '/^async fn read_to_marker/,/^}/' src/shell/client.rs | grep -c 'status('``
      → **≥ 1** (current: 0). *(A proxy — Mutation A pins it.)*
- [ ] The final-deadline wording is unchanged (*preservation*):
      ``grep -c 'timed out after {timeout:?} waiting for command output: {cmd:?}' src/shell/client.rs``
      → **1** (current: 1).
- [ ] The orphaned fragment is gone:
      ``grep -c 'The non-chat entry point: no transport' src/daemon/executor/shell_exec.rs``
      → **0** (current: 1).
- [ ] One helper renders both run errors:
      ``grep -c 'fn shell_run_error_text' src/daemon/executor/mod.rs`` → **1**
      (current: 0), and
      ``grep -c 'The command may still be running in that shell' src/daemon/executor/mod.rs``
      → **1** — the caveat now lives only in the helper (current: 2).
- [ ] All six non-ignored tests pass:

      ```sh
      cargo test --lib -- \
        client_run_command_fails_fast_when_the_shell_program_has_exited \
        client_run_command_fails_fast_when_the_command_exits_the_shell \
        client_run_command_on_a_live_shell_is_not_cut_short_by_the_exit_check \
        client_spawn_command_fails_fast_when_the_shell_program_has_exited \
        an_exited_shell_is_reported_as_exited_not_as_a_timeout \
        a_dead_shell_error_does_not_say_the_command_may_still_be_running
      ```

      → **6 passed** (current: `0 passed; 0 failed; 2045 filtered out`).
- [ ] Every existing client test still passes (*preservation*):
      ``cargo test --lib shell::client::tests`` → **0 failed**, and in
      particular `client_run_command_times_out_on_a_silent_command`,
      `client_run_command_survives_a_chunk_split_mid_marker` and
      `client_run_command_ignores_interleaved_non_chunk_frames` pass.
- [ ] Both mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.
- [ ] The Task 6 live check was run and its output pasted into the same
      entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.**

- **Mutation A — never poll.** In `read_to_marker`, make the `Exited` branch
  unreachable (e.g. match nothing instead of `ShellState::Exited`).
  `client_run_command_fails_fast_when_the_shell_program_has_exited` must go
  **red** — it now waits 20 s and times out. (Expect that one test run to
  take ~20 s.)
- **Mutation B — clear on a slice timeout.** Put `line.clear()` back at the
  top of the loop so it also runs after a slice elapses. Report what you
  observe across `cargo test --lib shell::client::tests` — **this mutation
  may not go red**: a half-frame only exists if a slice fires mid-frame, and
  the fixtures send short frames. Record the observation honestly either way;
  do not contrive a test to force it. The spec pins the rule in § Current
  state 4, and review checks it by reading.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Seven new tests (Task 4): four on the shared client loop (two fail-fast
shapes, one live-shell preservation, the `spawn_command` path), one on the
error context, one on the model-facing text, one live and ignored. Every
existing test must stay green — the client's own framing tests are the
guard on § Current state 4's hazard.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'client_tests=%s\n' "$(cargo test --lib shell::client::tests 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'exited_type=%s\n' "$(grep -c 'pub struct ShellExited' src/shell/client.rs)"
  printf 'loop_polls=%s\n' "$(awk '/^async fn read_to_marker/,/^}/' src/shell/client.rs | grep -c 'status(')"
  printf 'deadline_wording=%s\n' "$(grep -c 'timed out after {timeout:?} waiting for command output: {cmd:?}' src/shell/client.rs)"
  printf 'orphan_gone=%s\n' "$(grep -c 'The non-chat entry point: no transport' src/daemon/executor/shell_exec.rs)"
  printf 'helper=%s\n' "$(grep -c 'fn shell_run_error_text' src/daemon/executor/mod.rs)"
  printf 'caveat_sites=%s\n' "$(grep -c 'The command may still be running in that shell' src/daemon/executor/mod.rs)"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-08b.txt
cat /tmp/e2e-08b.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`** (a `FAILED` line names the
failing tests, so a one-off failure is identifiable — never paper over one),
`lib_counts=` at least `2043p-0f-9i`, `client_tests=` with `0f`,
`exited_type=1`, `loop_polls=1` (or more), `deadline_wording=1`,
`orphan_gone=0`, `helper=1`, `caveat_sites=1`, `ignore_total=9`.

**Run this block with nothing else running `cargo test`.** Two test
processes at once can collide on shared state and produce a failure that is
not this phase's.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-08b-shell-exit-detection.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08b.txt
diff /tmp/pasted-08b.txt /tmp/e2e-08b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results and
the live check belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/shell/client.rs` — Task 1 and tests 1–4, 7.
- `src/shell/mod.rs` — the one `ShellExited` export.
- `src/daemon/executor/shell_exec.rs` — Task 2 and test 5.
- `src/daemon/executor/mod.rs` — Task 3 and test 6.
- This phase doc's Update Log.

## Out of scope

- **Connection-time ssh prompts** (a password or host-key prompt on a fresh
  remote shell). Measured at drafting: the first command is written *before*
  any prompt is classified, so its bytes are consumed as ssh's password
  attempt. That is row **08e**, which needs its own probing. Do not route
  plain commands through the prompt classifier here.
- **`BatchMode` for ghost shells** — row 08c. `ssh_argv` and its
  `ssh_argv_does_not_set_batch_mode` pin are untouched.
- **`ShellResponse::Exited`** — unused on the wire; do not start producing it.
- **Reaping or restarting the dead shell.** This row reports the exit; what
  the registry does with an exited shell is unchanged.

## Update Log

<!-- entries appended below this line -->
