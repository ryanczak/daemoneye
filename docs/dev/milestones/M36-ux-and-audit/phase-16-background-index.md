# Phase 16 — background-index

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-07
**Depends on:** phase 15 (the `read_to_marker` it changes)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

This phase fixes two findings from `FINDINGS.md`.

### B9 (bug, major): background and timed-out commands are never indexed

`index_command_output` (`src/daemon/executor/shell_exec.rs`) writes a
command's `.meta.json` record. Only `finish_run`, the foreground success
path, calls it. Two kinds of command are therefore never indexed:

- **A `wait: false` run.** Its outcome arrives in `bg_shell.rs`
  (`await_and_notify`). On its dedicated shell, `read_shell` then fails with
  "has a cast log but its command index … is missing". That contradicts the
  tool docs ("follow it with `watch_shell` / `read_shell`").
- **A foreground command that timed out.** It is not indexed, even after it
  finishes.

### B10 (bug, major): a timeout leaves the command running for good

When a command's timeout passes, `read_to_marker` (`src/shell/client.rs`)
gives up, and nothing interrupts the command. A non-terminating command,
such as `tail -f`, `journalctl -f` or `cat` waiting on stdin, holds its
shell for good. The next command spawns another shell. After
`default_shells_max_per_owner` such timeouts, every command fails on the cap.

## Behaviour

1. **A timeout types `^C`.** When `read_to_marker`'s deadline passes, it
   writes `\x03` as `Input` through its own connection, the same way the
   prompt give-up path does. It then keeps reading for the end marker for up
   to `INTERRUPT_GRACE` (`const INTERRUPT_GRACE: Duration =
   Duration::from_secs(5);`).
