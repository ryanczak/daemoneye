# Phase 04 — run-records

**Status:** in-progress
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 03 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

M29's O5 is closed: the `command` records in `events.jsonl` say what really
happened. Two defects:

- **A `wait: false` command never records its exit code.** At launch,
  `dispatch_background.rs` writes a `command` record through
  `log_shell_command` with `status: "approved"` and `exit: 0`. That 0 is the
  launch's, not the command's, and nothing writes a record when the command
  finishes. After this phase the launch record carries `exit: null` (not
  known yet), and the completion writes a second `command` record with
  `status: "completed"`, `wait: false`, the shell id and the command's real
  exit code. A completion that failed (`Err` outcome) records
  `status: "failed"` and `exit: null`.
- **A denial's `mode` is the legacy value.** `log_command`
  (`src/daemon/utils/event_log.rs`), used for denials, timeouts and ghost
  refusals, writes `mode: "foreground"` or `"background"`. Every executed
  command writes `mode: "shell"`. After this phase `log_command` writes
  `mode: "shell"` too, and `wait` keeps being derived from the mode it was
  given, as today.

`daemoneye top`'s Activity row (`src/cli/commands/top.rs`, the `"command"`
arm) prints `exit -` for a missing exit. After this phase it prints `exit -`
for a `null` exit as well, not `exit null`.

The suite goes from **2622 to exactly 2625**.

## Constraints

- **Nothing reads `mode`** (no `get("mode")` in `src`), so changing its
  value breaks no reader. `exit` is read only by `top.rs`.
- **`log_shell_command` takes `exit_code: Option<i32>`.** The foreground
  callers pass `Some(code)`, the background launch passes `None`, and the
  background start error keeps `Some(-1)`.
- **The completion record is written where the outcome is known:** the
  background completion path in `src/daemon/executor/bg_shell.rs`, where
  `completion_message` gets its `outcome`. `await_and_notify` and
  `await_notify_and_close` both complete a run. Write the record once, in
  the place both pass through; if there is no such place, in both.
- `log_command_derives_wait_from_mode` (`event_log_tests.rs`) may assert the
  old mode value. If it does, update that assertion to `"shell"`; its `wait`
  assertions stay. That is not a new test.
- **Use `patch` with whole blocks,** never `patch_lines`. If a fix grows an
  allowlisted function, raise its `tests/hygiene_allowlist.txt` line and say
  why in the Update Log (M35 README).

## Spec

1. **Write two failing tests first,** and run each to keep its failing
   output:
   - `bg_completion_records_the_commands_exit_code` in `bg_shell.rs`'s tests,
     built like `bg_notify_pushes_a_completion_message_carrying_the_real_exit_code`
     (same `crate::test_home()` guard, an in-process host, a command ending
     `exit 4`). After the run completes, today's events segment holds a
     `command` record with `status: "completed"`, `wait: false` and `exit: 4`.
   - `a_denial_record_is_shell_mode` in `event_log_tests.rs`:
     `log_command(Some("s"), "foreground", "", "ls", "denied", "")` writes a
     record with `mode: "shell"` and `wait: true`.
2. **Fix both** — the completion record, `exit_code: Option<i32>` with
   `None` at the launch, and `log_command`'s mode. Both tests pass.
3. **The launch test** — `a_background_launch_records_no_exit_code` in
   `event_log_tests.rs`: `log_shell_command(.., false, .., None, ..)` writes
   `exit: null`. (It cannot compile before step 2, so it has no red run.)
4. **`top.rs`** — a `null` exit prints `-`. Extend the existing Activity-row
   test that checks the `"command"` arm with one `null` case, rather than
   adding a test.
5. **Mark M29** — in `docs/dev/milestones/M29-post-2.0-fixes/README.md`,
   append ` **Fixed in M35 phase-04.**` to the end of the O5 item's text.
6. **Red runs** — one `### Update — <date> (red runs)` entry pasting step 1's
   two `test result:` lines and assertion messages.
7. **Findings** — one `### Findings — <date>` entry from
   `grep -rn 'log_command(\|log_shell_command(' src --include=*.rs | grep -v _tests`:
   one line per caller and the `mode`/`exit` it now writes, or `none`.
8. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry.
9. **Commit** as `fix: background commands record their real exit; denials record shell mode (M29 O5)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `legacy_mode=0 new_tests=3 o5_fixed=1`
      (today `legacy_mode=1 new_tests=0 o5_fixed=0`).
- [ ] The red-runs entry shows both step-1 tests failing before the fix.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2625 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "legacy_mode=$(sed -n '/^pub fn log_command(/,/^}/p' src/daemon/utils/event_log.rs | grep -c '"mode":    mode') new_tests=$(( $(grep -c 'fn bg_completion_records_the_commands_exit_code' src/daemon/executor/bg_shell.rs) + $(grep -cE 'fn (a_denial_record_is_shell_mode|a_background_launch_records_no_exit_code)\(' src/daemon/utils/event_log_tests.rs) )) o5_fixed=$(grep -c 'Fixed in M35 phase-04' docs/dev/milestones/M29-post-2.0-fixes/README.md)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/daemon/utils/event_log.rs` and `event_log_tests.rs`,
  `src/daemon/executor/bg_shell.rs`, the `log_shell_command` callers in
  `dispatch_foreground.rs` and `dispatch_background.rs`,
  `src/cli/commands/top.rs` (the `"command"` arm and its test), the O5 line
  of the M29 README, and `tests/hygiene_allowlist.txt` (only per the rule
  above).

## Out of scope

- The `command_approval` records (they carry `decision`, not `exit`), and
  ghost `run_background` (`foreground.rs`) beyond the `log_command` mode
  change that reaches it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 19:20 (progress)

Started. Status flipped to in-progress (here and in the milestone README).
Working through the spec in order: the two red tests, the fixes, the launch
test, the `top.rs` null case, the M29 README mark, then the log entries and
the E2E block.

### Findings — 2026-10-31

`grep -rn 'log_command(\|log_shell_command(' src --include=*.rs | grep -v _tests`,
the callers and what each now writes (the `/log` parser in `chat.rs` shares
the name and is not a logging call):

- `log_command` (`event_log.rs`) — the mode is now `"shell"` always; `wait`
  still derived from the `mode` argument it is handed.
- `foreground.rs` 50, 277, 317, 349, 436, 474, 491 — `log_command` with
  `"foreground"` or `"background"`: `mode: "shell"`, `wait: true`/`false`
  from that argument.
- `mod.rs` 641, 739 — denials, timeouts and ghost refusals:
  `mode: "shell"`, `wait` from the approval's `mode`.
- `dispatch_foreground.rs` 304, 326, 404, 428 — `log_shell_command`:
  `mode: "shell"`, `wait: true`, `exit` `Some(result.exit_code)` on a
  completed run and `Some(-1)` on the two host-error and the shell-error
  path.
- `dispatch_background.rs` 138, 193 — the launch and its start error:
  `mode: "shell"`, `wait: false`, `exit` `None` on the launch and
  `Some(-1)` on the start error.
- `bg_shell.rs` 371 (`record_command_completion`) — the completion:
  `mode: "shell"`, `wait: false`, `exit` `Some(code)` on `Ok`, `None` and
  `status: "failed"` on `Err`.

### Update — 2026-10-31 19:55 (red runs)

Step 1's two tests, each run against the pre-fix tree and its `test result:`
line plus assertion message kept.

`bg_completion_records_the_commands_exit_code` (with the completion-record
fix not yet in place, the completion wrote no `command` record at all, so the
segment was missing):

```
running 1 test
test daemon::executor::bg_shell::tests::bg_completion_records_the_commands_exit_code ... FAILED

failures:

---- daemon::executor::bg_shell::tests::bg_completion_records_the_commands_exit_code stdout ----

thread 'daemon::executor::bg_shell::tests::bg_completion_records_the_commands_exit_code' (2296432) panicked at src/daemon/executor/bg_shell.rs:409:14:
today's event segment exists — the record was never written: Os { code: 2, kind: NotFound, message: "No such file or directory" }
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    daemon::executor::bg_shell::tests::bg_completion_records_the_commands_exit_code

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2544 filtered out; finished in 0.03s
```

`a_denial_record_is_shell_mode` (with `log_command` still writing the legacy
mode):

```
running 1 test
test daemon::utils::event_log::tests::a_denial_record_is_shell_mode ... FAILED

failures:

---- daemon::utils::event_log::tests::a_denial_record_is_shell_mode stdout ----

thread 'daemon::utils::event_log::tests::a_denial_record_is_shell_mode' (2300852) panicked at src/daemon/utils/event_log_tests.rs:613:9:
assertion `left == right` failed: the record says the shell substrate ran it
  left: String("foreground")
 right: "shell"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2546 filtered out; finished in 0.03s
```

Step 2 fixed both: the two tests now pass, and `a_background_launch_records_no_exit_code`
(step 3, which could not compile before `exit_code: Option<i32>`) passes too.
