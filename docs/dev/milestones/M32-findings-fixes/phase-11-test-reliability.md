# Phase 11 — Test reliability

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Depends on:** phase-10
**Estimated diff:** ~190 lines
**Tags:** language=rust, kind=test, size=m
**Findings:** M31 `FINDINGS.md` #65, #83; M32 `FINDINGS.md` #11
(#5, #6, #51, #53 and #54 move to phase 11b)

## Goal

Two flakes and one coverage gap in the suite.

1. **M32 #11** — `for_each_streams_across_segments_in_order` and
   `for_each_skips_unparseable_lines` read every event segment under
   `HOME` and have failed four times with an event they did not write.
   The writers are four tests that call `log_event` without holding the
   HOME lock, so their event lands in whichever temp `HOME` another test
   has set at that moment. Found by instrumenting `log_event` and running
   the lib suite once:

   ```
   UNGUARDED log_event event=task_panic  thread=daemon::tests::supervise_restarts_on_panic
   UNGUARDED log_event event=file_create thread=daemon::executor::file_ops::ops::tests::the_local_write_path_is_unchanged
   UNGUARDED log_event event=command     thread=daemon::executor::foreground::tests::ghost_turn_without_a_pty_shell_is_refused
   UNGUARDED log_event event=command     thread=daemon::executor::foreground::tests::chat_command_without_session_is_refused
   ```

   After this phase the four tests take `crate::test_home()`, and
   `log_event` asserts under `cfg(test)` that the calling thread holds the
   lock — so the next unguarded writer fails by name, in its own test,
   instead of as a flake in an event-log test.
2. **M31 #65** — `read_shell_screen_lines_tails_the_visible_rows` reads
   the live screen twice: once to count the rows to tail, once with that
   tail. The header says `shell prompt` as soon as bash is back at its
   prompt, before the prompt row is drawn, so a redraw between the two
   reads changes the count. After this phase the test polls one read, with
   the tail under test (`lines: 2`), until that read shows `row-c` as its
   first row.
3. **M31 #83** — no test drives `run_terminal_command`'s approved path
   through `execute_tool_call`. After this phase two tests do: an approved
   local command runs in the session's PTY shell and the result carries
   its output, and a malformed host name is refused before any prompt.

The suite goes from **2607** to **exactly 2609**: two tests added, none
removed.

## Architecture references

- `src/lib.rs` § `test_home_guard` / `TestHomeGuard`: the lock the new
  assertion reads.
- `src/daemon/executor/dispatch_foreground.rs`: the arm the #83 tests
  drive; `src/daemon/executor/mod.rs` § the `tests` module, whose
  `get_shell_context_without_chat_pane_is_shells_only` is the existing
  `execute_tool_call` fixture the new tests follow.
- M32 README, decisions 1 and 3.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11-test-reliability.py && cargo fmt --all
   ```

   The script does the whole change:
   - `src/lib.rs`: a `#[cfg(test)]` thread-local `HOLDS_HOME_GUARD`, set
     by `test_home_guard()` and cleared by `TestHomeGuard::drop`;
   - `src/daemon/utils/event_log.rs`: `log_event` asserts it under
     `cfg(test)`, naming the event and the fix;
   - the four tests above take `crate::test_home()` as their first line;
   - `read_shell_screen_lines_tails_the_visible_rows` is rewritten to one
     read per poll;
   - `dispatch_foreground.rs` gains a `mod approved_path_tests` with its
     own in-process PTY host spawn and the two #83 tests.

   **Do not open the script, and do not read any source file before
   running it.** Every line of code it writes was compiled, linted and
   tested on a scratch worktree of `a46472b6`: fmt and clippy clean, the
   full suite at `passed=2609 failed=0`. Checking that it will compile is
   the compiler's job, and it already did it. The script asserts its own
   anchors and prints four lines, the last
   `two approved-path tests added`. If an assert fires, stop and report
   it in the Update Log. Do not hand-edit anything it touches, and do not
   edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `test:`.

### The red runs are done — do not repeat them

The assertion is the regression gate for #11, and it was run red before
the four guards went in: with the assertion alone, the four tests fail,
each naming its event (`log_event("command") from a test that does not
hold the HOME lock …`), `0 passed; 4 failed`. The script applies the
assertion and the guards together, so your run is green. #65 is a race
the fix removes structurally; 25 runs of the old test passed alone, and
25 of the new one. The #83 run test was mutation-checked: dropping the
`[ran in shell …]` line from `with_shell_line` fails it on
`the result names the shell it ran in`. Do not repeat any of this.

### The script

`docs/dev/milestones/M32-findings-fixes/scripts/phase-11-test-reliability.py`
(sha256 begins `3097d6e61978f5f4`). It is kept in the repo as the record of the change.
Run it; there is nothing in it to review.

## Acceptance criteria

- [ ] The End-to-end block prints `assert=1 guarded=4 approved_tests=2
      single_read=1` (today: `assert=0 guarded=0 approved_tests=0
      single_read=0`).
- [ ] `cargo test` reports **exactly** `passed=2609 failed=0` (today
      2607).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "assert=$(grep -c 'HOLDS_HOME_GUARD.with(|c| c.get())' src/daemon/utils/event_log.rs) guarded=$(grep -A1 -E 'async fn (supervise_restarts_on_panic|the_local_write_path_is_unchanged|ghost_turn_without_a_pty_shell_is_refused|chat_command_without_session_is_refused)\(\)' src/daemon/mod.rs src/daemon/executor/foreground.rs src/daemon/executor/file_ops/ops.rs | grep -c 'crate::test_home()') approved_tests=$(grep -cE 'async fn (an_approved_local_command_runs_in_the_sessions_shell|a_malformed_host_is_refused_without_asking)\(\)' src/daemon/executor/dispatch_foreground.rs) single_read=$(grep -c 'screen_until(&reg, &id, Some(2), None' src/daemon/executor/knowledge/shell_observer_tests.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `assert=1 guarded=4 approved_tests=2 single_read=1`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, `passed=2609 failed=0`. If
the suite stops early on a failure, paste that output, run the block once
more and paste both.

## Authorizations

- Run `scripts/phase-11-test-reliability.py`; do not edit it.
- Edit the seven files the script names, through the script only:
  `src/lib.rs`, `src/daemon/utils/event_log.rs`, `src/daemon/mod.rs`,
  `src/daemon/executor/foreground.rs`,
  `src/daemon/executor/file_ops/ops.rs`,
  `src/daemon/executor/knowledge/shell_observer_tests.rs`,
  `src/daemon/executor/dispatch_foreground.rs`.

## Out of scope

- #5, #6, #51, #53, #54: phase 11b (test-tidies).
- The event-log tests' own `with_test_home` helper (a closure form, not
  the phase-10 fixture's shape).
- Any test that the new assertion does **not** fail. The dry run ran the
  whole lib suite under it: exactly the four tests above fail, no other.

## Update Log

<!-- entries appended below this line -->
### Notes for executor — 2026-10-30

The first run (session `6ae40c6b`) stopped after 120 read-only turns. It
never flipped the status and never ran the script. From turn 30 on it was
checking whether the script's test code would compile, for example
whether `std::io::Cursor` implements `AsyncBufRead`. It does; the code was
compiled and the full suite run against it before this doc was written.
The script is now a file, so step 1 is one command. Your whole phase is:
the started entry, that command, the cuts-done entry, `git diff --stat`
for Findings, the End-to-end block, the commit. Read no source file.
Green gates are the expected outcome.

### Update — 2026-10-30 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the stall was verification of already-verified code, a
spec-shape problem; moving the script into a file and forbidding the
read removes it, which resume would carry forward and takeover would
not test.