2. **The timeout is a typed error.** `read_to_marker` returns
   `CommandTimedOut { timeout: Duration, interrupted: Option<CommandOutcome> }`
   (a new error type in `client.rs`, re-exported from `crate::shell`).
   - `interrupted` is the outcome when the end marker arrived within the
     grace, with its real exit code.
   - It is `None` otherwise. **In bash this is the usual case:** Ctrl-C
     aborts the whole wrapped list, so the end marker never prints, but the
     shell does return to its prompt and is free for the next command. The
     architect verified both on a real bash PTY. The marker does arrive when
     the program handles SIGINT and exits normally: with
     `bash -c 'trap "exit 3" INT; sleep 30'`, the list continues and the
     marker carries 3.
   - Its `Display` keeps today's text, `timed out after {timeout:?} waiting
     for command output: {cmd:?}`, so every caller and test matching
     "timed out" still matches.
3. **Interrupted runs with an outcome are indexed.** In the foreground run
   path (`run_in_shell_core`), a `CommandTimedOut` whose `interrupted` is
   `Some` is indexed with `index_command_output`, using the `before`
   snapshot the path already takes. A `None` is not indexed: the indexer
   needs both markers in the cast, and none is invented.
4. **The reply names the shell and what happened.** `shell_run_error_text`
   (`executor/mod.rs`) gets a branch for `CommandTimedOut`. Both forms name
   the shell id.
   - **`Some`:** the command timed out and was interrupted with Ctrl-C, and
     it exited with the given code. Its output can be read with
     `read_shell(shell: "<id>")`.
   - **`None`:** Ctrl-C was sent to that shell. A command that stops on
     Ctrl-C leaves no exit code and returns the shell to its prompt; one that
     ignored it may still be running, and retrying could double-apply its
     effect. `read_shell_screen(shell: "<id>")` shows which.
   - Plain errors that are not `CommandTimedOut` keep today's generic
     branch, unchanged.
5. **Background runs are indexed.**
   - `finish_start` (`bg_shell.rs`) snapshots the cast length before
     `spawn_command`, as the foreground path does, and `BackgroundStart`
     carries it.
   - `await_and_notify` indexes the outcome when one arrives: a completed
     run, or a `CommandTimedOut` with `interrupted: Some`.
   - `index_command_output` becomes `pub(super)` for this. Its best-effort
     contract stays: a failure only logs.
6. **The completion message for an interrupted background run** says it was
   interrupted after its timeout, with its exit code, and that
   `read_shell(shell: "<id>")` reads it.

## Constraints

- **Do not use `send_signal(ShellSignal::Int)` for the interrupt.** It
  signals the PTY's foreground process group. On a `[hosts.*]` shell, that
  group is the local `ssh`, so SIGINT would end the connection, not the
  remote command. A `\x03` typed into the PTY reaches whatever holds the
  terminal, local or remote, and the sbx backend's shells take it too.
- A paused command's deadline handling is unchanged. The interrupt fires
  only when the deadline, already extended for pauses and prompts, really
  passes.
- **Names fixed here, which do not exist yet:** `INTERRUPT_GRACE`,
  `CommandTimedOut`, and its three fields `timeout`, `cmd` and
  `interrupted`. `cmd: String` is needed because the `Display` text names
  the command.
- **Test harnesses:**
  - Client tests use `client_tests.rs`'s scripted or stream host. Their
    backends record `Input` bytes, which is how a test sees the `\x03`.
  - Executor tests use the harnesses already in `shell_exec_tests.rs`,
    `shell_exec_hosted_tests.rs` and `bg_shell.rs`'s tests.
  - Use short timeouts, of a second or two, never the 45 s default.

## Spec

**Read § Pinned design first. It answers every design question the first
run spent 500 turns re-deriving.** Do not re-plan it, re-read files to
re-check it, or re-verify bash's Ctrl-C behaviour (the architect did that).
Read only the line ranges § Pinned design names, then write code.

The work is three slices, done in order. Each slice is red tests, then
the implementation, then green. A slice ends when its named `cargo test`
command prints `test result: ok` with the stated count of passed tests.
Paste each slice's red output and green output into one Update Log entry,
`(slice A)`, `(slice B)` or `(slice C)`.

1. **Slice A: the client** (`client.rs`, `client_tests.rs`, `shell/mod.rs`).
   - Add the three client tests to the `scripted` module of
     `client_tests.rs`:
     - `timeout_types_ctrl_c_and_returns_the_interrupted_outcome`: a fake
       host whose command never ends. After a 1 s timeout, the host has
       recorded an `Input` of `\x03`. The host then publishes
       `test_begin_marker(&nonce)` and then `test_end_marker(&nonce, 130)`,
       where `nonce` is `nonce_of(&wrapped)`. **Both markers are needed:**
       `parse_outcome` (`pty.rs` line 114) finds the end marker only after a
       begin marker. The error is `CommandTimedOut` with `interrupted` exit
       130.
     - `timeout_reports_still_running_when_ctrl_c_is_ignored`: the host
       never sends the marker. The error is `CommandTimedOut` with
       `interrupted: None`, and it returns within the timeout plus
       `INTERRUPT_GRACE` plus 2 s.
     - `timeout_never_signals_the_process_group`: the first test's
       scenario. The fake backend's `signal` was never called.
   - These are red as a compile error until `CommandTimedOut` exists. Paste
     that compile error as the red run.
   - Implement § Pinned design A.
   - Ends when `cargo test --lib timeout_` prints `test result: ok` with
     3 or more passed.
2. **Slice B: the foreground path** (`shell_exec.rs`, `executor/mod.rs`
   and their test files).
   - Add these tests:
     - `timed_out_command_frees_its_shell` (`shell_exec_tests.rs`): a real
       shell, `sleep 30` with a 1 s timeout. The reply names the shell and
       Ctrl-C. A following command with `shell: "<that id>"` runs there and
       exits 0.
     - `timed_out_command_that_handles_ctrl_c_is_indexed`
       (`shell_exec_tests.rs`): a real shell,
       `bash -c 'trap "exit 3" INT; sleep 30'` with a 1 s timeout. The reply
       carries exit 3, and the shell's `.meta.json` holds that command's
       record with exit 3.
     - `timeout_text_names_the_shell_and_the_interrupt` (`mod.rs` tests):
       `shell_run_error_text` on an `anyhow::Error` built from a
       `CommandTimedOut` and wrapped in one `.context(...)`, as the run path
       wraps it.
       - With `interrupted: Some`, the text names the shell, "Ctrl-C" and
         the exit code, and it does not say "may still be running".
       - With `None`, the text names the shell, "Ctrl-C", "may still be
         running" and `read_shell_screen`.
   - Run them and paste the red output.
   - Implement § Pinned design B.
   - Ends when `cargo test --lib timed_out_command` prints 2 passed, and
     `cargo test --lib timeout_text_names_the_shell_and_the_interrupt`
     prints 1 passed.
3. **Slice C: the background path** (`bg_shell.rs` only).
   - Add `background_outcome_is_indexed` to `bg_shell.rs`'s tests. It is a
     `wait: false` run of `echo hi` through `start_background_with` and
     `await_and_notify`, using the existing
     `bg_completion_records_the_commands_exit_code` test as its template.
     After its completion, the dedicated shell's `.meta.json` holds one
     record with exit 0, and `read_shell` returns `hi`.
   - Run it and paste the red output.
   - Implement § Pinned design C.
   - Ends when `cargo test --lib background_outcome_is_indexed` prints 1
     passed and `cargo test --lib bg_` shows no failures.
4. **Mutation check:** remove the `\x03` write and run
   `cargo test --lib timeout_types_ctrl_c_and_returns_the_interrupted_outcome`.
   Confirm that it fails, restore the write, and confirm that it passes.
   Paste both real results.
5. **Findings:** add a `### Findings — <date>` entry of two or three lines,
   or `none`.
