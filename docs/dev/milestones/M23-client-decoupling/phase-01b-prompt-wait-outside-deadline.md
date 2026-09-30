# Phase 01b: prompt-wait-outside-deadline — the time a user spends answering a password prompt no longer counts against the command's timeout

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** none (M21 carry-forward)
**Estimated diff:** ~10 lines of production code across 2 files; 2 new tests; 1 comment rewrite
**Tags:** language=rust, kind=bugfix, size=s

## Goal

A command that prompts for a password has a deadline fixed when it starts,
and the wait for the user's answer runs **inside** that deadline. A user who
takes longer than the timeout to type gets the worst outcome. **The password
is sent, and then the command is killed as timed out.** After this phase, the
time spent waiting on a prompt's answer is added back to the deadline. The
command gets its full budget for its own work, and a command that hangs after
being answered still times out on schedule.

Two small follow-ons ride along:
- **09b's sudo-edit timeout workaround goes.** It is now false.
- **01a's review residual:** a test comment that still states the wrong
  spawn diagnosis is corrected.

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md` — row 01b (and the
  01a residual it carries).
- `docs/dev/milestones/M21-host-model/phase-09b-remote-sudo-edit.md`
  § Current state 4 — where this was first recorded.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

**Shell-gate note.** Your shell refuses any command you type that contains
the literal word for the privilege tool. The two new tests are named without
it; filter them by those names. Source code may contain the word (the
existing `PROMPTING` fixture does), written with the `patch` tool.

## Current state

Measured on **2026-09-30** at commit `2dbd1299`.

Baseline: `cargo test --lib` → **2090 passed / 0 failed / 14 ignored**.

### 1. The deadline (`src/shell/client.rs`, `read_to_marker`)

There are **two** `let deadline = …` lines in this file. **Change only the
one in `read_to_marker`** (`:367`). The other (`:147`) belongs to a different
function and is out of scope.

```rust
    let mut offered_at: usize = 0;
    let deadline = tokio::time::Instant::now() + timeout;