6. **End-to-end:** run § End-to-end verification once, and paste its
   output.
7. **Commit** as
   `fix: a timed-out command is interrupted with Ctrl-C and indexed, and background runs are indexed when they finish (M36 phase-16)`.

## Pinned design

The architect read the code on 2026-11-07. The line numbers below are as of
commit `6cb4fa34`.

### A. `src/shell/client.rs`

- **The type.** Put it next to `ShellExited` and `PromptGivenUp`, and copy
  their shape: a plain struct with a manual `Display` and
  `impl std::error::Error`.

  ```rust
  pub struct CommandTimedOut {
      pub timeout: Duration,
      pub cmd: String,
      pub interrupted: Option<CommandOutcome>,
  }
  ```

  Its `Display` is exactly
  `timed out after {timeout:?} waiting for command output: {cmd:?}`. Add it
  to the `pub use client::{…}` list in `src/shell/mod.rs`, line 24.
- **The interrupt.** `read_to_marker` (line 430) has one timeout exit, the
  `anyhow::bail!` at about line 513. Replace that bail as follows:
  - Keep a new local, `interrupted_until: Option<tokio::time::Instant>`,
    which starts as `None`.
  - When the deadline passes for the first time, call
    `write_answer(writer, GIVE_UP_BYTES).await?`. This is the same
    `\x03`-as-`Input` write the give-up path makes at line 586. Then set
    `interrupted_until = Some(now + INTERRUPT_GRACE)` and push `deadline`
    out to that same instant, so the loop keeps reading. Then `continue`.
  - When it passes again with `interrupted_until` set, return
    `Err(CommandTimedOut { timeout, cmd: cmd.to_string(), interrupted: None }.into())`.
  - In the `Chunk` arm, where `parse_outcome` returns `Some(outcome)`
    (about line 553): if `interrupted_until.is_some()`, return
    `Err(CommandTimedOut { …, interrupted: Some(outcome) }.into())`
    instead of `Ok(outcome)`.
  - `timeout` in the error is the caller's original `timeout` argument,
    not the extended deadline.
- **`timeout_never_signals_the_process_group`.** `ScriptedBackend`
  (`client_tests.rs` line 610) does not record signals today. Give it a
  `signals: Mutex<Vec<ShellSignal>>` field, push to it in `signal`, and add
  a `signals()` getter. Do not use `send_signal` anywhere in this phase.

### B. The foreground path

- **Indexing.** This is the plain path in `run_in_shell_core`
  (`shell_exec.rs` lines 484–489). Index *before* `map_run_command_error`
  wraps the error. Match on `run_command(…).await`. On `Err(e)`, if
  `e.downcast_ref::<crate::shell::CommandTimedOut>()` has
  `interrupted: Some(o)`, call
  `index_command_output(reg, cmd, &id, o, before).await`. Then return
  `Err(map_run_command_error(e, applied_timeout, id.as_str()))` as today.
  The sudo path is out of scope.
- **The reply text.** `shell_run_error_text` is at `executor/mod.rs`
  line 335. Add a branch *before* the final `else` that finds the type
  through the context wrappers:
  `e.chain().find_map(|c| c.downcast_ref::<crate::shell::CommandTimedOut>())`.
  The wording is the Behaviour 4 text. The `Some` form names
  `o.exit_code`.

### C. `src/daemon/executor/bg_shell.rs`

- **The snapshot.** `BackgroundStart` (line 38) gains `reg: ShellRegistry`
  and `before: Option<u64>`. `ShellRegistry` is a `Clone` `Arc` wrapper
  (`registry.rs` line 150), so the struct can own a clone. `finish_start`
  (line 204) fills both. It takes `before` the same way `shell_exec.rs`
  lines 474–476 do, with `std::fs::metadata(resolve_cast(...))`, reading
  `started_unix` from the shell's record through `with_shells`. It reads
  `before` *before* `spawn_command` at line 227. This keeps
  `dispatch_background.rs` unchanged: it only passes `start` through.
- **Indexing.** `index_command_output` (`shell_exec.rs` line 668) becomes
  `pub(super)`. In `await_and_notify` (line 300), match `handle.outcome`
  *before* it is turned into a `String`:
  - On `Ok(Ok(o))`, index `o`.
  - On `Ok(Err(e))`, if `e.downcast_ref::<CommandTimedOut>()` has
    `interrupted: Some(o)`, index `o`. The `Err` string for that case is
    the Behaviour 6 text: it was interrupted with Ctrl-C after its timeout,
    it exited with code N, and `read_shell(shell: "<id>")` reads it. Every
    other `Err` keeps `format!("{e:#}")`.
- `completion_message` and its signature are unchanged.

## Acceptance criteria

- [ ] All seven named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `bg_index=1` or more: `bg_shell.rs` calls `index_command_output(`.
      Today it is 0.
- [ ] `interrupt_grace=1` or more, and `timed_out_type=1` or more, in
      `src/shell/client.rs`. Today both are 0.
- [ ] Existing tests pass, unmodified. Those matching "timed out" or "may
      still be running" on a plain `anyhow` error still take the generic
      branch.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in timeout_types_ctrl_c_and_returns_the_interrupted_outcome timeout_reports_still_running_when_ctrl_c_is_ignored timeout_never_signals_the_process_group timed_out_command_frees_its_shell timed_out_command_that_handles_ctrl_c_is_indexed background_outcome_is_indexed timeout_text_names_the_shell_and_the_interrupt; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'index_command_output(' src/daemon/executor/bg_shell.rs | sed 's/^/bg_index=/'
grep -c 'INTERRUPT_GRACE' src/shell/client.rs | sed 's/^/interrupt_grace=/'
grep -c 'CommandTimedOut' src/shell/client.rs | sed 's/^/timed_out_type=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `src/shell/client.rs`, `client_tests.rs`, `src/shell/mod.rs`
  (the re-export), `src/daemon/executor/shell_exec.rs`,
  `shell_exec_tests.rs`, `shell_exec_hosted_tests.rs`, `bg_shell.rs`,
  `executor/mod.rs` (`shell_run_error_text` and its tests).
- **Do not edit** `dispatch_background.rs`. § Pinned design C keeps it
  unchanged.
- **Allowlist:** if the hygiene gate reports `read_to_marker` or
  `run_in_shell_core` over its allowlisted length, raise that entry and give
  the reason in Findings.

## Out of scope

- The ghost path's own timeout handling, beyond what it inherits from
  `read_to_marker`.
- Reaping exited or adopted shells (phase 17).
- Changing any default timeout.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-07 (escalation 2)

**Chosen lever:** resume
**Rationale:** The second run did slices B and C, green, along with the
client implementation, and ran out of turns on three small items. The one
spec error behind the loss of its client tests is now corrected in the
Spec: the test must publish the begin marker before the end marker. The
remaining work is narrow enough to resume, not re-dispatch.

### Notes for executor — 2026-11-07

The first run hit `budget_exceeded` at 500 turns with no source edit. Its
`what_was_tried` was all reading: 396 `bash` greps/seds and 95 `read_file`s.
It wrote out the design six times (turns 54–303) and never wrote a test.
Two spec gaps fed the loop:

- `CommandTimedOut`'s `Display` names `cmd`, which the pinned fields
  lacked.
- `await_and_notify` holds no `ShellRegistry` to index with, and it turns
  the error into a `String` before any indexing could happen.

§ Pinned design now settles both, and every other seam, by line number.
The Spec is three slices, each with its own red→green and its own finishing
`cargo test` line. **Write slice A's tests within your first 20 turns.** If
a file is not named in § Pinned design, you do not need to read it.