```

and, further down in the same function (`:443`), the callback where the
password round trip happens. `on_chunk` blocks here until the user answers:

```rust
                if buf.len() > offered_at {
                    let seen = String::from_utf8_lossy(&buf[offered_at..]).into_owned();
                    match on_chunk(&seen).await {
                        PromptReply::Wait => {}
                        PromptReply::Send(answer) => {
```

### 2. Measured

Both runs used a throwaway probe in `shell_exec.rs`'s `tests` module, since
reverted. It used the existing `PROMPTING` fake (a shell function that
prompts with `$SUDO_PROMPT` and prints `GOT_<len>`), a 2 s timeout, and a
transport whose answer arrives **3 s** after the prompt (a `tokio::io::duplex`
whose other end a spawned task writes to after `sleep(3s)`):

| Tree | Result |
|---|---|
| Today | `Err("… did not complete within the 2s limit: timed out after 2s waiting for command output: …")` after 3.00 s. **The answer had already been written.** |
| With Task 1 | `Ok((0, "[de-sudo-prompt-0]\nGOT_15"))` after 3.00 s |

And the negative, with Task 1 applied: an **instantly** answered prompt
followed by a hang (`<prompt>; sleep 30`, timeout 2 s) timed out at
**2.00 s**. Only the answer's wait is excluded, never the command's own run
time. With Task 1, `cargo test --lib shell::client` (26) and
`cargo test --lib shell_exec` (38) both passed unchanged.

### 3. The 09b workaround (`src/daemon/executor/file_ops/ops.rs:30-33`)

```rust
/// A sudo edit's timeout, in seconds. The run's deadline includes the user
/// typing the password, and the client waits up to 120 s for it
/// (`USER_PROMPT_TIMEOUT`), so 30 s would cut a slow typist off mid-prompt.
const SUDO_EDIT_TIMEOUT_SECS: u64 = 150;
```

Its one use is `Some(SUDO_EDIT_TIMEOUT_SECS)` at `:144`. After Task 1 its
premise is false.

### 4. The 01a residual (`src/daemon/executor/shell_exec.rs:2751-2755`, `connect_tests`)

```rust
    /// Serialises this module's PTY-spawning tests against each other: each
    /// forks a real `sh` into a PTY, and under full-suite load concurrent
    /// forks intermittently fail (`spawn_command failed`). Each test also
    /// holds `test_home_guard()` next to this lock, so a HOME-mutating test
    /// cannot delete `HOME` out from under one of these spawns.
```

The middle clause ("under full-suite load concurrent forks intermittently
fail") is the wrong diagnosis 01a removed everywhere else. Concurrent forks
never failed; spawns into a deleted `HOME` did.

## Spec

### Task 1 — exclude the answer's wait from the deadline (`src/shell/client.rs`, `read_to_marker` only)

1. `let deadline = …` → `let mut deadline = …` (the `:367` line).
2. Replace `match on_chunk(&seen).await {` with:

   ```rust
                    // The callback blocks while a prompt waits for the user's
                    // answer. That wait is not the command's run time, so it is
                    // added back to the deadline: a slow typist is not cut off
                    // after their password was already sent.
                    let asked = tokio::time::Instant::now();
                    let reply = on_chunk(&seen).await;
                    deadline += asked.elapsed();
                    match reply {
   ```

   Nothing else in the function changes.

### Task 2 — retire the 09b workaround (`src/daemon/executor/file_ops/ops.rs`)

Delete `SUDO_EDIT_TIMEOUT_SECS` and its doc comment. Its use at `:144`
becomes `Some(REMOTE_WRITE_TIMEOUT_SECS)`, the same 30 s every remote write
uses.

### Task 3 — the 01a residual (`src/daemon/executor/shell_exec.rs`, `connect_tests`)

Replace the `PTY_TEST_LOCK` doc comment with exactly:

```rust
    /// Serialises this module's PTY-spawning tests against each other. Each
    /// test also holds `test_home_guard()` next to this lock, so a
    /// HOME-mutating test cannot delete `HOME` out from under one of these
    /// spawns (a spawn into a deleted `HOME` fails with `ENOENT`).
```

### Task 4 — tests (`src/daemon/executor/shell_exec.rs`, the `tests` module, not `connect_tests`)

Two tests, names pinned. Both use this module's existing fixtures:
- `test_home()`, which holds the HOME guard;
- a seed run that defines `PROMPTING` in a shell from `spawn_with_cast`, the
  way `run_with_transport` does;
- a second `run_in_shell_with_prompts` call in that same shell
  (`Some(id.as_str())`) with a **2 s** timeout (`Some(2)`) and prompt id `"p"`.

1. `an_answer_after_the_timeout_still_completes_the_command`: the command is
   `<the privilege tool> true`, so the fake prompts. The transport's read
   half is a `tokio::io::duplex(4096)`. A spawned task sleeps **3 s**, writes
   one `Request::CredentialResponse { id: "p", credential: CREDENTIAL }` line
   to the other end, then sleeps 30 s so the stream stays open. The write
   half is `tokio::io::sink()`. Assert the run returns `Ok` with exit code 0
   and text containing `GOT_15`.
2. `a_hang_after_an_answer_still_times_out`: the command is
   `<the privilege tool> true; sleep 30`, answered instantly by
   `answered_transport()`. Assert the run returns `Err` whose `{e:#}`
   contains `timed out after 2s`, and that it took **under 5 s**
   (`std::time::Instant` around the call).

**Test count:** two new, so **2092 passed / 14 ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output **into
this phase doc** as the first fenced block of a new Update Log entry titled
exactly `### Update — <date> (end-to-end verification)`. A summary in your
final message is not the entry. Then run the paste check and put its literal
`PASTE MATCH` line in the entry.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was run at drafting.

- [ ] `cargo test --lib` → **2092 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2090/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The two new tests pass:
      ``cargo test --lib -- an_answer_after_the_timeout_still_completes_the_command a_hang_after_an_answer_still_times_out``
      → **2 passed** (current: `0 passed; … filtered out`).
- [ ] The silent-command timeout is unchanged (*preservation*):
      ``cargo test --lib client_run_command_times_out_on_a_silent_command``
      → **1 passed**.
- [ ] The 09b workaround is gone:
      ``grep -c 'SUDO_EDIT_TIMEOUT_SECS' src/daemon/executor/file_ops/ops.rs``
      → **0** (current: **2**).
- [ ] The wrong diagnosis is gone:
      ``grep -c 'concurrent$' src/daemon/executor/shell_exec.rs`` → **0**
      (current: **1**, the line ending `and under full-suite load concurrent`).
- [ ] Mutations A and B were applied, run, and their observed results
      recorded.
- [ ] The § End-to-end verification block was run verbatim and pasted as the
      **first fenced block** of the `(end-to-end verification)` entry, with
      the literal `PASTE MATCH`:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>``
      → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run once, restore, and record the observed
result.

- **Mutation A:** delete `deadline += asked.elapsed();`. Run test 1.
  Expected **red** (`timed out after 2s`).
- **Mutation B:** replace `asked.elapsed()` with
  `std::time::Duration::from_secs(60)`, so every callback extends the
  deadline by a minute whether or not anyone waited. Run test 2. Expected
  **red**: the hang completes after 30 s instead of timing out, so the
  under-5-s assertion fails.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Two tests pin both halves of the change. A late answer no longer kills an
answered command. The budget is still enforced for the command's own run
time.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- an_answer_after_the_timeout_still_completes_the_command a_hang_after_an_answer_still_times_out 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'silent_timeout=%s\n' "$(cargo test --lib client_run_command_times_out_on_a_silent_command 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'workaround=%s\n' "$(grep -c 'SUDO_EDIT_TIMEOUT_SECS' src/daemon/executor/file_ops/ops.rs)"
  printf 'wrong_diagnosis=%s\n' "$(grep -c 'concurrent$' src/daemon/executor/shell_exec.rs)"
} > /tmp/e2e-m23-01b.txt
cat /tmp/e2e-m23-01b.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2092p-0f-14i`,
`new_tests=2p-0f`, `silent_timeout=1 passed`, `workaround=0`,
`wrong_diagnosis=0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Paste the file into
this phase doc, then run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-01b-prompt-wait-outside-deadline.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-01b.txt
diff /tmp/pasted-m23-01b.txt /tmp/e2e-m23-01b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes into the entry. The mutation results go in
the same entry, after the first block.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/client.rs`: Task 1, inside `read_to_marker` only.
- `src/daemon/executor/file_ops/ops.rs`: Task 2 (the constant and its one
  use).
- `src/daemon/executor/shell_exec.rs`: Task 3 (the one doc comment) and
  Task 4's tests.
- This phase doc's Update Log.

## Out of scope

- **The other `let deadline` in `client.rs`** (`:147`).
- **`USER_PROMPT_TIMEOUT`** (120 s), the client's own bound on how long it
  waits for an answer. It is unchanged, and it is what keeps the extension
  finite.
- **Default timeouts** for `run_terminal_command` (45 s) and remote writes
  (30 s). Unchanged.

## Update Log

<!-- entries appended below this line -->