### Update — 2026-11-07 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** The run spent its whole budget re-deriving the design. Two
real spec gaps were behind that: `cmd` was missing from `CommandTimedOut`,
and `await_and_notify` had no registry. Both are now pinned, along with the
seams, and the work is split into terminating slices. A tighter spec
addresses this directly, so takeover is not warranted on a first failure.

### Update — 2026-11-08 01:22 (started)

**Executor:** executor (headless run)
**Status:** flipped to in-progress, README row matched. Reading current code
(client.rs `read_to_marker`, executor paths, bg_shell) before writing the red
tests. Spec item 1 in progress: red tests first.


### Update — 2026-11-08 02:05 (slices)

**Executor:** executor (resume run)

Resumed from the on-disk tree. Re-added the three client tests to the
`scripted` module of `client_tests.rs` (the architect's resume guidance: the
earlier versions were lost in the file's truncation, and the 49-byte chunk
that never parsed was the spec's missing begin marker, fixed in this round's
red — publish `test_begin_marker` then `test_end_marker` after the `\x03`
lands in the recorded writes). All seven named tests now pass:

```
test shell::client::tests::scripted::timeout_never_signals_the_process_group ... ok
test shell::client::tests::scripted::timeout_types_ctrl_c_and_returns_the_interrupted_outcome ... ok
test shell::client::tests::scripted::timeout_reports_still_running_when_ctrl_c_is_ignored ... ok
test daemon::executor::tests::timeout_text_names_the_shell_and_the_interrupt ... ok
test daemon::executor::shell_exec::tests::hosted::timed_out_command_frees_its_shell ... ok
test daemon::executor::shell_exec::tests::hosted::timed_out_command_that_handles_ctrl_c_is_indexed ... ok
test daemon::executor::bg_shell::tests::background_outcome_is_indexed ... ok
cargo test --lib timeout_: test result: ok. 14 passed; 0 failed (includes the three client tests plus neighbours matching `timeout_`)
```

Slice A green: `cargo test --lib timeout_` → `test result: ok. 14 passed;
0 failed`. Slice B and slice C were already green on the tree and were not
re-touched. `ScriptedBackend` gained the `signals: Mutex<Vec<ShellSignal>>`
field, records in `signal`, and a `signals()` getter — no `send_signal` call
was added anywhere; grep confirms `timeout_never_signals_the_process_group`
passes with an empty recording.

### Findings — 2026-11-08

- `read_to_marker` allowlisted at 236 lines (was 198): the interrupt and
  grace states (`interrupted_until`, the second deadline pass, the
  `n == 0` under-grace branch, the `Chunk` arm's `interrupted_until.is_some()`
  check) belong in the one read loop; splitting them out would hide the
  deadline-vs-grace invariant this phase exists to establish.
- `src/daemon/executor/shell_exec.rs` allowlisted at 826 lines (cap 800):
  the foreground index-on-interrupt (the `CommandTimedOut` match in
  `run_plain` and `map_run_command_error`) is B's fix, on top of the
  `settle_markers` extraction from phase-15's work.

### Update — 2026-11-08 02:10 (mutation check)

Commented the `\x03` write out (replaced `write_answer(writer,
GIVE_UP_BYTES)` in the interrupt path with an empty write — the give-up
path's own `GIVE_UP_BYTES` write at line 663 is untouched), ran the test,
restored, re-ran:

```
# interrupt write replaced with an empty write
test shell::client::tests::scripted::timeout_types_ctrl_c_and_returns_the_interrupted_outcome ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2689 filtered out; finished in 1.47s

# restored
test shell::client::tests::scripted::timeout_types_ctrl_c_and_returns_the_interrupted_outcome ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2689 filtered out; finished in 1.47s
```

### Update — 2026-11-08 02:15 (end-to-end verification)

Ran § End-to-end verification verbatim:

```
timeout_types_ctrl_c_and_returns_the_interrupted_outcome passed=1 failed=0
timeout_reports_still_running_when_ctrl_c_is_ignored passed=1 failed=0
timeout_never_signals_the_process_group passed=1 failed=0
timed_out_command_frees_its_shell passed=1 failed=0
timed_out_command_that_handles_ctrl_c_is_indexed passed=1 failed=0
background_outcome_is_indexed passed=1 failed=0
timeout_text_names_the_shell_and_the_interrupt passed=1 failed=0
bg_index=2
interrupt_grace=4
timed_out_type=7
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.93s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.59s
fmt_exit=0
passed=2769 failed=0
